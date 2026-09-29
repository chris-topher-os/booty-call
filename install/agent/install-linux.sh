#!/usr/bin/env bash
# Installs the booty-call agent on a Debian switch node (run with sudo).
#
# usage:
#   sudo ./install-linux.sh \
#     --bin /path/to/booty-call-agent \
#     --box-id gpu --os-id debian --allowed-peers fabrico \
#     --boot-entries '{"debian":1,"windows":2}'
#
# --allowed-peers: tailscale node name(s) of the control (and any other
# peers that may talk to this agent), comma-separated.
#
# --boot-entries maps every os_id this box can boot to its UEFI boot entry
# number (from `sudo efibootmgr -v`). The agent needs the full map because
# it may be asked to reboot into any of the other partitions.
set -euo pipefail

BIN="" BOX_ID="" OS_ID="" PEERS="" PORT="" ENTRIES='{}'
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin) BIN="$2"; shift 2;;
    --box-id) BOX_ID="$2"; shift 2;;
    --os-id) OS_ID="$2"; shift 2;;
    --allowed-peers) PEERS="$2"; shift 2;;
    --port) PORT="$2"; shift 2;;
    --boot-entries) ENTRIES="$2"; shift 2;;
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

cat > /etc/booty-call/agent.json <<EOF
{
  "box_id": "$BOX_ID",
  "os_id": "$OS_ID",
  "allowed_peers": $PEERS_JSON,
  "boot_entries": $ENTRIES$PORT_LINE
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
echo "installed booty-call-agent for $BOX_ID/$OS_ID"
