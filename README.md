# booty-call

Remotely boot any OS partition of a multi-boot target box over Tailscale,
whether the box is offline or already booted into a different OS.

```
PWA / curl --tailnet--> control (macOS daemon) --WoL broadcast (LAN)--> box NIC (S5)
                        control --tailnet--> agent on each OS (status / reboot)
```

- **agent** (`crates/agent`): runs on every OS partition. Two endpoints only:
  `GET /status`, `POST /reboot` (sets the one-shot UEFI `BootNext` variable
  for the requested OS, then reboots). No other surface. Liveness = the
  control polling `/status`; no custom heartbeat.
- **auth**: no app-level tokens. Tailscale provides transport encryption and
  identity; each agent/control resolves the source IP's tailnet node name via
  `tailscale whois` and 403s anyone not in its `allowed_peers` list. Agents
  only bind the tailnet interface — no tailscale, no service.
- **control** (`crates/control`): node registry, liveness poller, per-box
  switch state machine, and a single-page PWA (radio button per OS partition).
- Switch from online: one reboot. Switch from offline: WoL wakes the box into
  its default OS, then one corrective reboot if that isn't the target.

## Layout

```
crates/common    wire types + agent config
crates/agent     booty-call-agent (tiny_http; systemd on Linux, service on Windows)
crates/control   booty-call-control (axum) + static/ PWA
install/agent    install-linux.sh (Debian/systemd), install-windows.ps1 (elevated PowerShell)
install/control  install-macos.sh (macOS launchd agent)
```

## Building

The dev environment is managed by [mise](https://mise.jdx.dev) (`mise.toml`):
the pinned Rust toolchain and its cross targets install automatically, and the
apt cross compilers on the Linux build box are declared under `[bootstrap]`.

```sh
mise trust                        # once, per clone
mise bootstrap packages apply     # Linux cross box only; installs the apt cross gccs (sudo)
mise run build                    # host release build
mise run build-agent-linux        # static x86_64 agent for the Debian partition
mise run build-agent-windows      # x86_64 agent for the Windows partition
mise run build-control            # control plane for the host
```

The musl cross build links through `x86_64-linux-gnu-gcc`; that is wired up in
`.cargo/config.toml`.

## Control setup (macOS)

```sh
mise run build-control
./install/control/install-macos.sh target/release/booty-call-control
```

Edits `~/.config/booty-call/control.json`: `allowed_peers` (tailscale node
names of the devices allowed to use the API/PWA), box id/name, `default_os`
(what the firmware boots on cold start), and WoL MAC (and optional LAN IP).
Logs: `~/Library/Logs/booty-call/`.

## Register a node + install an agent

From a device whose node name is in the control's `allowed_peers`, once per OS
(ts_ip from `tailscale ip -4` on the partition):

```sh
curl -s -X POST http://<control>:8765/api/boxes/<box>/nodes \
  -H 'Content-Type: application/json' \
  -d '{"os_id":"debian","ts_ip":"100.x.y.z"}'
```

Then install the agent on the partition:

```sh
# Debian partition:
sudo install/agent/install-linux.sh --bin target/.../booty-call-agent \
  --box-id <box> --os-id debian --allowed-peers <control-node-name> \
  --boot-entries '{"debian":1,"windows":2}'

# Windows partition (elevated):
powershell -ExecutionPolicy Bypass -File install/agent/install-windows.ps1 \
  -Bin booty-call-agent.exe -BoxId <box> -OsId windows -AllowedPeers <control-node-name> \
  -BootEntries '{"debian":1,"windows":2}'
```

Boot entry numbers come from the firmware boot menu or `sudo efibootmgr -v`.
The agent needs the full map (it may be asked to reboot into any partition).

## API

All routes 403 unless the caller's source IP resolves (via `tailscale whois`)
to a node in the control's `allowed_peers`.

| method | path | body | result |
|---|---|---|---|
| GET | `/api/state` | – | boxes, per-node liveness, switch phase |
| POST | `/api/boxes/:id/nodes` | `{"os_id","ts_ip","port"?}` | `{"os_id"}` |
| DELETE | `/api/boxes/:id/nodes/:os` | – | unregister |
| POST | `/api/boxes/:id/switch` | `{"os"}` | 202 accepted / 409 already switching or stuck |

PWA: browse `http://<control-tailnet-ip>:8765/` from a browser on an allowed
node.

## Firmware prerequisites (one-time, on the box)

- ErP / EuP ready **off** (or the NIC loses S5 power and WoL is dead).
- Wake-on-PCIe / Power-On-by-NIC **on**.
- NIC wake enabled in the OS (`ethtool` `Wake-on: g` on Debian, handled by
  `install-windows.ps1` on Windows; disable Fast Startup on Windows).

## Notes

- `BootNext` is one-shot: the firmware clears it after one attempt. If the
  target entry fails and the box falls through to the default, the control
  issues one corrective reboot; after that the box is marked stuck and the
  PWA offers a retry.
- The control must be L2-local to the box for WoL. If it later isn't, only
  the offline-branch degrades (marked stuck: "no response to wake-on-LAN");
  online switching is unaffected.
- The whois result is cached per source IP for 60s in both directions
  (allowed and denied), so the poller doesn't re-query tailscale every tick.
