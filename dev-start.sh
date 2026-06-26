#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Start all Meridian services in watch/hot-reload mode for local development.
#
# Prereqs (run once):
#   bash install-dev.sh          # installs deps, Claude Code integrations
#   cargo install cargo-watch    # Rust file watcher
#
# What this opens (3 Terminal windows):
#   1. Rust daemon  — cargo watch, rebuilds + restarts on every .rs save
#   2. MLX server   — uvicorn --reload, reloads on every .py save in services/agents/
#   3. Tauri tray   — npm run tauri dev (automatically starts Next.js hot-reload
#                     on port 3939 via beforeDevCommand; dashboard loads in the
#                     native Tauri webview)
#
# Capture (v1.64.0+) runs in-process inside the Tauri tray binary — no separate
# screenpipe or a11y-helper agent is needed.
#

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

if [[ ! -d "${REPO_ROOT}/tray/node_modules" ]]; then
    echo "✗ tray/node_modules not found — run: bash install-dev.sh" >&2
    exit 1
fi

if [[ ! -d "${REPO_ROOT}/ui/node_modules" ]]; then
    echo "✗ ui/node_modules not found — run: bash install-dev.sh" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Stop any previous dev run FIRST so re-running is idempotent.
# The Rust daemon binds a unix socket (~/.meridian/daemon.sock) and the MLX
# server binds port 7823. `npm run tauri dev` manages the Next.js dev server
# lifecycle internally (beforeDevCommand) — killing it here is enough.
# ---------------------------------------------------------------------------
echo "→ stopping any previous dev run…"
pkill -f 'cargo-watch.*--bin meridian'  2>/dev/null || true   # daemon file-watcher
pkill -f 'target/debug/meridian$'       2>/dev/null || true   # daemon binary
pkill -f 'uvicorn agents.server:app'    2>/dev/null || true   # MLX dev server
pkill -f 'tauri dev'                    2>/dev/null || true   # tray file-watcher
pkill -f 'target/debug/meridian-tray$'  2>/dev/null || true   # tray binary
pkill -f 'Meridian Dev.app'             2>/dev/null || true   # stale dev .app bundle
sleep 1   # let sockets / ports free before the new windows bind them
# Clear the Next.js build cache so stale module references (e.g. a deleted
# instrumentation.ts) don't cause beforeDevCommand to fail on the next run.
rm -rf "${REPO_ROOT}/ui/.next"
echo "  ✓ previous dev run stopped"

# ---------------------------------------------------------------------------
# Launch each service in its own Terminal window
# ---------------------------------------------------------------------------

osascript <<APPLESCRIPT
tell application "Terminal"
    activate

    -- 1. Rust daemon (cargo watch)
    do script "echo '=== Rust daemon (cargo watch) ===' && cd '${REPO_ROOT}' && cargo watch --watch . --watch '/Users/adityaharish/Documents/Meridiona/screenpipe-fork' -x 'run --bin meridian'"

    -- 2. MLX server (uvicorn --reload, watches services/agents/ only)
    do script "echo '=== MLX server (uvicorn --reload) ===' && cd '${REPO_ROOT}/services' && .venv/bin/uvicorn agents.server:app --reload --reload-dir '${REPO_ROOT}/services/agents' --host 127.0.0.1 --port 7823"

    -- 3. Tauri tray (hot reload — also starts Next.js dev server automatically via beforeDevCommand)
    do script "echo '=== Tauri tray (tauri dev) ===' && cd '${REPO_ROOT}/tray' && npm run tauri dev"
end tell
APPLESCRIPT

echo ""
echo "✓ Dev services starting in 3 Terminal windows:"
echo ""
echo "  1. Rust daemon  — rebuilds automatically on .rs save"
echo "  2. MLX server   — reloads on .py changes in services/agents/"
echo "  3. Tauri tray   — hot reload (Next.js dev server starts automatically)"
echo ""
echo "  Dashboard: open the Meridian tray icon → Open Dashboard"
echo "  Capture runs in-process inside the tray — no separate agent needed."
echo ""
echo "  To stop: Ctrl-C in each window"
