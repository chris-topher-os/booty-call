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
  frontend at `http://<control-tailnet-fqdn>:8765`.

## Developing

The dev environment is managed by [mise](https://mise.jdx.dev). See `mise.toml`
for commands.

# Quick Start

## Target box prerequisites

Set the following BIOS configurations:

- ErP / EuP ready: **off**
- Wake-on-PCIe / Power-On-by-NIC: **on**

Note down the MAC address of the NIC you will wake: it's the NIC on the LAN
segment where the control machine lives, and it must be the same NIC the agent
installs WoL on. From the target box:

- Debian: `ip -br link` — the address column is the `link/ether` MAC of each
  interface (ignore `lo` and `tailscale0`)
- Windows: `getmac /v` — the Network Address column

## Control setup

On macOS:

```sh
mise run build-control
./install/control/install-macos.sh target/release/booty-call-control
```

The installer writes a config template to `~/.config/booty-call/control.json`
and the control reads it at startup. Edit it to match your setup: one entry
per box, `default_os` being the OS the firmware boots on cold start, and
`wol.mac` the NIC MAC noted in Target box prerequisites. The optional
`wol.ip` (the box's LAN IP) lets the control also unicast the magic packet.

For HTTPS on the tailnet FQDN, run `tailscale serve --bg 8765` on the control
node (it walks you through enabling tailnet HTTPS certificates if needed);
the PWA is then also at `https://<control-tailnet-fqdn>`.

## Install an agent

To get boot entry numbers, run `sudo efibootmgr -v` and look for `BootXXXX`
values. While `efibootmgr` lists them in hex, the agent expects the decimal
equivalent. Also note that every agent needs an entry for every agent
(including itself).

On Linux:

```sh
mise run build-agent-linux
sudo install/agent/install-linux.sh --bin target/.../booty-call-agent
```

On Windows (requires an elevated terminal):

```pwsh
powershell -ExecutionPolicy Bypass -File install/agent/install-windows.ps1 -Bin target/.../booty-call-agent.exe
```

The first install writes a config template with placeholders (`/etc/booty-call/agent.json`
or `C:\ProgramData\booty-call\agent.json`): fill in `box_id`, `os_id`,
`allowed_peers` (the control node name), `boot_entries`, and `control`
(the Tailnet FQDN of the control node), then restart the agent service
(`booty-call-agent` / `bootycallagent`).

On an already-installed machine, `mise run deploy` rebuilds and reinstalls
everything without touching the config.
