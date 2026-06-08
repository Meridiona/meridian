#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Install the tray app as a launchd agent to start on login

set -euo pipefail
IFS=$'\n\t'

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRAY_BIN="${REPO_ROOT}/tray/src-tauri/target/release/meridian-tray"
PLIST="${HOME}/Library/LaunchAgents/com.meridiona.tray.plist"

if [[ ! -x "${TRAY_BIN}" ]]; then
    echo "✗ meridian-tray binary not found at ${TRAY_BIN}" >&2
    echo "  Build it first: cd tray && npm run build" >&2
    exit 1
fi

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

launchctl load "${PLIST}" 2>/dev/null || launchctl load -F "${PLIST}"

echo "✓ Tray app registered as com.meridiona.tray — will start on next login"
