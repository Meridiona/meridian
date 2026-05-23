#!/usr/bin/env bash
# meridian — set up the Python services layer for a new dev machine
#
# Run once after cloning:
#   bash scripts/setup-services.sh
#
# Safe to re-run. Only overwrites hermes config.yaml, never .env.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVICES_DIR="${REPO_ROOT}/services"

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

# 2. Install dependencies
echo "Installing Python dependencies..."
"${SERVICES_DIR}/.venv/bin/pip" install --quiet -r "${SERVICES_DIR}/requirements.txt"
echo "  ✓ requirements installed"

# 3. Verify hermes is importable
echo "Verifying hermes install..."
if ! "${SERVICES_DIR}/.venv/bin/python3" -c "import run_agent" 2>/dev/null; then
    echo "  ERROR: 'run_agent' not importable — check requirements.txt includes hermes"
    exit 1
fi
echo "  ✓ hermes (run_agent) importable"

# 4. Run hermes config setup
echo "Configuring hermes..."
bash "${SERVICES_DIR}/scripts/setup-hermes.sh"

# 5. Final check — warn if API key is still the placeholder
HERMES_ENV="${SERVICES_DIR}/.hermes/.env"
if grep -q "YOUR_API_KEY_HERE\|<your" "${HERMES_ENV}" 2>/dev/null; then
    echo ""
    echo "  ⚠  OLLAMA_API_KEY in services/.hermes/.env is still a placeholder."
    echo "     Edit it before running the daemon:"
    echo "       \$EDITOR ${HERMES_ENV}"
fi

echo ""
echo "=== setup complete ==="
echo ""
echo "Next: cargo build --release && ./target/release/meridian"
