#!/usr/bin/env bash
# Stop and remove the meridian tagger-daemon launchd agent.
set -euo pipefail

LABEL="com.meridiona.tagger-daemon"
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
