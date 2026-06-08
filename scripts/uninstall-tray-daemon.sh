#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Remove the Meridian Tray launchd agent.
set -euo pipefail

LABEL="com.meridiona.tray"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

echo "→ stopping and unloading ${LABEL}"
launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true

if [[ -f "${PLIST}" ]]; then
    rm -f "${PLIST}"
    echo "✓ ${PLIST} removed"
fi

echo "✓ tray agent uninstalled"
