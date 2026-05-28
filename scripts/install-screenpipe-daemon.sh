#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Install screenpipe as a launchd LaunchAgent under the current user.
# screenpipe runs continuously, recording screen and audio on its default
# port 3030 with data stored in ~/.screenpipe.
#
#   ./scripts/install-screenpipe-daemon.sh
#
# Re-running this script is safe — it bootouts the existing agent first,
# rewrites the plist with current paths, and reloads it.
#
# Uninstall:
#   ./scripts/uninstall-screenpipe-daemon.sh
#   Or manually:
#     launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.screenpipe.plist
#     rm ~/Library/LaunchAgents/com.meridiona.screenpipe.plist

set -euo pipefail

LABEL="com.meridiona.screenpipe"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="${SCRIPT_DIR}/com.meridiona.screenpipe.plist"

LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"

GUI_TARGET="gui/$(id -u)"

if [[ ! -f "${TEMPLATE}" ]]; then
    echo "✗ template not found: ${TEMPLATE}" >&2
    exit 1
fi

# Locate the screenpipe binary.
SCREENPIPE_BIN="$(command -v screenpipe)" || true
if [[ -z "${SCREENPIPE_BIN}" ]]; then
    echo "✗ screenpipe binary not found in PATH — install with: npm install -g screenpipe" >&2
    exit 1
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${LAUNCH_AGENTS}"

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{SCREENPIPE_BIN}}|${SCREENPIPE_BIN}|g" \
    "${TEMPLATE}" > "${PLIST_DEST}"

# Validate the plist before loading.
if ! plutil -lint "${PLIST_DEST}" >/dev/null; then
    echo "✗ plist failed validation" >&2
    exit 1
fi

# Bootout if previously loaded (idempotent — ignore if not loaded).
if launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; then
    echo "→ bootout existing ${LABEL}"
    launchctl bootout "${GUI_TARGET}" "${PLIST_DEST}" || true
fi

echo "→ bootstrap ${LABEL}"
launchctl bootstrap "${GUI_TARGET}" "${PLIST_DEST}"
launchctl enable "${GUI_TARGET}/${LABEL}"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}"

echo
echo "✓ screenpipe installed and started"
echo
echo "Useful follow-ups:"
echo "  launchctl print  ${GUI_TARGET}/${LABEL}              # status"
echo "  tail -f ~/.meridian/logs/screenpipe.log               # live stdout"
echo "  tail -f ~/.meridian/logs/screenpipe-error.log         # live stderr"
echo "  ${SCRIPT_DIR}/uninstall-screenpipe-daemon.sh          # remove"

# Note: make this script executable after cloning:
#   chmod +x scripts/install-screenpipe-daemon.sh
