#!/usr/bin/env bash
# Install the meridian pm-worklog-daemon as a launchd LaunchAgent under the
# current user. The daemon posts Jira worklogs on an hourly interval managed
# by the daemon itself (RunAtLoad true — fires immediately on login to catch up).
#
#   ./services/scripts/install-pm-worklog-daemon.sh
#
# Re-running this script is safe — it bootouts the existing agent first,
# rewrites the plist with current paths, and reloads it.
#
# Uninstall:
#   ./services/scripts/uninstall-pm-worklog-daemon.sh
#   Or manually:
#     launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.pm-worklog-daemon.plist
#     rm ~/Library/LaunchAgents/com.meridiona.pm-worklog-daemon.plist

set -euo pipefail

LABEL="com.meridiona.pm-worklog-daemon"
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

# Read Jira credentials from repo root .env then services/.env (first match wins).
# Injected directly into the plist so launchd passes them to the daemon.
JIRA_URL=""
JIRA_EMAIL=""
JIRA_API_TOKEN=""
MERIDIAN_OO_AUTH=""

for ENV_FILE in "${REPO_ROOT}/.env" "${SERVICES_DIR}/.env"; do
    if [[ -f "${ENV_FILE}" ]]; then
        [[ -z "${JIRA_URL}"        ]] && JIRA_URL="$(grep -E '^JIRA_URL=' "${ENV_FILE}" | cut -d= -f2- | tr -d '[:space:]')"
        [[ -z "${JIRA_EMAIL}"      ]] && JIRA_EMAIL="$(grep -E '^JIRA_EMAIL=' "${ENV_FILE}" | cut -d= -f2- | tr -d '[:space:]')"
        [[ -z "${JIRA_API_TOKEN}"  ]] && JIRA_API_TOKEN="$(grep -E '^JIRA_API_TOKEN=' "${ENV_FILE}" | cut -d= -f2- | tr -d '[:space:]')"
        [[ -z "${MERIDIAN_OO_AUTH}" ]] && MERIDIAN_OO_AUTH="$(grep -E '^MERIDIAN_OO_AUTH=' "${ENV_FILE}" | cut -d= -f2- | tr -d '[:space:]')"
    fi
done

if [[ -z "${JIRA_URL}" ]]; then
    echo "✗ JIRA_URL not found in ${ENV_FILE}" >&2
    echo "  Add:  JIRA_URL=https://<your-org>.atlassian.net" >&2
    exit 1
fi
if [[ -z "${JIRA_EMAIL}" ]]; then
    echo "✗ JIRA_EMAIL not found in ${ENV_FILE}" >&2
    echo "  Add:  JIRA_EMAIL=<your-jira-email>" >&2
    exit 1
fi
if [[ -z "${JIRA_API_TOKEN}" ]]; then
    echo "✗ JIRA_API_TOKEN not found in ${ENV_FILE}" >&2
    echo "  Add:  JIRA_API_TOKEN=<your-api-token>" >&2
    exit 1
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
    -e "s|{{JIRA_URL}}|${JIRA_URL}|g" \
    -e "s|{{JIRA_EMAIL}}|${JIRA_EMAIL}|g" \
    -e "s|{{JIRA_API_TOKEN}}|${JIRA_API_TOKEN}|g" \
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
echo "✓ pm-worklog-daemon installed and started"
echo
echo "Useful follow-ups:"
echo "  launchctl print  ${GUI_TARGET}/${LABEL}              # status"
echo "  tail -f ~/.meridian/logs/pm-worklog-daemon.log       # live logs"
echo "  ${SCRIPT_DIR}/uninstall-pm-worklog-daemon.sh         # remove"
echo
echo "One-shot trigger without restarting:"
echo "  cd services && python -m agents.pm_worklog_update.jira_worklog_daemon --trigger-now"

# Note: make this script executable after cloning:
#   chmod +x services/scripts/install-pm-worklog-daemon.sh
