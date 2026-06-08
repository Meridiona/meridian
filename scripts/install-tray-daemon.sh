#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Install the Meridian Tray app as a launchd LaunchAgent.
# Follows the same pattern as install-daemon.sh.
#
#   bash scripts/install-tray-daemon.sh
set -euo pipefail

LABEL="com.meridiona.tray"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="${SCRIPT_DIR}/${LABEL}.plist"
LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

[[ -f "${TEMPLATE}" ]] || { echo "✗ plist template not found: ${TEMPLATE}" >&2; exit 1; }

# Locate the tray binary — always lives in the bundle's bin/ alongside meridian.
APP_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TRAY_BIN="${APP_ROOT}/bin/meridian-tray"

[[ -x "${TRAY_BIN}" ]] || {
    echo "✗ meridian-tray binary not found at ${TRAY_BIN}" >&2
    exit 1
}

mkdir -p "${HOME}/.meridian/logs" "${LAUNCH_AGENTS}"

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{TRAY_BIN}}|${TRAY_BIN}|g" \
    -e "s|{{HOME}}|${HOME}|g" \
    "${TEMPLATE}" > "${PLIST_DEST}"

plutil -lint "${PLIST_DEST}" >/dev/null || { echo "✗ plist failed validation" >&2; exit 1; }

echo "→ bootout ${LABEL} (if loaded)"
launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
_wait=0
while launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; do
    sleep 1; _wait=$((_wait+1))
    [[ "${_wait}" -ge 15 ]] && { echo "⚠ ${LABEL} still in domain after 15s — proceeding" >&2; break; }
done

echo "→ bootstrap ${LABEL}"
launchctl enable "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
launchctl bootstrap "${GUI_TARGET}" "${PLIST_DEST}"
launchctl enable "${GUI_TARGET}/${LABEL}"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}"

echo
echo "✓ tray app installed — it will appear in the menu bar shortly"
echo
echo "Useful follow-ups:"
echo "  launchctl print  ${GUI_TARGET}/${LABEL}"
echo "  tail -f ~/.meridian/logs/tray.log"
echo "  tail -f ~/.meridian/logs/tray-error.log"
echo "  ${SCRIPT_DIR}/uninstall-tray-daemon.sh"
