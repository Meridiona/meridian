#!/usr/bin/env bash
# meridian — set up the Python services layer for a new dev machine
#
# Run once after cloning:
#   bash scripts/setup-services.sh
#
# Pass --mlx to also install the MLX inference server extras (arm64 only):
#   bash scripts/setup-services.sh --mlx
#
# Safe to re-run. Only overwrites hermes config.yaml, never .env.
#
# NOTE: this creates services/.venv inside the repo — for your interactive
# dev work (running scripts, evals, etc. in the terminal). The launchd daemon
# uses a separate venv at ~/.meridian/mlx-server-venv (created automatically
# by install-mlx-server-daemon.sh) to avoid macOS 15 TCC/EPERM restrictions.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVICES_DIR="${REPO_ROOT}/services"

# MLX needs a native arm64 CPython: mlx ships arm64-only wheels (Metal), and a
# Rosetta/Intel python3 from PATH either fails the resolve or leaves a
# mixed-architecture venv. So on Apple Silicon the venv is built from a
# uv-MANAGED interpreter pinned by full build key — never the PATH python.
MERIDIAN_PY_BUILD="cpython-3.11-macos-aarch64-none"

# Hardware probe: hw.optional.arm64 stays 1 in a Rosetta (x86_64) shell on
# Apple Silicon, where uname -m reports the process arch and lies.
APPLE_SILICON=0
if [[ "$(uname -s)" == "Darwin" \
      && "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" == "1" ]]; then
    APPLE_SILICON=1
fi

# Record MLX capability for `meridian doctor`: on unsupported hardware it
# reports "unsupported" with the degradation story instead of an unfixable
# "server down → meridian start" loop.
mkdir -p "${HOME}/.meridian"
CAPABILITIES_FILE="${HOME}/.meridian/capabilities"
_mlx_cap="supported"
if [[ "$(uname -s)" == "Darwin" && "${APPLE_SILICON}" -eq 0 ]]; then
    _mlx_cap="unsupported_intel_hardware"
fi
{ grep -v '^mlx=' "${CAPABILITIES_FILE}" 2>/dev/null || true; echo "mlx=${_mlx_cap}"; } \
    > "${CAPABILITIES_FILE}.tmp" && mv "${CAPABILITIES_FILE}.tmp" "${CAPABILITIES_FILE}"

USE_MLX=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --mlx) USE_MLX=1 ;;
        *) echo "✗ Unknown flag: $1" >&2; exit 1 ;;
    esac
    shift
done

echo "=== meridian services setup ==="
echo ""

# 1. Find or install uv (Astral's Rust-based Python package manager).
UV_BIN=""
for _uv_candidate in "${HOME}/.local/bin/uv" /opt/homebrew/bin/uv /usr/local/bin/uv; do
    if [[ -x "${_uv_candidate}" ]]; then UV_BIN="${_uv_candidate}"; break; fi
done
[[ -z "${UV_BIN}" ]] && UV_BIN="$(command -v uv 2>/dev/null || true)"
if [[ -z "${UV_BIN}" ]]; then
    echo "Installing uv..."
    curl -LsSf https://astral.sh/uv/install.sh | sh
    UV_BIN="${HOME}/.local/bin/uv"
    echo "  ✓ uv installed"
fi
echo "  ✓ uv $("${UV_BIN}" --version | awk '{print $2}')"

# 2. Install Python services into services/.venv (standard uv default — inside repo).
#    This venv is for interactive dev use (terminal, evals, scripts).
#    The launchd daemon uses ~/.meridian/mlx-server-venv instead (see above).
if [[ "${USE_MLX}" -eq 1 ]]; then
    if [[ "${APPLE_SILICON}" -ne 1 ]]; then
        echo "✗ --mlx requires Apple Silicon hardware (mlx has no x86_64 wheels)" >&2
        echo "  Summaries still work via the agent CLIs; re-run without --mlx." >&2
        exit 1
    fi
    echo "Installing Python services (mlx + pm_worklog_update) to services/.venv..."
    "${UV_BIN}" python install "${MERIDIAN_PY_BUILD}"
    "${UV_BIN}" sync --project "${SERVICES_DIR}" \
        --extra mlx --extra pm_worklog_update \
        --python "${MERIDIAN_PY_BUILD}" --python-preference only-managed
    echo "  ✓ mlx + pm_worklog_update extras installed"

    if ! "${UV_BIN}" run --no-sync --project "${SERVICES_DIR}" \
            meridian-server --help >/dev/null 2>&1; then
        echo "  ✗ meridian-server not available after install" >&2
        exit 1
    fi
    echo "  ✓ meridian-server script available"
else
    echo "Installing Python services to services/.venv..."
    if [[ "${APPLE_SILICON}" -eq 1 ]]; then
        "${UV_BIN}" python install "${MERIDIAN_PY_BUILD}"
        "${UV_BIN}" sync --project "${SERVICES_DIR}" \
            --python "${MERIDIAN_PY_BUILD}" --python-preference only-managed
    else
        "${UV_BIN}" sync --project "${SERVICES_DIR}" --python 3.11
    fi
    echo "  ✓ core dependencies installed"

    if [[ "${APPLE_SILICON}" -eq 1 ]]; then
        echo "Apple Silicon detected — installing mlx-lm for local inference..."
        "${UV_BIN}" pip install --python "${SERVICES_DIR}/.venv/bin/python" "mlx-lm>=0.22,<1"
        echo "  ✓ mlx-lm installed"

        _macos_major="$(sw_vers -productVersion 2>/dev/null | cut -d. -f1)"
        if [[ "${_macos_major:-0}" -ge 26 ]]; then
            if [[ ! -d /Applications/Xcode.app ]]; then
                echo "  ⚠ Xcode.app not found — skipping apple-fm-sdk (requires full Xcode to build)."
                echo "    Install Xcode 26+ from the App Store, open it once to accept the license, then re-run."
            else
                echo "macOS ${_macos_major} detected — installing apple-fm-sdk for Apple Intelligence..."
                "${UV_BIN}" pip install --python "${SERVICES_DIR}/.venv/bin/python" apple-fm-sdk
                echo "  ✓ apple-fm-sdk installed"
            fi
        fi
    fi
fi

# 3. Hermes config setup (creates services/.hermes/config.yaml if absent).
echo "Configuring hermes..."
bash "${SERVICES_DIR}/scripts/setup-hermes.sh"

# 4. Final check — warn if API key is still the placeholder.
ROOT_ENV="${REPO_ROOT}/.env"
if grep -q "YOUR_API_KEY_HERE\|<your" "${ROOT_ENV}" 2>/dev/null; then
    echo ""
    echo "  ⚠  OLLAMA_API_KEY in <repo>/.env is still a placeholder."
    echo "     Edit it before running the daemon:"
    echo "       \$EDITOR ${ROOT_ENV}"
fi

echo ""
echo "=== setup complete ==="
echo ""
if [[ "${USE_MLX}" -eq 1 ]]; then
    echo "Next: install the MLX launchd agent, then start the Rust daemon"
    echo "  bash ${REPO_ROOT}/services/scripts/install-mlx-server-daemon.sh"
    echo "  cargo build --release && ./target/release/meridian"
else
    echo "Next: cargo build --release && ./target/release/meridian"
fi
