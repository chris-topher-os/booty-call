#!/usr/bin/env bash
# Installs the booty-call agent on a Debian switch node (run with sudo).
#
# usage:
#   sudo ./install-linux.sh \
#     --bin /path/to/booty-call-agent \
#     --box-id gpu --os-id debian --allowed-peers fabrico \
#     --control fabrico.tawny-wyrm.ts.net \
#     --boot-entries '{"debian":1,"windows":2}'
#
# --allowed-peers: tailscale node name(s) of the control (and any other
# peers that may talk to this agent), comma-separated.
#
# --control: address of the control plane (tailnet FQDN or IP, optional
# :port, default 8765). With it, the agent registers itself with the
# control at startup; without it, register the node manually.
#
# --boot-entries maps every os_id this box can boot to its UEFI boot entry
# number (from `sudo efibootmgr -v`). The agent needs the full map because
# it may be asked to reboot into any of the other partitions.
#
# --wol-iface: enable Wake-on-LAN (magic packet) on this interface and
# persist it across reboots via a systemd service. Without it, the script
# auto-detects a single PCI-backed Ethernet device, and skips (with a
# warning) if it can't find exactly one.
set -euo pipefail

BIN="" BOX_ID="" OS_ID="" PEERS="" PORT="" ENTRIES='{}' CONTROL="" WOL_IFACE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin) BIN="$2"; shift 2;;
    --box-id) BOX_ID="$2"; shift 2;;
    --os-id) OS_ID="$2"; shift 2;;
    --allowed-peers) PEERS="$2"; shift 2;;
    --port) PORT="$2"; shift 2;;
    --boot-entries) ENTRIES="$2"; shift 2;;
    --control) CONTROL="$2"; shift 2;;
    --wol-iface) WOL_IFACE="$2"; shift 2;;
    *) echo "unknown arg: $1" >&2; exit 1;;
  esac
done

[[ -n "$BIN" && -n "$BOX_ID" && -n "$OS_ID" && -n "$PEERS" ]] || {
  echo "required: --bin --box-id --os-id --allowed-peers" >&2; exit 1; }
PEERS_JSON=$(python3 -c "import json,sys; print(json.dumps([p for p in sys.argv[1].split(',') if p]))" "$PEERS")
command -v efibootmgr >/dev/null || echo "warning: efibootmgr not found (install the 'efibootmgr' package)" >&2

install -D -m 755 "$BIN" /usr/local/bin/booty-call-agent
mkdir -p /etc/booty-call

PORT_LINE=""
[[ -n "$PORT" ]] && PORT_LINE=",
  \"port\": $PORT"
CONTROL_LINE=""
[[ -n "$CONTROL" ]] && CONTROL_LINE=",
  \"control\": \"$CONTROL\""

cat > /etc/booty-call/agent.json <<EOF
{
  "box_id": "$BOX_ID",
  "os_id": "$OS_ID",
  "allowed_peers": $PEERS_JSON,
  "boot_entries": $ENTRIES$PORT_LINE$CONTROL_LINE
}
EOF

cat > /etc/systemd/system/booty-call-agent.service <<EOF
[Unit]
Description=booty-call agent ($BOX_ID/$OS_ID)
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
if [[ -z "$CONTROL" ]]; then
  echo "warning: no --control given; the agent will not self-register (register the node manually)" >&2
fi
# --- Wake-on-LAN ---------------------------------------------------------------

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
    echo "warning: no PCI-backed Ethernet interface found; skipping Wake-on-LAN (use --wol-iface)" >&2
  else
    echo "warning: multiple PCI-backed Ethernet interfaces (${cands[*]}); skipping Wake-on-LAN (use --wol-iface)" >&2
    WOL_IFACE=""
  fi
fi

if [[ -n "$WOL_IFACE" ]]; then
  [[ -e "/sys/class/net/$WOL_IFACE" ]] || { echo "no such interface: $WOL_IFACE" >&2; exit 1; }
  if ! ethtool -s "$WOL_IFACE" wol g; then
    echo "warning: ethtool -s $WOL_IFACE wol g failed (driver may not support magic-packet WoL); skipping persistence" >&2
  else
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
  fi
fi

echo "installed booty-call-agent for $BOX_ID/$OS_ID"
