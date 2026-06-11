#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Start all Meridian services in watch/hot-reload mode for local development.
#
# Prereqs (run once):
#   bash install-dev.sh          # installs deps, screenpipe + a11y-helper launchd agents
#   cargo install cargo-watch    # Rust file watcher
#
# What this opens (4 Terminal windows):
#   1. Rust daemon   — cargo watch, rebuilds + restarts on every .rs save
#   2. MLX server    — uvicorn --reload, reloads on every .py save in services/agents/
#   3. Next.js UI    — npm run dev, hot reload at http://localhost:3939
#   4. Tauri tray    — npm run tauri dev, hot reload
#
# screenpipe + a11y-helper run via launchd and do not need restarting.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# Pre-flight checks
# ---------------------------------------------------------------------------

if ! command -v cargo >/dev/null 2>&1; then
    echo "✗ cargo not found — install Rust: https://rustup.rs" >&2
    exit 1
fi

if ! cargo watch --version >/dev/null 2>&1; then
    echo "→ cargo-watch not found — installing..."
    cargo install cargo-watch
    echo "  ✓ cargo-watch installed"
fi

if [[ ! -d "${REPO_ROOT}/services/.venv" ]]; then
    echo "✗ services/.venv not found — run: bash install-dev.sh" >&2
    exit 1
fi

if [[ ! -d "${REPO_ROOT}/ui/node_modules" ]]; then
    echo "✗ ui/node_modules not found — run: bash install-dev.sh" >&2
    exit 1
fi

if [[ ! -d "${REPO_ROOT}/tray/node_modules" ]]; then
    echo "✗ tray/node_modules not found — run: bash install-dev.sh" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Launch each service in its own Terminal window
# ---------------------------------------------------------------------------

osascript <<APPLESCRIPT
tell application "Terminal"
    activate

    -- 1. Rust daemon (cargo watch)
    do script "echo '=== Rust daemon (cargo watch) ===' && cd '${REPO_ROOT}' && cargo watch -x 'run --bin meridian'"

    -- 2. MLX server (uvicorn --reload, watches services/agents/ only)
    do script "echo '=== MLX server (uvicorn --reload) ===' && cd '${REPO_ROOT}/services' && .venv/bin/uvicorn agents.server:app --reload --reload-dir '${REPO_ROOT}/services/agents' --host 127.0.0.1 --port 7823"

    -- 3. Next.js UI (hot reload)
    do script "echo '=== Next.js UI ===' && cd '${REPO_ROOT}/ui' && npm run dev"

    -- 4. Tauri tray (hot reload)
    do script "echo '=== Tauri tray ===' && cd '${REPO_ROOT}/tray' && npm run tauri dev"
end tell
APPLESCRIPT

echo ""
echo "✓ Dev services starting in 4 Terminal windows:"
echo ""
echo "  1. Rust daemon   — rebuilds automatically on .rs save"
echo "  2. MLX server    — reloads on .py changes in services/agents/"
echo "  3. Next.js UI    — http://localhost:3939 (hot reload)"
echo "  4. Tauri tray    — hot reload"
echo ""
echo "  screenpipe + a11y-helper running via launchd (no restarts needed)."
echo ""
echo "  To stop: Ctrl-C in each window + meridian stop (for launchd services)"
