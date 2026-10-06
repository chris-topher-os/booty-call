# Installs the booty-call agent on a Windows switch node. Run elevated.
#
# usage:
#   powershell -ExecutionPolicy Bypass -File .\install-windows.ps1 -Bin .\booty-call-agent.exe
#
# Config lives at C:\ProgramData\booty-call\agent.json. An existing config
# is kept; on first install a template is written and must be filled in
# before the agent will do anything useful:
#
#   box_id         id of this box (shared across its partitions)
#   os_id          the os this partition runs (pre-filled: windows)
#   allowed_peers  tailscale node name(s) allowed to talk to this agent
#   boot_entries   maps every os_id this box can boot to its UEFI boot
#                  entry number (visible in the firmware boot menu, or
#                  `efibootmgr -v` from Linux)
#   control        address of the control plane (tailnet FQDN or IP,
#                  optional :port, default 8765); with it the agent
#                  registers itself at startup
#
# Also disables Fast Startup (HiberbootEnabled=0, the same registry value
# the 'Turn on fast startup' Control Panel checkbox sets), so shutdown is
# a full shutdown — required for the Wake on LAN flow.
param(
  [Parameter(Mandatory)] [string] $Bin
)

$ErrorActionPreference = 'Stop'
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
  ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { throw 'must run as administrator' }

$binDir = 'C:\Program Files\booty-call'
$cfgDir = 'C:\ProgramData\booty-call'
New-Item -ItemType Directory -Force -Path $binDir, $cfgDir | Out-Null

$cfgPath = Join-Path $cfgDir 'agent.json'
$wroteTemplate = $false
if (-not (Test-Path $cfgPath)) {
  $template = @'
{
  "box_id": "<this box's id, e.g. gpu>",
  "os_id": "windows",
  "allowed_peers": ["<control node name>"],
  "boot_entries": { "<os_id": <decimal UEFI boot entry number> },
  "control": "<control tailnet FQDN>"
}
'@
  Set-Content -Path $cfgPath -Value $template -Encoding ascii
  $wroteTemplate = $true
  Write-Host "wrote config template to $cfgPath — fill it in"
}

# Stop a running agent first so its binary can be replaced (reinstalling).
sc.exe query bootycallagent 2>$null | Out-Null
if ($LASTEXITCODE -eq 0) {
  sc.exe stop bootycallagent | Out-Null
  sc.exe delete bootycallagent | Out-Null
}
Copy-Item $Bin (Join-Path $binDir 'booty-call-agent.exe') -Force

$exe = Join-Path $binDir 'booty-call-agent.exe'
sc.exe create bootycallagent binPath= "`"$exe`" --config `"$cfgPath`"" start= auto | Out-Null
sc.exe description bootycallagent "booty-call agent" | Out-Null
# Disable Fast Startup so 'shutdown' powers the box fully off (needed for WoL).
$hiberPath = 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Power'
Set-ItemProperty -Path $hiberPath -Name HiberbootEnabled -Value 0
Write-Host 'disabled Fast Startup (HiberbootEnabled=0)'

Start-Service bootycallagent
if ($wroteTemplate) {
  Write-Host "edit $cfgPath, then: Restart-Service bootycallagent"
}
Write-Host 'installed booty-call-agent service'
