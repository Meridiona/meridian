#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Stop and remove the OpenObserve launchd agent.
# Data at ~/.openobserve/data/ is left intact.

set -euo pipefail

LABEL="com.meridiona.openobserve"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

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
