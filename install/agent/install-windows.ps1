# Installs the booty-call agent on a Windows switch node. Run elevated.
#
# usage:
#   powershell -ExecutionPolicy Bypass -File .\install-windows.ps1 `
#     -Bin .\booty-call-agent.exe -BoxId gpu -OsId windows -AllowedPeers fabrico `
#     -Control fabrico.tawny-wyrm.ts.net `
#     -BootEntries '{"debian":1,"windows":2}'
#
# -AllowedPeers: tailscale node name(s) of the control (comma-separated).
#
# -Control: address of the control plane (tailnet FQDN or IP, optional
# :port, default 8765). With it, the agent registers itself with the
# control at startup; without it, register the node manually.
#
# -BootEntries maps every os_id this box can boot to its UEFI boot entry
# number (visible in the firmware boot menu, or `efibootmgr -v` from Linux).
#
# Also disables Fast Startup (HiberbootEnabled=0, the same registry value
# the 'Turn on fast startup' Control Panel checkbox sets), so shutdown is a
# full shutdown — required for the Wake on LAN flow.
param(
  [Parameter(Mandatory)] [string] $Bin,
  [Parameter(Mandatory)] [string] $BoxId,
  [Parameter(Mandatory)] [string] $OsId,
  [Parameter(Mandatory)] [string] $AllowedPeers,
  [string] $BootEntries = '{}',
  [int] $Port = 8766,
  [string] $Control = ''
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

$peersJson = ($AllowedPeers -split ',' | Where-Object { $_.Trim() }) | ForEach-Object { '"' + $_.Trim() + '"' } -join ', '
$portLine = if ($Port -ne 8766) { ", `n  `"port`": $Port" } else { '' }
$controlLine = if ($Control) { ", `n  `"control`": `"$Control`"" } else { '' }
$cfg = @"
{
  "box_id": "$BoxId",
  "os_id": "$OsId",
  "allowed_peers": [$peersJson],
  "boot_entries": $BootEntries$portLine$controlLine
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
# Disable Fast Startup so 'shutdown' powers the box fully off (needed for WoL).
$hiberPath = 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Power'
Set-ItemProperty -Path $hiberPath -Name HiberbootEnabled -Value 0
Write-Host 'disabled Fast Startup (HiberbootEnabled=0)'

Start-Service bootycallagent
if (-not $Control) { Write-Warning 'no -Control given; the agent will not self-register (register the node manually)' }
Write-Host "installed booty-call-agent service for $BoxId/$OsId"
