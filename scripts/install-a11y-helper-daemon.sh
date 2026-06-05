#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Install meridian-a11y-helper as a launchd LaunchAgent under the current user.
# The helper enables macOS accessibility on Electron/Chromium apps so
# screenpipe can capture them — see scripts/a11y-helper/main.swift.
#
#   ./scripts/install-a11y-helper-daemon.sh
#
# Re-running is safe — it bootouts the existing agent, refreshes the binary
# and plist, and reloads.
#
# The helper binary is copied to ~/.meridian/bin/ ONLY when its bytes differ
# from the committed artifact. The copy is byte-identical to the committed
# binary, so macOS's Accessibility grant (keyed to the binary's code hash)
# survives meridian updates that don't touch the helper.

set -euo pipefail

LABEL="com.meridiona.a11y-helper"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="${SCRIPT_DIR}/com.meridiona.a11y-helper.plist"
SRC_BIN="${SCRIPT_DIR}/a11y-helper/meridian-a11y-helper"

DEST_DIR="${HOME}/.meridian/bin"
DEST_BIN="${DEST_DIR}/meridian-a11y-helper"

LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

if [[ ! -f "${TEMPLATE}" ]]; then
    echo "✗ template not found: ${TEMPLATE}" >&2
    exit 1
fi
if [[ ! -f "${SRC_BIN}" ]]; then
    echo "✗ helper binary not found: ${SRC_BIN} (build with scripts/a11y-helper/build.sh)" >&2
    exit 1
fi

mkdir -p "${DEST_DIR}" "${HOME}/.meridian/logs" "${LAUNCH_AGENTS}"

# Copy only when the bytes changed — an unnecessary rewrite is harmless for
# TCC (same hash) but skipping it keeps mtimes meaningful for debugging.
if ! cmp -s "${SRC_BIN}" "${DEST_BIN}" 2>/dev/null; then
    cp "${SRC_BIN}" "${DEST_BIN}"
    chmod +x "${DEST_BIN}"
    echo "→ installed helper binary: ${DEST_BIN}"
else
    echo "→ helper binary unchanged: ${DEST_BIN}"
fi

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{HELPER_BIN}}|${DEST_BIN}|g" \
    "${TEMPLATE}" > "${PLIST_DEST}"

if ! plutil -lint "${PLIST_DEST}" >/dev/null; then
    echo "✗ plist failed validation" >&2
    exit 1
fi

echo "→ bootout ${LABEL} (if loaded)"
launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
_bootout_wait=0
while launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; do
    sleep 1
    _bootout_wait=$(( _bootout_wait + 1 ))
    if [[ "${_bootout_wait}" -ge 15 ]]; then
        echo "⚠ ${LABEL} still in launchd domain after 15s — proceeding anyway" >&2
        break
    fi
done

echo "→ bootstrap ${LABEL}"
launchctl enable "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
launchctl bootstrap "${GUI_TARGET}" "${PLIST_DEST}"
launchctl enable "${GUI_TARGET}/${LABEL}"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}"

echo
echo "✓ a11y-helper installed and started"
echo
echo "⚠ One-time permission: add ${DEST_BIN}"
echo "  to System Settings → Privacy & Security → Accessibility and toggle it on."
echo "  Until granted, Electron apps (Claude, Codex, Slack, …) stay invisible to capture."
echo
echo "Useful follow-ups:"
echo "  launchctl print ${GUI_TARGET}/${LABEL}            # status"
echo "  tail -f ~/.meridian/logs/a11y-helper.log          # live log (shows trust state + pokes)"
