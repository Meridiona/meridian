#!/usr/bin/env bash
# Install the coding-agent-indexer as a launchd LaunchAgent.
#
# Polls ~/.claude/projects/ and ~/.codex/sessions/ every 10 min, finds
# ended coding-agent sessions (idle ≥ 5 min, not the active file in
# their project dir), and registers each as one app_sessions row in
# meridian.db (with the full transcript in session_text).
#
# Real-time path: the Claude Code SessionEnd hook (install separately)
# calls `python -m coding_agent_indexer.hook` on every clean
# session end. The poll loop here is the fallback for crashes,
# force-kills, Codex sessions, and macOS-sleep gaps.
#
#   ./services/scripts/install-coding-agent-indexer.sh
#
# Re-running is safe — bootouts the old agent, rewrites the plist, reloads.
#
# Uninstall:
#   ./services/scripts/uninstall-coding-agent-indexer.sh

set -euo pipefail

LABEL="com.meridiona.coding-agent-indexer"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICES_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SERVICES_DIR}/.." && pwd)"
TEMPLATE="${SCRIPT_DIR}/${LABEL}.plist"
LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

# Prefer the project's venv313 (where agno + sqlite3 deps live), then
# any .venv at the services root, then system python3.
VENV313_PYTHON="${SERVICES_DIR}/.venv313/bin/python"
VENV_PYTHON="${SERVICES_DIR}/.venv/bin/python"
if [[ -x "${VENV313_PYTHON}" ]]; then
    PYTHON="${VENV313_PYTHON}"
elif [[ -x "${VENV_PYTHON}" ]]; then
    PYTHON="${VENV_PYTHON}"
else
    PYTHON="$(command -v python3)"
fi
echo "→ using python: ${PYTHON}"

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${LAUNCH_AGENTS}"

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{REPO_ROOT}}|${REPO_ROOT}|g" \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{PYTHON}}|${PYTHON}|g" \
    "${TEMPLATE}" > "${PLIST_DEST}"

if ! plutil -lint "${PLIST_DEST}" >/dev/null; then
    echo "✗ plist failed validation" >&2
    exit 1
fi

if launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; then
    echo "→ bootout existing ${LABEL}"
    launchctl bootout "${GUI_TARGET}" "${PLIST_DEST}" || true
fi

echo "→ bootstrap ${LABEL}"
launchctl bootstrap "${GUI_TARGET}" "${PLIST_DEST}"
launchctl enable "${GUI_TARGET}/${LABEL}"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}"

echo
echo "✓ coding-agent-indexer installed and started"
echo
echo "Useful follow-ups:"
echo "  launchctl print  ${GUI_TARGET}/${LABEL}                     # status"
echo "  tail -f ~/.meridian/logs/coding-agent-indexer.log           # live logs"
echo "  sqlite3 ~/.meridian/meridian.db \\"
echo "    \"SELECT id, started_at, duration_s, frame_count, claude_session_uuid \\"
echo "       FROM app_sessions WHERE claude_session_uuid IS NOT NULL \\"
echo "       ORDER BY id DESC LIMIT 5;\""
echo "  ${SCRIPT_DIR}/uninstall-coding-agent-indexer.sh             # remove"
