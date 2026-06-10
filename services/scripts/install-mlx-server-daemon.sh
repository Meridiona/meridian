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

# MLX requires Apple Silicon — mlx ships arm64-only wheels (Metal). Probe the
# HARDWARE via sysctl: hw.optional.arm64 stays 1 in a Rosetta (x86_64) shell,
# where uname -m would lie. Installing the agent anyway would just crash-loop.
if [[ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" != "1" ]]; then
    echo "✗ MLX requires Apple Silicon — not installing the MLX server agent on this Mac." >&2
    echo "  Coding-agent summaries still work via the agent CLIs (claude/codex)." >&2
    exit 1
fi

# Find the uv binary.
UV_BIN=""
for _uv_candidate in "${HOME}/.local/bin/uv" /opt/homebrew/bin/uv /usr/local/bin/uv; do
    if [[ -x "${_uv_candidate}" ]]; then UV_BIN="${_uv_candidate}"; break; fi
done
[[ -z "${UV_BIN}" ]] && UV_BIN="$(command -v uv 2>/dev/null || true)"
if [[ -z "${UV_BIN}" ]]; then
    echo "✗ uv not found. Run:  bash scripts/setup-services.sh --mlx" >&2
    exit 1
fi

# The venv always lives at services/.venv — inside the repo for dev installs,
# inside ~/.meridian/app/services/ for npm installs (managed by install-from-bundle.sh).
SERVICES_VENV="${SERVICES_DIR}/.venv"

if [[ ! -d "${SERVICES_VENV}" ]]; then
    if [[ "${SERVICES_DIR}" == "${HOME}/.meridian/"* ]]; then
        echo "✗ venv not found at ${SERVICES_VENV}" >&2
        echo "  npm install appears incomplete — reinstall via: meridian update" >&2
        exit 1
    fi
    echo "✗ venv not found at ${SERVICES_VENV}" >&2
    echo "  Run:  bash scripts/setup-services.sh --mlx" >&2
    exit 1
fi

# Refuse a mixed-architecture venv (built by a Rosetta/Intel python3 before the
# interpreter was pinned to a uv-managed arm64 build) — the server would only
# crash-loop on native-extension imports.
_venv_arch="$("${SERVICES_VENV}/bin/python" -c 'import platform; print(platform.machine())' 2>/dev/null || true)"
if [[ "${_venv_arch}" != "arm64" ]]; then
    echo "✗ venv python at ${SERVICES_VENV} is '${_venv_arch:-unknown}', need arm64 (mixed-architecture venv)." >&2
    if [[ "${SERVICES_DIR}" == "${HOME}/.meridian/"* ]]; then
        echo "  Rebuild it:  meridian update" >&2
    else
        echo "  Rebuild it:  rm -rf ${SERVICES_VENV} && bash scripts/setup-services.sh --mlx" >&2
    fi
    exit 1
fi

# OTel credentials — read from the repo-root .env if set there; fall back to
# empty string (telemetry silently disabled when both are unset).
MERIDIAN_OO_AUTH=""
MERIDIAN_OTLP_ENDPOINT=""
if [[ -f "${REPO_ROOT}/.env" ]]; then
    # `|| true`: grep exits non-zero when the key is absent, which under
    # `set -o pipefail` + `set -e` would abort the whole installer. These vars
    # are optional (telemetry off when unset), so never let a missing key fail.
    MERIDIAN_OO_AUTH=$(grep -E '^MERIDIAN_OO_AUTH=' "${REPO_ROOT}/.env" | tail -1 | cut -d= -f2- || true)
    MERIDIAN_OTLP_ENDPOINT=$(grep -E '^MERIDIAN_OTLP_ENDPOINT=' "${REPO_ROOT}/.env" | tail -1 | cut -d= -f2- || true)
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${LAUNCH_AGENTS}"

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{REPO_ROOT}}|${REPO_ROOT}|g" \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{MLX_SERVER_PORT}}|${MLX_SERVER_PORT}|g" \
    -e "s|{{UV_BIN}}|${UV_BIN}|g" \
    -e "s|{{SERVICES_VENV}}|${SERVICES_VENV}|g" \
    -e "s|{{MERIDIAN_OO_AUTH}}|${MERIDIAN_OO_AUTH}|g" \
    -e "s|{{MERIDIAN_OTLP_ENDPOINT}}|${MERIDIAN_OTLP_ENDPOINT}|g" \
    "${TEMPLATE}" > "${PLIST_DEST}"

if ! plutil -lint "${PLIST_DEST}" >/dev/null; then
    echo "✗ plist failed validation" >&2
    exit 1
fi

echo "→ bootout ${LABEL} (if loaded)"
launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
# bootout is async. A fixed sleep is unreliable — when the prior MLX process
# still holds the ~9 GB model in memory it takes seconds to exit, and
# bootstrapping before the domain entry is gone fails with EIO (errno 5).
# Wait until launchd confirms the service is actually gone.
_bootout_wait=0
while launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; do
    sleep 1
    _bootout_wait=$(( _bootout_wait + 1 ))
    if [[ "${_bootout_wait}" -ge 20 ]]; then
        echo "⚠ ${LABEL} still in launchd domain after 20s — proceeding anyway" >&2
        break
    fi
done

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
