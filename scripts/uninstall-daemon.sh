#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Stop and remove the meridian Rust daemon launchd agent.
set -euo pipefail

LABEL="com.meridiona.daemon"
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

# Legacy agents: the Jira updater and coding-agent indexer used to run as their
# own standalone Python launchd daemons. They were ported into this Rust daemon
# (the Python modules were deleted), but a pre-port install leaves the old
# KeepAlive agents behind — they then crash-loop every ~10s against the missing
# modules and spam ~MB/min into their logs. Boot out + disable + remove them so
# an upgrade-then-uninstall leaves nothing orphaned.
for legacy in com.meridiona.jira-updater com.meridiona.coding-agent-indexer; do
    legacy_plist="${HOME}/Library/LaunchAgents/${legacy}.plist"
    if launchctl print "${GUI_TARGET}/${legacy}" >/dev/null 2>&1; then
        launchctl disable "${GUI_TARGET}/${legacy}" 2>/dev/null || true
        launchctl bootout "${GUI_TARGET}/${legacy}" 2>/dev/null || true
        echo "✓ removed legacy agent ${legacy}"
    fi
    rm -f "${legacy_plist}"
done
