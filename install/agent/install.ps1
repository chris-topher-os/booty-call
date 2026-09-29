# Installs the booty-call agent on a Windows switch node. Run elevated.
#
# usage:
#   powershell -ExecutionPolicy Bypass -File .\install.ps1 `
#     -Bin .\booty-call-agent.exe -BoxId gpu -OsId windows -Token <token-from-control> `
#     -BootEntries '{"debian":1,"windows":2}'
#
# -BootEntries maps every os_id this box can boot to its UEFI boot entry
# number (visible in the firmware boot menu, or `efibootmgr -v` from Linux).
param(
  [Parameter(Mandatory)] [string] $Bin,
  [Parameter(Mandatory)] [string] $BoxId,
  [Parameter(Mandatory)] [string] $OsId,
  [Parameter(Mandatory)] [string] $Token,
  [string] $BootEntries = '{}',
  [int] $Port = 8766
)

$ErrorActionPreference = 'Stop'
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
  ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { throw 'must run as administrator' }

# Validate the boot entries JSON before touching anything.
try { $null = $BootEntries | ConvertFrom-Json } catch { throw "bad -BootEntries JSON: $_" }

$binDir = 'C:\Program Files\booty-call'
$cfgDir = 'C:\ProgramData\booty-call'
New-Item -ItemType Directory -Force -Path $binDir, $cfgDir | Out-Null
Copy-Item $Bin (Join-Path $binDir 'booty-call-agent.exe') -Force

$portLine = if ($Port -ne 8766) { ", `n  `"port`": $Port" } else { '' }
$cfg = @"
{
  "box_id": "$BoxId",
  "os_id": "$OsId",
  "token": "$Token",
  "boot_entries": $BootEntries$portLine
}
"@
Set-Content -Path (Join-Path $cfgDir 'agent.json') -Value $cfg -Encoding ascii

$exe = Join-Path $binDir 'booty-call-agent.exe'
$cfgPath = Join-Path $cfgDir 'agent.json'
sc.exe query bootycallagent 2>$null | Out-Null
if ($LASTEXITCODE -eq 0) {
  sc.exe stop bootycallagent | Out-Null
  sc.exe delete bootycallagent | Out-Null
}
sc.exe create bootycallagent binPath= "`"$exe`" --config `"$cfgPath`"" start= auto | Out-Null
sc.exe description bootycallagent "booty-call agent ($BoxId/$OsId)" | Out-Null
Start-Service bootycallagent
Write-Host "installed booty-call-agent service for $BoxId/$OsId"
