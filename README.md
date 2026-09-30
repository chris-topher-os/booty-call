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
SPEC.md          the original project spec, verbatim
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

## Install an agent

Registration is automatic: with `--control` set, the agent registers itself
with the control at startup (self-registration: the control trusts the
request's tailnet source IP as the node's address). It retries every minute
until the control answers, and re-registration is idempotent, so a missing
control at install time or a deleted registry entry heals itself.

```sh
# Debian partition:
sudo install/agent/install-linux.sh --bin target/.../booty-call-agent \
  --box-id <box> --os-id debian --allowed-peers <control-node-name> \
  --control <control-tailnet-fqdn> \
  --boot-entries '{"debian":1,"windows":2}'

# Windows partition (elevated):
powershell -ExecutionPolicy Bypass -File install/agent/install-windows.ps1 \
  -Bin booty-call-agent.exe -BoxId <box> -OsId windows -AllowedPeers <control-node-name> \
  -Control <control-tailnet-fqdn> \
  -BootEntries '{"debian":1,"windows":2}'
```

`--control` is the control's tailnet FQDN or IP, optionally with `:port`
(default 8765). Without it, register the node manually via the API instead.

Boot entry numbers come from the firmware boot menu or `sudo efibootmgr -v`
(the `BootXXXX` number; `XXXX` is hex, so `Boot0010` is 16, written as the
JSON decimal value). The agent needs the full map (it may be asked to reboot
into any partition).

## API

All routes 403 unless the caller's source IP resolves (via `tailscale whois`)
to a node in the control's `allowed_peers`.

| method | path | body | result |
|---|---|---|---|
| GET | `/api/state` | – | boxes, per-node liveness, switch phase |
| POST | `/api/boxes/:id/nodes` | `{"os_id","ts_ip","port"?}` | `{"os_id"}`; idempotent upsert |
| DELETE | `/api/boxes/:id/nodes/:os` | – | unregister |

`POST .../nodes` accepts two callers: a node in `allowed_peers` (may register
any node) or the node itself — agents self-register at startup by claiming
their own tailnet source IP as `ts_ip`, which the control then uses verbatim.
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
