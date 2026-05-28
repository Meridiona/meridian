#!/usr/bin/env bash
# Remove the ax-sidecar LaunchAgent.

set -euo pipefail

LABEL="com.meridiona.ax-sidecar"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

if launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; then
    echo "→ bootout ${LABEL}"
    launchctl bootout "${GUI_TARGET}" "${PLIST}" || true
fi

if [[ -f "${PLIST}" ]]; then
    rm -f "${PLIST}"
    echo "✓ removed ${PLIST}"
fi

echo "✓ ax-sidecar uninstalled"
