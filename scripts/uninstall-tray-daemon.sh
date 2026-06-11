#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
