#!/usr/bin/env bash
# Install the meridian MLX inference server as a launchd LaunchAgent.
#
# The server loads Qwen3.5-9B once at startup and keeps it in memory.
# The Rust daemon connects to it via POST /classify_sessions on the
# configured port instead of cold-loading the model per session.
#
#   ./services/scripts/install-mlx-server-daemon.sh
#   ./services/scripts/install-mlx-server-daemon.sh --port 7824   # custom port
#
# Re-running is safe — bootouts the existing agent first, rewrites the plist,
# and reloads it.
#
# Uninstall:
#   ./services/scripts/uninstall-mlx-server-daemon.sh

set -euo pipefail

LABEL="com.meridiona.mlx-server"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICES_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SERVICES_DIR}/.." && pwd)"
TEMPLATE="${SCRIPT_DIR}/${LABEL}.plist"

LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"
GUI_TARGET="gui/$(id -u)"

# Default port — override with --port N
MLX_SERVER_PORT="7823"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --port) MLX_SERVER_PORT="$2"; shift 2 ;;
        *) echo "unknown flag: $1" >&2; exit 1 ;;
    esac
done

if [[ ! -f "${TEMPLATE}" ]]; then
    echo "✗ plist template not found: ${TEMPLATE}" >&2
    exit 1
fi

VENV_PYTHON="${SERVICES_DIR}/.venv313/bin/python3.13"
if [[ ! -x "${VENV_PYTHON}" ]]; then
    echo "✗ MLX venv not found at ${VENV_PYTHON}" >&2
    echo "  Run:  cd services && python3.13 -m venv .venv313 && .venv313/bin/pip install -r requirements-mlx.txt" >&2
    exit 1
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${LAUNCH_AGENTS}"

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{REPO_ROOT}}|${REPO_ROOT}|g" \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{MLX_SERVER_PORT}}|${MLX_SERVER_PORT}|g" \
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
echo "✓ MLX server installed and starting (model load takes ~5s)"
echo
echo "Useful follow-ups:"
echo "  tail -f ~/.meridian/logs/mlx-server.log              # watch model load + requests"
echo "  launchctl print ${GUI_TARGET}/${LABEL}  # status"
echo "  curl http://127.0.0.1:${MLX_SERVER_PORT}/health       # health check"
echo "  ${SCRIPT_DIR}/uninstall-mlx-server-daemon.sh          # remove"
echo
echo "Set CLASSIFIER_BACKEND=mlx and MLX_SERVER_PORT=${MLX_SERVER_PORT} in your .env"
echo "then restart the Rust daemon."
