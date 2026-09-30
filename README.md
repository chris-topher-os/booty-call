# booty-call

Remotely boot any OS partition of a multi-boot target box over Tailscale,
whether the box is offline or already booted into a different OS.

```
PWA --tailnet--> control --LAN--> Wake on LAN
                 control --tailnet--> agents
```

- **agent** (`crates/agent`): runs on every OS partition, allowing the
  control to issue reboot commands and query liveness.
- **control** (`crates/control`): remote-control interface with a PWA
  frontend.

## Developing

The dev environment is managed by [mise](https://mise.jdx.dev). See `mise.toml`
for commands.

# Quick Start

## Control setup

On macOS:

```sh
mise run build-control
./install/control/install-macos.sh target/release/booty-call-control
```

## Target box prerequisites

Set the following BIOS configurations:

- ErP / EuP ready: **off**
- Wake-on-PCIe / Power-On-by-NIC: **on**

## Install an agent

The install scripts also handle the OS-side Wake on LAN prerequisites:

- Debian: enables magic-packet WoL (`ethtool -s <iface> wol g`) and persists
  it across reboots via `booty-call-wol.service`. Pass `--wol-iface <iface>`
  to pick the NIC; without it, the script auto-detects it when there is
  exactly one physical (PCI-backed) Ethernet interface.
- Windows: disables Fast Startup (`HiberbootEnabled=0`) so that shutdown is
  a real shutdown.

Replace `<control-address>` with the Tailnet FQDN of the control node.

To get boot entry numbers, run `sudo efibootmgr -v` and look for `BootXXXX`
values. While `efibootmgr` lists them in hex, the booty-call agent expects
the decimal equivalent. Also note that every agent needs an entry for every
agent (including itself) in its `boot-entries`.

On Linux:

```sh
mise run build-agent-linux
sudo install/agent/install-linux.sh --bin target/.../booty-call-agent \
  --box-id <box> --os-id linux --allowed-peers <control-node-name> \
  --control <control-address> \
  --boot-entries '{"<os>": <id>, "<sib_os>": <sib_id>}'
```

On Windows (requires an elevated terminal):

```pwsh
powershell -ExecutionPolicy Bypass -File install/agent/install-windows.ps1 \
  -Bin booty-call-agent.exe -BoxId <box> -OsId windows -AllowedPeers <control-node-name> \
  -Control <control-address> \
  -BootEntries '{"<os>": <id>, "<sib_os>": <sib_id>}'
```
