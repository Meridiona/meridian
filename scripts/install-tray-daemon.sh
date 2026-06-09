#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Install the tray app as a launchd agent to start on login

set -euo pipefail
IFS=$'\n\t'

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Support both dev mode (target/release) and bundle mode (bin/)
TRAY_BIN=""
if [[ -x "${REPO_ROOT}/target/release/meridian-tray" ]]; then
    # Dev mode: built locally to target/release
    TRAY_BIN="${REPO_ROOT}/target/release/meridian-tray"
elif [[ -x "${REPO_ROOT}/bin/meridian-tray" ]]; then
    # Bundle mode: installed to ~/.meridian/app/bin
    TRAY_BIN="${REPO_ROOT}/bin/meridian-tray"
else
    echo "✗ meridian-tray binary not found" >&2
    echo "  Dev: build it with: cd tray && npm run tauri build" >&2
    echo "  Bundle: already included via: meridian update" >&2
    exit 1
fi
PLIST="${HOME}/Library/LaunchAgents/com.meridiona.tray.plist"

mkdir -p "$(dirname "${PLIST}")"

cat > "${PLIST}" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.meridiona.tray</string>
    <key>Program</key>
    <string>TRAY_BIN_PLACEHOLDER</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>LOGS_PLACEHOLDER/tray.log</string>
    <key>StandardErrorPath</key>
    <string>LOGS_PLACEHOLDER/tray.log</string>
</dict>
</plist>
EOF

# Replace placeholders
sed -i '' "s|TRAY_BIN_PLACEHOLDER|${TRAY_BIN}|g" "${PLIST}"
sed -i '' "s|LOGS_PLACEHOLDER|${HOME}/.meridian/logs|g" "${PLIST}"

mkdir -p "${HOME}/.meridian/logs"
chmod 644 "${PLIST}"

GUI_TARGET="gui/$(id -u)"
LABEL="com.meridiona.tray"

launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
# Wait for bootout to complete before bootstrapping
while launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; do
    sleep 0.2
done
launchctl enable "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
launchctl bootstrap "${GUI_TARGET}" "${PLIST}"
launchctl enable "${GUI_TARGET}/${LABEL}"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}"

echo "✓ Tray app installed and started (com.meridiona.tray)"
echo "  launchctl print ${GUI_TARGET}/${LABEL}    # status"
echo "  tail -f ~/.meridian/logs/tray.log         # logs"
