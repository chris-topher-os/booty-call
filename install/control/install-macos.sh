#!/usr/bin/env bash
# Installs the booty-call control plane as a macOS launchd agent (per-user,
# no sudo). Linux systemd and other platforms: add a path here when needed.
#
# usage:
#   ./install-macos.sh /path/to/booty-call-control            # install + start
#   ./install-macos.sh uninstall                            # stop + remove
set -euo pipefail

LABEL=dev.chris.booty-call.control
BIN_DIR="$HOME/.local/share/booty-call"
CFG_DIR="$HOME/.config/booty-call"
CFG="$CFG_DIR/control.json"
LOG_DIR="$HOME/Library/Logs/booty-call"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"

generate_config() {
  mkdir -p "$CFG_DIR"
  cat > "$CFG" <<EOF
{
  "listen": "",
  "state_file": "$HOME/.local/share/booty-call/boxes.json",
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
  echo "wrote config template to $CFG — fill in the boxes"
}

uninstall() {
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed $LABEL"
}

case "${1:-}" in
  uninstall) uninstall; exit 0;;
esac

BIN="${1:?usage: install-macos.sh /path/to/booty-call-control | uninstall}"
[[ -x "$BIN" ]] || { echo "binary not found or not executable: $BIN" >&2; exit 1; }

# Stop a running control first so its binary can be replaced (reinstalling).
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true

mkdir -p "$BIN_DIR" "$LOG_DIR" "$(dirname "$PLIST")"
install -m 755 "$BIN" "$BIN_DIR/booty-call-control"
[[ -f "$CFG" ]] || generate_config

# ServeDir resolves "static/" relative to the working directory, so the
# working directory is crates/control (the parent of the static dir). Expect
# the binary at <repo>/target/release/booty-call-control.
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
    <string>$BIN_DIR/booty-call-control</string>
    <string>--config</string>
    <string>$CFG</string>
  </array>
  <key>WorkingDirectory</key><string>$(dirname "$STATIC_DIR")</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/sbin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$LOG_DIR/control.out</string>
  <key>StandardErrorPath</key><string>$LOG_DIR/control.err</string>
</dict>
</plist>
EOF

launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl print "$DOMAIN/$LABEL" | head -5
echo "installed $LABEL (config: $CFG, logs: $LOG_DIR)"
