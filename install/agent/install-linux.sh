#!/usr/bin/env bash
# Installs the booty-call agent on a Debian switch node (run with sudo).
#
# usage:
#   sudo ./install-linux.sh --bin /path/to/booty-call-agent [--wol-iface enp3s0]
#
# Config lives at /etc/booty-call/agent.json. An existing config is kept;
# on first install a template is written and must be filled in before the
# agent will do anything useful:
#
#   box_id         id of this box (shared across its partitions)
#   os_id          the os this partition runs
#   allowed_peers  tailscale node name(s) allowed to talk to this agent
#   boot_entries   maps every os_id this box can boot to its UEFI boot
#                  entry number (from `sudo efibootmgr -v`; it prints hex,
#                  the agent wants decimal). The agent needs the full map
#                  because it may be asked to reboot into any partition.
#   control        address of the control plane (tailnet FQDN or IP,
#                  optional :port, default 8765); with it the agent
#                  registers itself at startup
#
# Wake-on-LAN: enabled (magic packet) on --wol-iface and persisted via
# booty-call-wol.service. Without --wol-iface, the interface is reused
# from an already-installed wol service; otherwise the script
# auto-detects a single PCI-backed Ethernet device, and fails if it can't
# find exactly one (the offline boot flow needs a working WoL, so a failed
# setup aborts the install before anything is written).
set -euo pipefail

BIN="" WOL_IFACE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin) BIN="$2"; shift 2;;
    --wol-iface) WOL_IFACE="$2"; shift 2;;
    *) echo "unknown arg: $1" >&2; exit 1;;
  esac
done
[[ -n "$BIN" ]] || { echo "required: --bin" >&2; exit 1; }
command -v efibootmgr >/dev/null || echo "warning: efibootmgr not found (install the 'efibootmgr' package)" >&2

WOL_SERVICE=/etc/systemd/system/booty-call-wol.service
if [[ -z "$WOL_IFACE" && -f "$WOL_SERVICE" ]]; then
  WOL_IFACE=$(awk '/^ExecStart=/{print $3}' "$WOL_SERVICE")
fi

# --- Wake-on-LAN (set up first, so a failed WoL init aborts before installing)

# List Ethernet interfaces backed by a PCI device (i.e. physical NICs;
# tailscale/veth/lo/bridges have no PCI device behind them).
pci_ifaces() {
  local d name
  for d in /sys/class/net/*; do
    name=$(basename "$d")
    [[ "$name" == lo ]] && continue
    [[ -e "$d/device" ]] || continue
    # PCI devices live under /devices/pci<domain>:<bus>/...
    readlink -f "$d/device" | grep -qE '/devices/pci[0-9a-f]+:' && echo "$name"
  done
}

if [[ -z "$WOL_IFACE" ]]; then
  mapfile -t cands < <(pci_ifaces)
  if [[ ${#cands[@]} -eq 1 ]]; then
    WOL_IFACE="${cands[0]}"
    echo "auto-detected WoL interface: $WOL_IFACE"
  elif [[ ${#cands[@]} -eq 0 ]]; then
    echo "error: no PCI-backed Ethernet interface found; cannot enable Wake-on-LAN (use --wol-iface)" >&2
    exit 1
  else
    echo "error: multiple PCI-backed Ethernet interfaces (${cands[*]}); cannot auto-detect the WoL interface (use --wol-iface)" >&2
    exit 1
  fi
fi

[[ -e "/sys/class/net/$WOL_IFACE" ]] || { echo "error: no such interface: $WOL_IFACE" >&2; exit 1; }
if ! ethtool -s "$WOL_IFACE" wol g; then
  echo "error: ethtool -s $WOL_IFACE wol g failed (driver may not support magic-packet WoL)" >&2
  exit 1
fi

ETHTOOL_BIN=$(command -v ethtool)
cat > /etc/systemd/system/booty-call-wol.service <<EOF
[Unit]
Description=Enable Wake-on-LAN on $WOL_IFACE (booty-call)
After=systemd-udev-settle.service

[Service]
Type=oneshot
ExecStart=$ETHTOOL_BIN -s $WOL_IFACE wol g
Restart=on-failure
RestartSec=2
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now booty-call-wol.service
echo "enabled Wake-on-LAN (magic packet) on $WOL_IFACE, persisted via booty-call-wol.service"

TEMPLATE=0
mkdir -p /etc/booty-call
if [[ ! -f /etc/booty-call/agent.json ]]; then
  cat > /etc/booty-call/agent.json <<'EOF'
{
  "box_id": "<this box's id, e.g. gpu>",
  "os_id": "<os this partition runs, e.g. debian>",
  "allowed_peers": ["<control node name>"],
  "boot_entries": { "<os_id>": <decimal UEFI boot entry number> },
  "control": "<control tailnet FQDN>"
}
EOF
  TEMPLATE=1
  echo "wrote config template to /etc/booty-call/agent.json — fill it in"
fi

# Stop a running agent first so its binary can be replaced (reinstalling).
systemctl stop booty-call-agent 2>/dev/null || true
install -D -m 755 "$BIN" /usr/local/bin/booty-call-agent

cat > /etc/systemd/system/booty-call-agent.service <<EOF
[Unit]
Description=booty-call agent
After=network-online.target tailscaled.service
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/booty-call-agent --config /etc/booty-call/agent.json
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now booty-call-agent.service
systemctl --no-pager --lines=0 status booty-call-agent.service || true

if [[ $TEMPLATE -eq 1 ]]; then
  echo "edit /etc/booty-call/agent.json, then: systemctl restart booty-call-agent"
fi
echo "installed booty-call-agent"
