#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Stop and remove the OpenObserve launchd agent.
# Data at ~/.openobserve/data/ is left intact.

set -euo pipefail

LABEL="com.meridiona.openobserve"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

# Remove legacy agent if still present.
_legacy="${HOME}/Library/LaunchAgents/ai.openobserve.plist"
if [[ -f "${_legacy}" ]]; then
    launchctl bootout "${GUI_TARGET}/ai.openobserve" 2>/dev/null || true
    rm -f "${_legacy}"
    echo "✓ legacy ai.openobserve agent removed"
fi

if [[ ! -f "${PLIST}" ]]; then
    echo "(${LABEL} not installed)"
    exit 0
fi

if launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; then
    echo "→ bootout ${LABEL}"
    launchctl bootout "${GUI_TARGET}" "${PLIST}" || true
fi

rm -f "${PLIST}"
echo "✓ ${LABEL} uninstalled"
echo "  (trace data at ~/.openobserve/data/ is preserved)"
