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

VENV="${SERVICES_DIR}/.venv"
VENV_CFG="${VENV}/pyvenv.cfg"
if [[ ! -f "${VENV_CFG}" ]]; then
    echo "✗ venv not found at ${VENV}" >&2
    echo "  Run:  bash scripts/setup-services.sh --mlx" >&2
    exit 1
fi

# Resolve the base Python from pyvenv.cfg so we invoke it directly rather
# than through the venv wrapper. The wrapper shebang causes Python to read
# pyvenv.cfg at startup, which EPERM-fails when launchd launches the process
# on macOS 15 (launchd inherits no TCC Documents permission).
BASE_PYTHON=$(grep '^executable' "${VENV_CFG}" | awk '{print $3}')
if [[ ! -x "${BASE_PYTHON}" ]]; then
    echo "✗ base Python not found at ${BASE_PYTHON} (from ${VENV_CFG})" >&2
    exit 1
fi

# venv site-packages directory (PYTHONPATH replaces venv activation).
SITE_PACKAGES=$(ls -d "${VENV}/lib/python"*/site-packages 2>/dev/null | head -1)
if [[ -z "${SITE_PACKAGES}" ]]; then
    echo "✗ site-packages not found under ${VENV}/lib/" >&2
    exit 1
fi

# OTel credentials — read from the repo-root .env if set there; fall back to
# empty string (telemetry silently disabled when both are unset).
MERIDIAN_OO_AUTH=""
MERIDIAN_OTLP_ENDPOINT=""
if [[ -f "${REPO_ROOT}/.env" ]]; then
    MERIDIAN_OO_AUTH=$(grep -E '^MERIDIAN_OO_AUTH=' "${REPO_ROOT}/.env" | tail -1 | cut -d= -f2-)
    MERIDIAN_OTLP_ENDPOINT=$(grep -E '^MERIDIAN_OTLP_ENDPOINT=' "${REPO_ROOT}/.env" | tail -1 | cut -d= -f2-)
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${LAUNCH_AGENTS}"

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{REPO_ROOT}}|${REPO_ROOT}|g" \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{MLX_SERVER_PORT}}|${MLX_SERVER_PORT}|g" \
    -e "s|{{BASE_PYTHON}}|${BASE_PYTHON}|g" \
    -e "s|{{SITE_PACKAGES}}|${SITE_PACKAGES}|g" \
    -e "s|{{MERIDIAN_OO_AUTH}}|${MERIDIAN_OO_AUTH}|g" \
    -e "s|{{MERIDIAN_OTLP_ENDPOINT}}|${MERIDIAN_OTLP_ENDPOINT}|g" \
    "${TEMPLATE}" > "${PLIST_DEST}"

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

echo
echo "→ installing Claude Code coding-agent SessionEnd hook …"
bash "${SCRIPT_DIR}/install-claude-hook.sh"
