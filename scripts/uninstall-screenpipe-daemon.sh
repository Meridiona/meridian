#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Stop and remove the screenpipe launchd agent.
set -euo pipefail

LABEL="com.meridiona.screenpipe"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

if [[ -f "${PLIST}" ]]; then
    if launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; then
        echo "→ bootout ${LABEL}"
        launchctl bootout "${GUI_TARGET}" "${PLIST}" || true
    fi
    rm -f "${PLIST}"
    echo "✓ removed ${PLIST}"
else
    echo "(not installed) ${PLIST}"
fi
