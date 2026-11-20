# Сквозная проверка тихих режимов установщика Windows (ПП6, задачи 5 и 9) сквозь hlsr.exe со сборкой
# dev-hooks: HLSR-Setup.exe /S --updated /D=... -> код 0, дерево, реестр (JSON), ярлыки .lnk;
# --updated /S --force-run без /D= -> каталог из реестра, запуск HLSR.exe в dry-run с чистым
# окружением; HLSR.exe --hlsr-uninstall /S -> ключи удалены, каталог удаляет команда из dry-run;
# /HLSRDELETEGAMES /HLSRDELETESETTINGS -> очистка библиотеки и настроек; --hlsr-uninstall-cleanup.
#
# Настоящие реестр, меню "Пуск", рабочий стол, %APPDATA%\hlsr, %LOCALAPPDATA%\hlsr и C:\HLSR не
# трогаются: реестр - файл JSON (HLSR_FAKE_REGISTRY), папки пользователя, данные и журналы - внутри
# рабочего каталога (HLSR_KNOWN_FOLDERS, HLSR_TEMP_DIR, HLSR_DATA_DIR, HLSR_LOGS_DIR), запуск
# процессов (HLSR.exe, PowerShell, icacls) - только запись в dry-run (HLSR_LAUNCH_DRY_RUN). Окон нет.
# Установщик собирает hlsr-release (exe + нагрузка, Р1).
#
#   scripts/slint/serial.sh cargo build -p hlsr-app --features dev-hooks -p hlsr-release
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/slint/check-setup.ps1 `
#       -Exe <target>\debug\hlsr.exe -HlsrRelease <target>\debug\hlsr-release.exe
#
# Строки сообщений - ASCII: Windows PowerShell 5.1 читает файл без BOM в кодировке ANSI.
param(
    [Parameter(Mandatory = $true)] [string] $Exe,
    [Parameter(Mandatory = $true)] [string] $HlsrRelease,
    [string] $Work = '',
    [switch] $Keep
)
$ErrorActionPreference = 'Stop'
$script:failures = @()

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

function Sha([string] $path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
}

function ReadJson([string] $path) {
    return (Get-Content -Raw -Encoding UTF8 -LiteralPath $path | ConvertFrom-Json)
}

function DryRun() {
    if (-not (Test-Path -LiteralPath $script:dry)) { return @() }
    return @(Get-Content -Encoding UTF8 -LiteralPath $script:dry | Where-Object { $_ } | ForEach-Object { $_ | ConvertFrom-Json })
}

function RegValue($model, [string] $key, [string] $name) {
    $section = $model.PSObject.Properties | Where-Object { $_.Name -ieq $key } | Select-Object -First 1
    if (-not $section) { return $null }
    $value = $section.Value.PSObject.Properties | Where-Object { $_.Name -ieq $name } | Select-Object -First 1
    if (-not $value) { return $null }
    return $value.Value.data
}

# Журнал установщика (stderr) идет прямо в консоль: в Windows PowerShell 5.1 перенаправленный
# stderr стал бы записями ошибок.
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

$Exe = (Resolve-Path -LiteralPath $Exe).Path
$HlsrRelease = (Resolve-Path -LiteralPath $HlsrRelease).Path
if (-not $Work) {
    $Work = Join-Path ([IO.Path]::GetTempPath()) ("hlsr-check-setup-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
}
New-Item -ItemType Directory -Force $Work | Out-Null
$Work = (Resolve-Path -LiteralPath $Work).Path
Write-Host "== work: $Work"

$tree = Join-Path $Work 'build'
$downloads = Join-Path $Work 'Downloads'
$home_ = Join-Path $Work 'home'
$temp = Join-Path $Work 'temp'
$data = Join-Path $Work 'data'
$logs = Join-Path $Work 'logs'
$dist = Join-Path $Work 'dist'
$library = Join-Path $Work 'Library'
$script:dry = Join-Path $Work 'dry-run.jsonl'
$registry = Join-Path $Work 'registry.json'
$installDir = Join-Path $Work 'HLSR\Launcher'
$guid = '3f824379-6b41-5725-b424-59cb87f2acae'
$installKey = "HKEY_CURRENT_USER\Software\$guid"
$uninstallKey = "HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\$guid"

foreach ($dir in @($tree, (Join-Path $tree 'resources'), $downloads, $home_, $temp, $data)) {
    New-Item -ItemType Directory -Force $dir | Out-Null
}
[IO.File]::WriteAllText((Join-Path $data 'library.json'), (@{ libraryPath = $library } | ConvertTo-Json -Compress))
Copy-Item -LiteralPath $Exe -Destination (Join-Path $tree 'HLSR.exe')
[IO.File]::WriteAllText((Join-Path $tree 'resources\app.bin'), 'res 0.5.0')
$setup = Join-Path $downloads 'HLSR-Setup-0.5.0-ru.exe'

$saved = @{}
$vars = @{
    HLSR_FAKE_REGISTRY         = $registry
    HLSR_KNOWN_FOLDERS         = $home_
    HLSR_TEMP_DIR              = $temp
    HLSR_LAUNCH_DRY_RUN        = $script:dry
    HLSR_DATA_DIR              = $data
    HLSR_LOGS_DIR              = $logs
    SLINT_SCALE_FACTOR         = '1.25'
    PORTABLE_EXECUTABLE_DIR    = 'X:\unpacked'
    PORTABLE_EXECUTABLE_FILE   = $null
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

    Write-Host '== /S --updated /D='
    $code = Run $setup @('/S', '--updated', "/D=$installDir")
    Check ($code -eq 0) "silent install exit code $code"
    $exe = Join-Path $installDir 'HLSR.exe'
    Check ((Test-Path -LiteralPath $exe) -and (Sha $exe) -eq (Sha (Join-Path $tree 'HLSR.exe'))) 'HLSR.exe = exe prefix of the installer'
    Check ((Sha (Join-Path $installDir 'resources\app.bin')) -eq (Sha (Join-Path $tree 'resources\app.bin'))) 'resources\app.bin'
    Check (Test-Path -LiteralPath (Join-Path $installDir 'resources\install-files.json')) 'install-files.json'
    $owner = [IO.File]::ReadAllText((Join-Path $installDir '.hlsr-install-owner.json'))
    $model = ReadJson $registry
    $installId = RegValue $model $installKey 'HLSRInstallId'
    Check ($installId -match '^\{[0-9A-F]{8}-[0-9A-F]{4}-4[0-9A-F]{3}-[89AB][0-9A-F]{3}-[0-9A-F]{12}\}$') "HLSRInstallId $installId"
    Check ($owner -eq "{`"schemaVersion`":1,`"appId`":`"$guid`",`"installId`":`"$installId`"}") 'owner marker = registry installId'
    Check ((RegValue $model $installKey 'InstallLocation') -eq $installDir) 'InstallLocation'
    Check ((RegValue $model $uninstallKey 'DisplayVersion') -eq '0.5.0') 'DisplayVersion 0.5.0'
    Check ((RegValue $model $uninstallKey 'QuietUninstallString') -eq "`"$exe`" --hlsr-uninstall /S") 'QuietUninstallString'
    Check ((RegValue $model 'HKEY_CURRENT_USER\Software\HLSR' 'InstallerOrigin') -eq 'ru') 'InstallerOrigin ru'
    $shell = New-Object -ComObject WScript.Shell
    $links = @(Get-ChildItem -LiteralPath $home_ -Recurse -Filter '*.lnk')
    Check ($links.Count -eq 2) "shortcuts: $($links.Count) (Start Menu, Desktop)"
    foreach ($link in $links) {
        $target = $shell.CreateShortcut($link.FullName).TargetPath
        Check ($target -ieq $exe) "$($link.Name) -> $target"
    }
    Check (Test-Path -LiteralPath (Join-Path $library '.hlsr\library.json')) 'library marker'
    $icacls = @(DryRun | Where-Object { $_.file -like '*icacls.exe' })
    Check ($icacls.Count -eq 1 -and $icacls[0].args[0] -eq $library) 'icacls on the library (dry-run)'

    Write-Host '== --updated /S --force-run (no /D=)'
    Remove-Item -LiteralPath $script:dry -Force
    $code = Run $setup @('--updated', '/S', '--force-run')
    Check ($code -eq 0) "silent update exit code $code"
    $model = ReadJson $registry
    Check ((RegValue $model $installKey 'HLSRInstallId') -eq $installId) 'HLSRInstallId unchanged'
    $spawned = @(DryRun | Where-Object { $_.kind -eq 'spawn' })
    Check ($spawned.Count -eq 1 -and $spawned[0].file -eq $exe -and $spawned[0].cwd -eq $installDir) 'HLSR.exe launched from the registry InstallLocation'
    if ($spawned.Count -eq 1) {
        $names = @($spawned[0].env.PSObject.Properties | ForEach-Object { $_.Name })
        Check (-not ($names | Where-Object { $_ -like 'PORTABLE_EXECUTABLE_*' -or $_ -like 'SLINT_*' -or $_ -like 'WINIT_*' })) 'launch environment without PORTABLE_EXECUTABLE_*, SLINT_*, WINIT_*'
        Check ($spawned[0].args.Count -eq 0) 'launched without arguments'
    }

    Write-Host '== HLSR.exe --hlsr-uninstall /S'
    Remove-Item -LiteralPath $script:dry -Force
    $code = Run $exe @('--hlsr-uninstall', '/S')
    Check ($code -eq 0) "silent uninstall exit code $code"
    $model = ReadJson $registry
    Check ($null -eq (RegValue $model $installKey 'HLSRInstallId') -and $null -eq (RegValue $model $uninstallKey 'DisplayVersion')) 'install and uninstall keys removed'
    Check (@(Get-ChildItem -LiteralPath $home_ -Recurse -Filter '*.lnk').Count -eq 0) 'own shortcuts removed'
    $removal = @(DryRun | Where-Object { $_.kind -eq 'spawn' -and $_.file -like '*powershell.exe' })
    Check ($removal.Count -eq 1 -and $removal[0].env.HLSR_REMOVE_DIR -eq $installDir -and $removal[0].cwd -eq $temp) 'removal command in dry-run (HLSR_REMOVE_DIR, cwd = TEMP)'
    if ($removal.Count -eq 1) {
        # Скрипт удаления выполняется здесь же, без нового процесса и окна.
        $env:HLSR_REMOVE_DIR = $removal[0].env.HLSR_REMOVE_DIR
        Invoke-Expression $removal[0].args[$removal[0].args.Count - 1]
        Remove-Item Env:\HLSR_REMOVE_DIR
        Check (-not (Test-Path -LiteralPath $installDir)) 'install directory removed by the removal script'
    }

    Write-Host '== /HLSRDELETEGAMES /HLSRDELETESETTINGS'
    Remove-Item -LiteralPath $script:dry -Force
    $code = Run $setup @('/S', "/D=$installDir")
    Check ($code -eq 0) "reinstall exit code $code"
    New-Item -ItemType Directory -Force (Join-Path $library 'Games\hl') | Out-Null
    [IO.File]::WriteAllText((Join-Path $library 'Games\hl\hl.exe'), 'game')
    [IO.File]::WriteAllText((Join-Path $data 'settings.json'), '{}')
    [IO.File]::WriteAllText((Join-Path $data 'game-stats.json'), '{}')
    $code = Run $exe @('--hlsr-uninstall', '/S', '/HLSRDELETEGAMES', '/HLSRDELETESETTINGS')
    Check ($code -eq 0) "uninstall with data exit code $code"
    Check (-not (Test-Path -LiteralPath $library)) 'library purged'
    Check (-not (Test-Path -LiteralPath (Join-Path $data 'settings.json'))) 'settings removed'
    Check (Test-Path -LiteralPath (Join-Path $data 'game-stats.json')) 'game-stats.json kept (/HLSRDELETESETTINGS without /HLSRDELETEPROFILE)'

    Write-Host '== --hlsr-uninstall-cleanup --print-library'
    [IO.File]::WriteAllText((Join-Path $data 'library.json'), (@{ libraryPath = $library } | ConvertTo-Json -Compress))
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        # Через конвейер: hlsr.exe выпуска - приложение с подсистемой GUI, и Windows PowerShell без
        # конвейера его не ждет и stdout не забирает.
        $global:LASTEXITCODE = $null
        $printed = @(& $Exe '--hlsr-uninstall-cleanup' '--print-library' | ForEach-Object { $_ })
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    Check ($code -eq 0 -and ($printed -join '') -eq $library) "cleanup prints the library (code $code)"

    Write-Host '== logs stay in HLSR_LOGS_DIR'
    Check (Test-Path -LiteralPath (Join-Path $logs 'main.log')) 'main.log in the redirected logs dir'
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
