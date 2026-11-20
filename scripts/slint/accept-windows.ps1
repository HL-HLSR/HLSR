# Приёмка 0.4.7 для Windows (ПП6, задача 8): готовые артефакты выпуска проходят проверки замороженной
# обновлялки 0.4.7 (tools/compat-047/accept-release.js). Выпуск, не прошедший приёмку, не публикуется.
#
# Два режима:
#   -Dist <каталог>   готовый каталог выпуска Windows (HLSR-Setup-<v>.exe, latest.yml, launcher-channel.json)
#                     проверяется как есть. Боевой канал - вшитые ключи 0.4.7; тестовый - с -TestKeys <json>.
#   -Exe <HLSR.exe>   собрать выпуск здесь: hlsr-release setup (+ stage ресурсов) -> Node
#                     launcher-release-prepare.js; канал подписан ТЕСТОВЫМ ключом (создаётся во временном
#                     каталоге; боевой ключ и HLSR_MANIFEST_PRIVATE_KEY_PATH не используются) -> приёмка.
#
#   -Qa        упаковка под QA-идентичностью (com.hlsrteam.hlsr.qa, свой GUID и каталог): не встаёт поверх боевой
#   -Native    путь Б без Electron: замороженные install.js и process-wait.js запускают установщик
#   -DevHooks  путь Б на сборке dev-hooks с перенаправлениями (HLSR_FAKE_REGISTRY, HLSR_KNOWN_FOLDERS,
#              HLSR_TEMP_DIR, HLSR_LAUNCH_DRY_RUN, HLSR_DATA_DIR): настоящие реестр, меню "Пуск", рабочий стол,
#              %APPDATA%\hlsr и C:\HLSR не трогаются - так проверяется машина, где стоит настоящая 0.4.7.
#              Без -DevHooks путь Б пишет в настоящий HKCU и разрешён только на одноразовом раннере CI
#              (GITHUB_ACTIONS=true).
#
# Примеры:
#   # локально, без окон и без настоящего реестра: hlsr.exe со сборкой dev-hooks
#   scripts/slint/serial.sh cargo build -p hlsr-app --features dev-hooks -p hlsr-release
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/slint/accept-windows.ps1 -DevHooks `
#       -Exe <target>\debug\hlsr.exe -HlsrRelease <target>\debug\hlsr-release.exe
#   # CI: боевая сборка под QA-идентичностью, настоящий HKCU раннера
#   pwsh scripts/slint/accept-windows.ps1 -Exe target\release\hlsr.exe -Qa -Native
#
# Строки сообщений и код - ASCII: Windows PowerShell 5.1 читает файл без BOM в кодировке ANSI.
param(
    [string] $Dist = '',
    [string] $Exe = '',
    [string] $HlsrRelease = '',
    [string] $TestKeys = '',
    [string] $Version = '',
    [string] $Work = '',
    [switch] $Qa,
    [switch] $Native,
    [switch] $DevHooks,
    [switch] $Keep
)
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$accept = Join-Path $repo 'tools\compat-047\accept-release.js'
$nodeTools = Join-Path $repo 'tools\release\node'

if (-not $Dist -and -not $Exe) { throw 'need -Dist <release dir> or -Exe <HLSR.exe>' }
if ($DevHooks) { $Native = $true }

# Код возврата node; stderr идёт прямо в консоль.
function Invoke-Node([string[]] $arguments, [hashtable] $environment = @{}) {
    $saved = @{}
    foreach ($name in $environment.Keys) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $environment[$name])
    }
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & node @arguments | Out-Host
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
        foreach ($name in $environment.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name]) }
    }
}

function Invoke-Tool([string[]] $arguments) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $script:HlsrRelease @arguments | Out-Host
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
}

if (-not $HlsrRelease) {
    foreach ($candidate in @('target\release\hlsr-release.exe', 'target\debug\hlsr-release.exe')) {
        $path = Join-Path $repo $candidate
        if (Test-Path -LiteralPath $path) { $HlsrRelease = $path; break }
    }
}
if ($HlsrRelease) { $HlsrRelease = (Resolve-Path -LiteralPath $HlsrRelease).Path }
$script:HlsrRelease = $HlsrRelease

if (-not $Work) {
    $Work = Join-Path ([IO.Path]::GetTempPath()) ('hlsr-accept-windows-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
}
New-Item -ItemType Directory -Force $Work | Out-Null
$Work = (Resolve-Path -LiteralPath $Work).Path
Write-Host "== work: $Work"
$acceptArgs = @()

try {
    if (-not $Dist) {
        if (-not $HlsrRelease) { throw 'hlsr-release.exe not found: cargo build -p hlsr-release, or pass -HlsrRelease' }
        $Exe = (Resolve-Path -LiteralPath $Exe).Path
        $Dist = Join-Path $Work 'dist\windows'
        if (-not $Version) {
            $previous = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            $found = & $HlsrRelease exe-version $Exe 2>$null
            $exit = $LASTEXITCODE
            $ErrorActionPreference = $previous
            if ($exit -eq 0 -and $found -match '^(\d+\.\d+\.\d+)') { $Version = $Matches[1] }
            else {
                # Нет ProductVersion (пробный exe): числа версии проекта без -dev.
                $workspace = (& $HlsrRelease version).Trim()
                if ($workspace -notmatch '^(\d+\.\d+\.\d+)') { throw "workspace version: $workspace" }
                $Version = $Matches[1]
            }
        }
        Write-Host "== pack $Version ($(if ($Qa) { 'QA' } else { 'production' }) identity)"

        # Ресурсы: в CI - настоящие (hlsr-release stage), под dev-hooks - один файл (быстро, без 7za и оверлея).
        $resources = Join-Path $Work 'resources-src'
        if ($DevHooks) {
            New-Item -ItemType Directory -Force (Join-Path $resources '7zip') | Out-Null
            [IO.File]::WriteAllText((Join-Path $resources '7zip\LICENSE.txt'), 'resources 0.5.0')
        } else {
            $code = Invoke-Tool @('stage', '--platform', 'windows', '--repo', $repo, '--out', $resources)
            if ($code -ne 0) { throw "hlsr-release stage failed ($code)" }
        }
        $setupArgs = @('setup', '--exe', $Exe, '--resources', $resources, '--version', $Version, '--out', $Dist)
        if ($Qa) { $setupArgs += '--qa' }
        if ($DevHooks) { $setupArgs += '--allow-unversioned-exe' }
        $code = Invoke-Tool $setupArgs
        if ($code -ne 0) { throw "hlsr-release setup failed ($code)" }

        # Тестовые ключи: во временном каталоге вне репозитория. Боевой ключ канала и ключи аттестации
        # (HLSR-secrets) не читаются.
        $keysDir = Join-Path $Work 'keys'
        $keysJson = & node (Join-Path $nodeTools 'tests\support\make-keys.js') $keysDir $Version
        if ($LASTEXITCODE -ne 0) { throw 'make-keys failed' }
        $keys = $keysJson | ConvertFrom-Json
        $prepareArgs = @((Join-Path $nodeTools 'launcher-release-prepare.js'), '--version', $Version, '--build-dir', $Dist,
            '--out', $Dist, '--chunk-store', (Join-Path $Work 'chunk-store'), '--test-public-key', $keys.publicKey)
        if ($Qa) { $prepareArgs += '--qa-profile' }
        $code = Invoke-Node $prepareArgs @{
            HLSR_MANIFEST_PRIVATE_KEY_PATH = $keys.privateKey
            HLSR_ATTEST_KEYS_DIR           = $keys.attestDir
            HLSR_CHANGELOG_JSON            = $keys.changelog
        }
        if ($code -ne 0) { throw "launcher-release-prepare failed ($code)" }
        $TestKeys = $keys.testKeys
    }

    $acceptArgs = @($accept, '--dist', $Dist, '--platform', 'win32')
    if ($TestKeys) { $acceptArgs += @('--test-keys', $TestKeys) }
    if ($HlsrRelease) { $acceptArgs += @('--hlsr-release', $HlsrRelease) }
    if ($Qa) { $acceptArgs += '--expect-qa' }
    if ($DevHooks) { $acceptArgs += @('--dev-hooks', '--allow-unversioned') }
    if ($Native) { $acceptArgs += '--native-windows' }
    if ($Keep) { $acceptArgs += '--keep' }
    Write-Host '== acceptance'
    $code = Invoke-Node $acceptArgs
    if ($code -ne 0) { throw "acceptance failed ($code)" }
    Write-Host 'ok    acceptance passed'
} finally {
    if (-not $Keep) { Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue }
    else { Write-Host "work dir kept: $Work" }
}
