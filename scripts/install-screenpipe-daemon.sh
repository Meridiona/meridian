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

# Always attempt bootout by label — launchctl print can return non-zero even when
# the label is still registered (e.g. service stopped but domain entry exists),
# causing bootstrap to fail with EIO. Label-based bootout is also more reliable
# when the plist content changed since the last load.
echo "→ bootout ${LABEL} (if loaded)"
launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
# bootout is async — wait until the domain entry actually clears before
# bootstrapping, otherwise launchctl bootstrap can fail with EIO (errno 5).
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
echo "✓ screenpipe installed and started"
echo
echo "Useful follow-ups:"
echo "  launchctl print  ${GUI_TARGET}/${LABEL}              # status"
echo "  tail -f ~/.meridian/logs/screenpipe.log               # live stdout"
echo "  tail -f ~/.meridian/logs/screenpipe-error.log         # live stderr"
echo "  ${SCRIPT_DIR}/uninstall-screenpipe-daemon.sh          # remove"

# Note: make this script executable after cloning:
#   chmod +x scripts/install-screenpipe-daemon.sh
