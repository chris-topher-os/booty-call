#!/usr/bin/env bash
# Installs the oswitch control plane as a macOS launchd agent (per-user,
# no sudo). Linux systemd and other platforms: add a path here when needed.
#
# usage:
#   ./install.sh /path/to/oswitch-control            # install + start
#   ./install.sh uninstall                            # stop + remove
set -euo pipefail

LABEL=dev.chris.oswitch.control
BIN_DIR="$HOME/.local/share/oswitch"
CFG_DIR="$HOME/.config/oswitch"
CFG="$CFG_DIR/control.json"
LOG_DIR="$HOME/Library/Logs/oswitch"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"

generate_config() {
  local key
  key="$(openssl rand -hex 32)"
  mkdir -p "$CFG_DIR"
  cat > "$CFG" <<EOF
{
  "listen": "",
  "admin_key": "$key",
  "state_file": "$HOME/.local/share/oswitch/boxes.json",
  "poll_interval_secs": 3,
  "boxes": [
    {
      "id": "box1",
      "name": "Box 1",
      "default_os": "<os the firmware boots on cold start>",
      "wol": { "mac": "<mac of the box's NIC>", "ip": "<box LAN ip, optional>" }
    }
  ]
}
EOF
  chmod 600 "$CFG"
  echo "wrote config template to $CFG — fill in the boxes, then note the admin_key"
}

uninstall() {
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed $LABEL"
}

case "${1:-}" in
  uninstall) uninstall; exit 0;;
esac

BIN="${1:?usage: install.sh /path/to/oswitch-control | uninstall}"
[[ -x "$BIN" ]] || { echo "binary not found or not executable: $BIN" >&2; exit 1; }

mkdir -p "$BIN_DIR" "$LOG_DIR"
install -m 755 "$BIN" "$BIN_DIR/oswitch-control"
[[ -f "$CFG" ]] || generate_config

# ServeDir resolves "static/" relative to the working directory; expect the
# binary at <repo>/target/release/oswitch-control.
STATIC_DIR="$(cd "$(dirname "$BIN")/../.." 2>/dev/null && pwd)/crates/control/static"
if [[ ! -d "$STATIC_DIR" ]]; then
  echo "could not find the PWA static dir (expected <repo>/target/release layout)." >&2
  echo "build from the repo root: cargo build --release -p control" >&2
  exit 1
fi

cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN_DIR/oswitch-control</string>
    <string>--config</string>
    <string>$CFG</string>
  </array>
  <key>WorkingDirectory</key><string>$STATIC_DIR</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$LOG_DIR/control.out</string>
  <key>StandardErrorPath</key><string>$LOG_DIR/control.err</string>
</dict>
</plist>
EOF

launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl print "$DOMAIN/$LABEL" | head -5
echo "installed $LABEL (config: $CFG, logs: $LOG_DIR)"
