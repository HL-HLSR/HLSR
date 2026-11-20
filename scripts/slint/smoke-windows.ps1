# Пробные запуски HLSR.exe на раннере Windows (CI, задача 22): снимок окна и
# проверка журнала. Сборка — с признаком dev-hooks (снимок — render_to_image GPUI).
#
#   pwsh scripts/slint/smoke-windows.ps1 -Exe target\release\hlsr.exe -Out $env:RUNNER_TEMP\smoke
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/slint/smoke-windows.ps1 -Exe ... -Out ...
#
# Код и строки сообщений - ASCII, кириллица журнала в регулярных выражениях - \uXXXX: Windows
# PowerShell 5.1 читает файл без BOM в кодировке ANSI (как check-setup.ps1).
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = 'Stop'
$script:failures = @()

function Fail([string] $message) {
    Write-Host "FAIL    $message"
    $script:failures += $message
}

function Ok([string] $message) {
    Write-Host "ok      $message"
}

# Ширина и высота PNG из заголовка IHDR.
function PngSize([string] $path) {
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $w = ([int]$bytes[16] -shl 24) -bor ([int]$bytes[17] -shl 16) -bor ([int]$bytes[18] -shl 8) -bor [int]$bytes[19]
    $h = ([int]$bytes[20] -shl 24) -bor ([int]$bytes[21] -shl 16) -bor ([int]$bytes[22] -shl 8) -bor [int]$bytes[23]
    return @($w, $h)
}

$script:ExePath = (Resolve-Path -LiteralPath $Exe).Path

# Строки main.log (crates/hlsr-app/src/window.rs), размеры окна - в логических px:
# "[window] окно 1600x900, масштаб 100 %, зум 1", "[ui-scale] масштаб 150 %, окно 1600x900".
$script:ReWindow = '\[window\] \u043e\u043a\u043d\u043e (\d+)x(\d+), \u043c\u0430\u0441\u0448\u0442\u0430\u0431'
$script:ReUiScale = '\[ui-scale\] \u043c\u0430\u0441\u0448\u0442\u0430\u0431 \d+ %, \u043e\u043a\u043d\u043e (\d+)x(\d+)'

# Процессы hlsr этого exe.
function Ours() {
    return @(Get-Process -Name hlsr -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $script:ExePath })
}

function Smoke([string] $name, [hashtable] $vars) {
    Write-Host "== $name"
    $dir = Join-Path $Out $name
    New-Item -ItemType Directory -Force $dir | Out-Null
    $png = Join-Path $dir 's.png'
    $log = Join-Path $dir 'logs\main.log'
    # Не только раннер: библиотека с «установленной» игрой во временном каталоге (пустую
    # recoverLibraryPath заменяет найденной, например C:\HLSR), Discord выключен и перенос
    # config.rpc уже сделан (иначе лаунчер подключится к клиенту Discord пользователя).
    $data = Join-Path $dir 'data'
    $library = Join-Path $dir 'library'
    New-Item -ItemType Directory -Force $data, (Join-Path $library 'Games\hl1-won') | Out-Null
    [IO.File]::WriteAllText((Join-Path $library 'Games\hl1-won\installed.json'), '{"state":"ready"}')
    [IO.File]::WriteAllText((Join-Path $data 'library.json'), (@{ libraryPath = $library } | ConvertTo-Json -Compress))
    [IO.File]::WriteAllText((Join-Path $data 'discord-rpc.json'),
        '{"schemaVersion":2,"enabled":false,"source":"user","migrated":true,"legacyValue":null}')
    $saved = @{}
    $all = @{
        HLSR_DATA_DIR       = $data
        HLSR_LOGS_DIR       = (Join-Path $dir 'logs')
        HLSR_SMOKE_SNAPSHOT = $png
    } + $vars
    foreach ($k in $all.Keys) {
        $saved[$k] = [Environment]::GetEnvironmentVariable($k)
        [Environment]::SetEnvironmentVariable($k, $all[$k])
    }
    try {
        $process = Start-Process -FilePath $Exe -PassThru -Wait
        if ($process.ExitCode -eq 0) { Ok "exit code 0" } else { Fail "$name - exit code $($process.ExitCode)" }
        # Завершаем только процессы этого exe (не HLSR.exe пользователя и не чужие проверки).
        $deadline = (Get-Date).AddSeconds(30)
        while ((Ours) -and (Get-Date) -lt $deadline) {
            Start-Sleep -Milliseconds 250
        }
        Ours | Stop-Process -Force
    } finally {
        foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    }

    if (-not (Test-Path $log)) { Fail "$name - no main.log"; return }
    $text = Get-Content -Raw -Encoding UTF8 $log
    $text -split "`r?`n" | Where-Object { $_ -match '^\[' } | ForEach-Object { Write-Host "        $_" }

    if (-not (Test-Path $png)) { Fail "$name - no snapshot"; return }
    $size = PngSize $png
    $window = [regex]::Matches($text, $script:ReWindow)
    $scale = [regex]::Matches($text, $script:ReUiScale)
    if ($window.Count -eq 0) { Fail "$name - no [window] line"; return }
    # Последний размер окна: после [ui-scale] - его, иначе - [window].
    $last = if ($scale.Count) { $scale[$scale.Count - 1] } else { $window[$window.Count - 1] }
    $w = [int]$last.Groups[1].Value
    $h = [int]$last.Groups[2].Value
    # Снимок - в физических px: логический размер окна, умноженный на масштаб системы (одинаковый
    # по обеим осям, округление - до 1 px).
    $factor = $size[0] / $w
    if ($factor -gt 0 -and [math]::Abs($size[1] - $h * $factor) -le 1) {
        Ok "snapshot $($size[0])x$($size[1]) - as the window ${w}x${h} x $([math]::Round($factor, 2))"
    } else {
        Fail "$name - snapshot $($size[0])x$($size[1]), window ${w}x${h}"
    }
}

# Режим очистки после удаления (cleanup-cli 0.4.7) сквозь hlsr.exe: путь библиотеки из library.json,
# код 0; без флагов - ничего не удалено, код 3 (NOTHING_TO_DO).
function CleanupMode() {
    Write-Host '== --hlsr-uninstall-cleanup'
    $dir = Join-Path $Out 'cleanup'
    $data = Join-Path $dir 'data'
    New-Item -ItemType Directory -Force $data | Out-Null
    $library = Join-Path $dir 'Library'
    [IO.File]::WriteAllText((Join-Path $data 'library.json'), (@{ libraryPath = $library } | ConvertTo-Json -Compress))
    $saved = @{ HLSR_DATA_DIR = $env:HLSR_DATA_DIR; HLSR_LOGS_DIR = $env:HLSR_LOGS_DIR }
    $env:HLSR_DATA_DIR = $data
    $env:HLSR_LOGS_DIR = Join-Path $dir 'logs'
    try {
        # Через конвейер: hlsr.exe выпуска — приложение с подсистемой GUI, и Windows PowerShell без
        # конвейера его не ждёт и stdout не забирает.
        $global:LASTEXITCODE = $null
        $printed = @(& $Exe '--hlsr-uninstall-cleanup' '--print-library' | ForEach-Object { $_ })
        if ($LASTEXITCODE -eq 0 -and ($printed -join '') -eq $library) { Ok "--print-library: $library" }
        else { Fail "cleanup --print-library - code $LASTEXITCODE, output '$printed'" }
        & $Exe '--hlsr-uninstall-cleanup' | Out-Null
        if ($LASTEXITCODE -eq 3) { Ok 'without flags: code 3' } else { Fail "cleanup without flags - code $LASTEXITCODE" }
    } finally {
        $env:HLSR_DATA_DIR = $saved.HLSR_DATA_DIR
        $env:HLSR_LOGS_DIR = $saved.HLSR_LOGS_DIR
    }
}

New-Item -ItemType Directory -Force $Out | Out-Null
CleanupMode
# Рендерер один: DirectX 11 (без видеокарты, на раннере, - WARP сам).
Smoke 'default' @{}
Smoke 'ui-scale-1.5' @{ HLSR_SMOKE_UI_SCALE = '1.5' }

if ($script:failures.Count) {
    Write-Host "result: failures $($script:failures.Count)"
    exit 1
}
Write-Host 'result: no failures'
exit 0
