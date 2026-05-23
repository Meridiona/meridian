#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Install the meridian Rust daemon as a launchd LaunchAgent under the
# current user. The daemon runs continuously, polling screenpipe every
# POLL_INTERVAL_SECS seconds (default 60).
#
#   ./scripts/install-daemon.sh
#
# Re-running this script is safe — it bootouts the existing agent first,
# rewrites the plist with current paths, and reloads it.
#
# Uninstall:
#   ./scripts/uninstall-daemon.sh
#   Or manually:
#     launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.daemon.plist
#     rm ~/Library/LaunchAgents/com.meridiona.daemon.plist

set -euo pipefail

LABEL="com.meridiona.daemon"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEMPLATE="${SCRIPT_DIR}/${LABEL}.plist"

LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"

GUI_TARGET="gui/$(id -u)"

if [[ ! -f "${TEMPLATE}" ]]; then
    echo "✗ template not found: ${TEMPLATE}" >&2
    exit 1
fi

# Locate the daemon binary — prefer /usr/local/bin, fall back to ~/.local/bin.
DAEMON_BIN=""
if [[ -x "/usr/local/bin/meridian-daemon" ]]; then
    DAEMON_BIN="/usr/local/bin/meridian-daemon"
elif [[ -x "${HOME}/.local/bin/meridian-daemon" ]]; then
    DAEMON_BIN="${HOME}/.local/bin/meridian-daemon"
else
    echo "✗ meridian-daemon binary not found at /usr/local/bin/meridian-daemon or ~/.local/bin/meridian-daemon — run ./install.sh first" >&2
    exit 1
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${LAUNCH_AGENTS}"

# Read MERIDIAN_OO_AUTH and MERIDIAN_OTLP_ENDPOINT from ~/.meridian/.env
# (optional — OTLP export is silently disabled when unset).
ENV_FILE="${HOME}/.meridian/.env"
MERIDIAN_OO_AUTH=""
MERIDIAN_OTLP_ENDPOINT=""
if [[ -f "${ENV_FILE}" ]]; then
    MERIDIAN_OO_AUTH="$(grep -E '^MERIDIAN_OO_AUTH=' "${ENV_FILE}" | cut -d= -f2- | tr -d '[:space:]')" || true
    MERIDIAN_OTLP_ENDPOINT="$(grep -E '^MERIDIAN_OTLP_ENDPOINT=' "${ENV_FILE}" | cut -d= -f2- | tr -d '[:space:]')" || true
fi
if [[ -z "${MERIDIAN_OO_AUTH}" ]]; then
    echo "  ⚠ MERIDIAN_OO_AUTH not set in ~/.meridian/.env — OTLP export will be disabled"
fi

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{REPO_ROOT}}|${REPO_ROOT}|g" \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{DAEMON_BIN}}|${DAEMON_BIN}|g" \
    -e "s|{{MERIDIAN_OO_AUTH}}|${MERIDIAN_OO_AUTH}|g" \
    -e "s|{{MERIDIAN_OTLP_ENDPOINT}}|${MERIDIAN_OTLP_ENDPOINT}|g" \
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
echo "✓ daemon installed and started"
echo
echo "Useful follow-ups:"
echo "  launchctl print  ${GUI_TARGET}/${LABEL}       # status"
echo "  tail -f ~/.meridian/logs/daemon.log            # live stdout"
echo "  tail -f ~/.meridian/logs/daemon-error.log      # live stderr"
echo "  ${SCRIPT_DIR}/uninstall-daemon.sh              # remove"

# Note: make this script executable after cloning:
#   chmod +x scripts/install-daemon.sh
