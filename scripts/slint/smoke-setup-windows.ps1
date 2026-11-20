# Окна режимов установщика (ПП6, задача 10) сквозь настоящий hlsr.exe со сборкой dev-hooks, на
# невидимом рабочем столе Win32: HLSR-Setup.exe без /S -> окно установщика 640x560;
# HLSR.exe --hlsr-uninstall без /S -> окно удаления; HLSR.exe --hlsr-updater --plan=<нет файла> ->
# окно апдейтера с ошибкой плана. Каждое окно снимается HLSR_SMOKE_SNAPSHOT через 800 мс и
# закрывается; код выхода 0, PNG нужного размера, после окна установщика реестр (JSON) не изменился.
#
# Настоящие реестр, меню "Пуск", рабочий стол, %APPDATA%\hlsr, %LOCALAPPDATA%\hlsr и C:\HLSR не
# трогаются (те же перенаправления, что check-setup.ps1). Окна - только под headless-win.py:
#   scripts/slint/serial.sh cargo build -p hlsr-app --features dev-hooks -p hlsr-release
#   python scripts/slint/headless-win.py powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts/slint/smoke-setup-windows.ps1 -Exe <target>\debug\hlsr.exe -HlsrRelease <target>\debug\hlsr-release.exe
#
# Строки сообщений - ASCII: Windows PowerShell 5.1 читает файл без BOM в кодировке ANSI.
param(
    [Parameter(Mandatory = $true)] [string] $Exe,
    [Parameter(Mandatory = $true)] [string] $HlsrRelease,
    [string] $Work = '',
    [string] $Shots = '',
    [switch] $Keep
)
$ErrorActionPreference = 'Stop'
$script:failures = @()
if (-not $env:HLSR_HEADLESS_DESKTOP) {
    Write-Host 'run under scripts/slint/headless-win.py: windows must not appear on the screen'
    exit 2
}

function Fail([string] $message) {
    Write-Host "FAIL  $message"
    $script:failures += $message
}

function Ok([string] $message) {
    Write-Host "ok    $message"
}

function Check([bool] $condition, [string] $message) {
    if ($condition) { Ok $message } else { Fail $message }
}

function Run([string] $file, [string[]] $arguments) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $file @arguments | Out-Host
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
}

function PngSize([string] $path) {
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    Add-Type -AssemblyName System.Drawing
    $image = [System.Drawing.Image]::FromFile($path)
    try { return @($image.Width, $image.Height) } finally { $image.Dispose() }
}

$Exe = (Resolve-Path -LiteralPath $Exe).Path
$HlsrRelease = (Resolve-Path -LiteralPath $HlsrRelease).Path
if (-not $Work) {
    $Work = Join-Path ([IO.Path]::GetTempPath()) ("hlsr-smoke-setup-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
}
New-Item -ItemType Directory -Force $Work | Out-Null
$Work = (Resolve-Path -LiteralPath $Work).Path
if (-not $Shots) { $Shots = Join-Path $Work 'shots' }
New-Item -ItemType Directory -Force $Shots | Out-Null
Write-Host "== work: $Work"

$tree = Join-Path $Work 'build'
$downloads = Join-Path $Work 'Downloads'
$home_ = Join-Path $Work 'home'
$temp = Join-Path $Work 'temp'
$data = Join-Path $Work 'data'
$logs = Join-Path $Work 'logs'
$dist = Join-Path $Work 'dist'
$library = Join-Path $Work 'Library'
$dry = Join-Path $Work 'dry-run.jsonl'
$registry = Join-Path $Work 'registry.json'
$installDir = Join-Path $Work 'HLSR\Launcher'

foreach ($dir in @($tree, (Join-Path $tree 'resources'), $downloads, $home_, $temp, $data)) {
    New-Item -ItemType Directory -Force $dir | Out-Null
}
[IO.File]::WriteAllText((Join-Path $data 'library.json'), (@{ libraryPath = $library } | ConvertTo-Json -Compress))
Copy-Item -LiteralPath $Exe -Destination (Join-Path $tree 'HLSR.exe')
[IO.File]::WriteAllText((Join-Path $tree 'resources\app.bin'), 'res 0.5.0')
$setup = Join-Path $downloads 'HLSR-Setup-0.5.0-ru.exe'

$saved = @{}
$vars = @{
    HLSR_FAKE_REGISTRY       = $registry
    HLSR_KNOWN_FOLDERS       = $home_
    HLSR_TEMP_DIR            = $temp
    HLSR_LAUNCH_DRY_RUN      = $dry
    HLSR_DATA_DIR            = $data
    HLSR_LOGS_DIR            = $logs
    HLSR_SMOKE_SNAPSHOT      = $null
    PORTABLE_EXECUTABLE_FILE = $null
}
foreach ($k in $vars.Keys) {
    $saved[$k] = [Environment]::GetEnvironmentVariable($k)
}
try {
    Write-Host '== pack (hlsr-release setup)'
    $code = Run $HlsrRelease @('setup', '--exe', (Join-Path $tree 'HLSR.exe'), '--resources', (Join-Path $tree 'resources'),
        '--version', '0.5.0', '--out', $dist, '--allow-unversioned-exe')
    $packed = Join-Path $dist 'HLSR-Setup-0.5.0.exe'
    if ($code -eq 0 -and (Test-Path -LiteralPath $packed)) { Copy-Item -LiteralPath $packed -Destination $setup }
    Check ($code -eq 0 -and (Test-Path -LiteralPath $setup)) "installer packed (code $code)"
    foreach ($k in $vars.Keys) { [Environment]::SetEnvironmentVariable($k, $vars[$k]) }

    Write-Host '== installer window (no /S)'
    $shot = Join-Path $Shots 'installer.png'
    $env:HLSR_SMOKE_SNAPSHOT = $shot
    $code = Run $setup @()
    Check ($code -eq 0) "installer window exit code $code"
    $size = PngSize $shot
    Check ($null -ne $size -and ($size[0] % 640) -eq 0 -and ($size[1] * 640) -eq ($size[0] * 560)) "installer snapshot $($size -join 'x') (640x560 x system scale)"
    Check (-not (Test-Path -LiteralPath $registry)) 'registry untouched by opening the window'
    Check (-not (Test-Path -LiteralPath $installDir)) 'nothing installed by opening the window'

    Write-Host '== silent install, then the uninstall window'
    $env:HLSR_SMOKE_SNAPSHOT = $null
    Remove-Item Env:\HLSR_SMOKE_SNAPSHOT -ErrorAction SilentlyContinue
    $code = Run $setup @('/S', "/D=$installDir")
    Check ($code -eq 0) "silent install exit code $code"
    $installed = Join-Path $installDir 'HLSR.exe'
    $before = [IO.File]::ReadAllText($registry)
    $shot = Join-Path $Shots 'uninstall.png'
    $env:HLSR_SMOKE_SNAPSHOT = $shot
    $code = Run $installed @('--hlsr-uninstall')
    Check ($code -eq 0) "uninstall window exit code $code"
    $size = PngSize $shot
    Check ($null -ne $size -and ($size[1] * 640) -eq ($size[0] * 560)) "uninstall snapshot $($size -join 'x')"
    Check ([IO.File]::ReadAllText($registry) -eq $before) 'registry unchanged: the window only asks'
    Check (Test-Path -LiteralPath $installed) 'HLSR.exe still installed'

    Write-Host '== updater window (missing plan)'
    $shot = Join-Path $Shots 'updater.png'
    $env:HLSR_SMOKE_SNAPSHOT = $shot
    $code = Run $installed @('--hlsr-updater', ("--plan=" + (Join-Path $Work 'no-plan.json')))
    $size = PngSize $shot
    Check ($null -ne $size -and $size[0] -gt $size[1]) "updater snapshot $($size -join 'x') (exit code $code)"
    $updaterLog = Join-Path $home_ 'AppData\Local\HLSR\logs\updater.log'
    $logged = @(Get-ChildItem -LiteralPath $home_ -Recurse -Filter 'updater.log' -ErrorAction SilentlyContinue)
    Check ($logged.Count -eq 1) "updater.log in the redirected LOCALAPPDATA ($($logged.FullName -join ', '))"
} finally {
    foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    if (-not $Keep) {
        Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($script:failures.Count) {
    Write-Host "FAILED: $($script:failures.Count)"
    exit 1
}
Write-Host 'all checks passed'
exit 0
