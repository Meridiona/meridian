#!/usr/bin/env bash
# Install the ax-sidecar as a launchd LaunchAgent.
# It polls VS Code's a11y tree every 3 s and writes terminal content into
# screenpipe's frames table so it appears in accessibility searches.
#
#   ./services/scripts/install-ax-sidecar.sh
#
# Re-running is safe — bootouts the old agent, rewrites the plist, reloads.
# Requires the swift/ax_terminal binary to exist (compile with swiftc if not).
#
# Uninstall:
#   ./services/scripts/uninstall-ax-sidecar.sh

set -euo pipefail

LABEL="com.meridiona.ax-sidecar"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICES_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SERVICES_DIR}/.." && pwd)"
TEMPLATE="${SCRIPT_DIR}/${LABEL}.plist"
LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

# Ensure ax_terminal binary exists
AX_TERMINAL="${REPO_ROOT}/swift/ax_terminal"
if [[ ! -x "${AX_TERMINAL}" ]]; then
    echo "→ compiling swift/ax_terminal..."
    swiftc "${REPO_ROOT}/swift/ax_terminal.swift" -o "${AX_TERMINAL}" -O
    echo "✓ compiled ${AX_TERMINAL}"
fi

# Prefer venv python, fall back to system python3
VENV_PYTHON="${SERVICES_DIR}/.venv/bin/python"
if [[ -x "${VENV_PYTHON}" ]]; then
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
echo "✓ ax-sidecar installed and started"
echo
echo "Useful follow-ups:"
echo "  launchctl print  ${GUI_TARGET}/${LABEL}      # status"
echo "  tail -f ~/.meridian/logs/ax-sidecar.log      # live logs"
echo "  sqlite3 ~/.screenpipe/db.sqlite \\"
echo "    \"SELECT window_name, length(accessibility_text), timestamp \\"
echo "       FROM frames WHERE device_name='ax-sidecar' ORDER BY timestamp DESC LIMIT 5;\""
echo "  ${SCRIPT_DIR}/uninstall-ax-sidecar.sh        # remove"
