# Проверка LiveSplit.BunnySplit.dll на настоящем .NET Framework (Windows PowerShell 5.1):
# ComponentSettings.SetSettings с настройками, которые лаунчер пишет в раскладку HL1.
# Код выхода 0 — без исключения; иначе 1 и текст исключения.
#   powershell -STA -File bunnysplit-netfx-check.ps1 -LiveSplit <папка LiveSplit> -Dll <файл>
param(
  [Parameter(Mandatory = $true)][string]$LiveSplit,
  [Parameter(Mandatory = $true)][string]$Dll
)
$ErrorActionPreference = 'Stop'
$LiveSplit = (Resolve-Path $LiveSplit).Path
$Dll = (Resolve-Path $Dll).Path

# Зависимости LiveSplit.Core (UpdateManager и др.) берутся из папки LiveSplit.
$resolve = [ResolveEventHandler] {
  param($sender, $e)
  $file = Join-Path $LiveSplit ((New-Object System.Reflection.AssemblyName($e.Name)).Name + '.dll')
  if (Test-Path $file) { return [System.Reflection.Assembly]::LoadFrom($file) }
  return $null
}
[AppDomain]::CurrentDomain.add_AssemblyResolve($resolve)

try {
  # Те же значения, что BUNNYSPLIT_SETTINGS в livesplit-splits.js.
  $xml = New-Object System.Xml.XmlDocument
  $xml.LoadXml('<Settings><Version>2.0.2.0</Version><EnableAutoSplit>True</EnableAutoSplit><SplitOnGameEnd>True</SplitOnGameEnd><SplitOnHL1Chapters>True</SplitOnHL1Chapters><SplitOnOP4Chapters>False</SplitOnOP4Chapters><SplitOnBSChapters>False</SplitOnBSChapters><SplitOnGMCChapters>False</SplitOnGMCChapters><SplitOnMaps>False</SplitOnMaps><EnableAutoReset>True</EnableAutoReset><EnableAutoStart>True</EnableAutoStart></Settings>')
  [void][System.Reflection.Assembly]::LoadFrom((Join-Path $LiveSplit 'LiveSplit.Core.dll'))
  $assembly = [System.Reflection.Assembly]::LoadFrom($Dll)
  $settings = [Activator]::CreateInstance($assembly.GetType('LiveSplit.BunnySplit.ComponentSettings', $true))
  $settings.SetSettings($xml.DocumentElement)
  Write-Output "OK: $([System.Runtime.InteropServices.RuntimeEnvironment]::GetSystemVersion()) $Dll"
  exit 0
} catch {
  $inner = $_.Exception
  while ($inner.InnerException) { $inner = $inner.InnerException }
  Write-Output "FAIL: $($inner.GetType().FullName): $($inner.Message)"
  exit 1
}
