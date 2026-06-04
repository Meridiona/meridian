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

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVICES_DIR="${REPO_ROOT}/services"

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

# 1. Python venv — prefer python3.11 so hermes-agent installs cleanly
if command -v python3.11 >/dev/null 2>&1; then
    PYTHON_BIN="python3.11"
elif command -v python3 >/dev/null 2>&1; then
    PYTHON_BIN="python3"
else
    echo "✗ No python3 found on PATH"
    exit 1
fi
if [ ! -d "${SERVICES_DIR}/.venv" ]; then
    echo "Creating Python venv (using ${PYTHON_BIN})..."
    "${PYTHON_BIN}" -m venv "${SERVICES_DIR}/.venv"
    echo "  ✓ .venv created"
else
    echo "  ✓ .venv already exists"
fi

# 2. Install core dependencies
echo "Installing Python dependencies..."
"${SERVICES_DIR}/.venv/bin/pip" install --quiet -r "${SERVICES_DIR}/requirements.txt"
echo "  ✓ requirements installed"

# 2b. Install the services package itself in editable mode so that
#     `python -m agents.*` modules are importable by the launchd daemons.
echo "Installing meridian-agents package..."
"${SERVICES_DIR}/.venv/bin/pip" install --quiet -e "${SERVICES_DIR}"
echo "  ✓ meridian-agents installed"

# 3a. MLX inference server extras — only on Apple Silicon, only when --mlx is set.
#     Installs mlx-lm, outlines, fastapi, uvicorn and the meridian-server script.
if [[ "${USE_MLX}" -eq 1 ]]; then
    if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
        echo "✗ --mlx requires Apple Silicon (arm64 macOS)" >&2
        exit 1
    fi
    echo "Installing MLX inference + server extras (.[mlx])..."
    "${SERVICES_DIR}/.venv/bin/pip" install --quiet -e "${SERVICES_DIR}[mlx]"
    echo "  ✓ MLX extras installed"
    if ! "${SERVICES_DIR}/.venv/bin/meridian-server" --help >/dev/null 2>&1; then
        echo "  ✗ meridian-server not available after install" >&2
        exit 1
    fi
    echo "  ✓ meridian-server script available"
elif [[ "$(uname -s)" == "Darwin" && "$(uname -m)" == "arm64" ]]; then
    # Without --mlx, install just mlx-lm so llm_selector can detect local models.
    echo "Apple Silicon detected — installing mlx-lm for local inference..."
    "${SERVICES_DIR}/.venv/bin/pip" install --quiet "mlx-lm>=0.22,<1"
    echo "  ✓ mlx-lm installed"

    # On macOS 26+, install the Apple Foundation Models Python SDK so
    # llm_selector can use Apple Intelligence on 8 GB machines (no MLX model
    # download needed). Building the wheel requires Xcode.app (full, not CLT) —
    # it compiles a Swift/C bridge. Runtime only needs macOS 26 system frameworks.
    # npm users get a pre-built wheel from the release bundle and never need Xcode.
    _macos_major="$(sw_vers -productVersion 2>/dev/null | cut -d. -f1)"
    if [[ "${_macos_major:-0}" -ge 26 ]]; then
        if [[ ! -d /Applications/Xcode.app ]]; then
            echo "  ⚠ Xcode.app not found — skipping apple-fm-sdk (requires full Xcode to build)."
            echo "    Install Xcode 26+ from the App Store, open it once to accept the license, then re-run."
        else
            echo "macOS ${_macos_major} detected — installing apple-fm-sdk for Apple Intelligence..."
            "${SERVICES_DIR}/.venv/bin/pip" install --quiet "apple-fm-sdk"
            echo "  ✓ apple-fm-sdk installed"
        fi
    fi
fi

# 3b. Verify hermes is importable
echo "Verifying hermes install..."
if ! "${SERVICES_DIR}/.venv/bin/python3" -c "import run_agent" 2>/dev/null; then
    echo "  ERROR: 'run_agent' not importable — check requirements.txt includes hermes"
    exit 1
fi
echo "  ✓ hermes (run_agent) importable"

# 4. Hermes config setup
echo "Configuring hermes..."
bash "${SERVICES_DIR}/scripts/setup-hermes.sh"

# 5. Final check — warn if API key is still the placeholder
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
    echo "Next: start the MLX server, then cargo build --release && ./target/release/meridian"
    echo "  ${SERVICES_DIR}/.venv/bin/meridian-server --backend mlx --port 7823"
else
    echo "Next: cargo build --release && ./target/release/meridian"
fi
