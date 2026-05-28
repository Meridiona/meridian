#!/usr/bin/env bash
# Install the meridian jira-updater as a launchd LaunchAgent under the
# current user. Slot-based schedule is managed by the daemon itself
# (RunAtLoad false — it does not fire on every login).
#
#   ./services/scripts/install-jira-updater-daemon.sh
#
# Re-running this script is safe — it bootouts the existing agent first,
# rewrites the plist with current paths, and reloads it.
#
# Uninstall:
#   ./services/scripts/uninstall-jira-updater-daemon.sh
#   Or manually:
#     launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.jira-updater.plist
#     rm ~/Library/LaunchAgents/com.meridiona.jira-updater.plist

set -euo pipefail

LABEL="com.meridiona.jira-updater"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICES_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SERVICES_DIR}/.." && pwd)"
TEMPLATE="${SCRIPT_DIR}/${LABEL}.plist"

LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"

GUI_TARGET="gui/$(id -u)"

if [[ ! -f "${TEMPLATE}" ]]; then
    echo "✗ template not found: ${TEMPLATE}" >&2
    exit 1
fi

VENV_PYTHON="${SERVICES_DIR}/.venv/bin/python"
if [[ ! -x "${VENV_PYTHON}" ]]; then
    echo "✗ python venv not found at ${VENV_PYTHON}" >&2
    echo "  Run:  cd services && python3.11 -m venv .venv && source .venv/bin/activate && pip install -e ." >&2
    exit 1
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${LAUNCH_AGENTS}"

# Read MERIDIAN_OO_AUTH from services/.env (required — observability.py
# initialises before dotenv is loaded, so launchd must inject it directly).
ENV_FILE="${SERVICES_DIR}/.env"
MERIDIAN_OO_AUTH=""
if [[ -f "${ENV_FILE}" ]]; then
    MERIDIAN_OO_AUTH="$(grep -E '^MERIDIAN_OO_AUTH=' "${ENV_FILE}" | cut -d= -f2- | tr -d '[:space:]')"
fi
if [[ -z "${MERIDIAN_OO_AUTH}" ]]; then
    echo "✗ MERIDIAN_OO_AUTH not found in ${ENV_FILE}" >&2
    echo "  Add:  MERIDIAN_OO_AUTH=<base64-encoded user:password>" >&2
    exit 1
fi

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{REPO_ROOT}}|${REPO_ROOT}|g" \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{MERIDIAN_OO_AUTH}}|${MERIDIAN_OO_AUTH}|g" \
    "${TEMPLATE}" > "${PLIST_DEST}"

# Validate the plist before loading.
if ! plutil -lint "${PLIST_DEST}" >/dev/null; then
    echo "✗ plist failed validation" >&2
    exit 1
fi

echo "→ bootout ${LABEL} (if loaded)"
launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
sleep 1

echo "→ bootstrap ${LABEL}"
launchctl enable "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
launchctl bootstrap "${GUI_TARGET}" "${PLIST_DEST}"
launchctl enable "${GUI_TARGET}/${LABEL}"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}"

echo
echo "✓ jira-updater installed and started"
echo
echo "Useful follow-ups:"
echo "  launchctl print  ${GUI_TARGET}/${LABEL}           # status"
echo "  tail -f ~/.meridian/logs/jira-updater.log         # live logs"
echo "  ${SCRIPT_DIR}/uninstall-jira-updater-daemon.sh    # remove"
echo
echo "One-shot trigger without restarting:"
echo "  cd services && python -m agents.jira_updater_daemon --trigger-now"

# Note: make this script executable after cloning:
#   chmod +x services/scripts/install-jira-updater-daemon.sh
