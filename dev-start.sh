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
# Stop any previous dev run FIRST so re-running this is idempotent. The Rust
# daemon binds a unix socket (~/.meridian/daemon.sock), NOT a TCP port, and just
# removes+rebinds it on launch — so a daemon from an earlier run keeps running
# silently and races the same DB / etl_cursor (you'd accumulate N daemons after
# N runs, stalling classification). Kill the backend watchers + binaries here.
# ---------------------------------------------------------------------------
UI_PORT="${MERIDIAN_UI_PORT:-3939}"
echo "→ stopping any previous dev run…"
pkill -f 'cargo-watch.*--bin meridian'  2>/dev/null || true   # daemon file-watcher
pkill -f 'target/debug/meridian$'       2>/dev/null || true   # daemon binary (not -tray / -server)
pkill -f 'uvicorn agents.server:app'    2>/dev/null || true   # MLX dev server (uvicorn --reload)
# Stop the launchd MLX server so the dev uvicorn can bind port 7823.
launchctl stop "gui/$(id -u)/com.meridiona.mlx-server" 2>/dev/null || true
# Kill any stray process still holding port 7823.
_mlx_pids="$(lsof -ti tcp:7823 2>/dev/null || true)"
[[ -n "${_mlx_pids}" ]] && kill ${_mlx_pids} 2>/dev/null || true
pkill -f 'tauri dev'                    2>/dev/null || true    # tray file-watcher
pkill -f 'target/debug/meridian-tray$'  2>/dev/null || true   # tray binary
# Free the dashboard port if a prior `next dev` still holds it.
_ui_pids="$(lsof -ti "tcp:${UI_PORT}" 2>/dev/null || true)"
[[ -n "${_ui_pids}" ]] && kill ${_ui_pids} 2>/dev/null || true
sleep 1   # let sockets / ports free before the new windows bind them
echo "  ✓ previous dev run stopped"

# ---------------------------------------------------------------------------
# Ensure screenpipe is up — meridian stop disables it via launchctl, so it
# won't auto-restart. Re-enable + bootstrap + kickstart it here so the daemon
# has frames to read. Idempotent: safe to run when screenpipe is already live.
# ---------------------------------------------------------------------------
LABEL_SCREENPIPE="com.meridiona.screenpipe"
GUI_TARGET="gui/$(id -u)"
SP_PLIST="${HOME}/Library/LaunchAgents/${LABEL_SCREENPIPE}.plist"
if [[ -f "$SP_PLIST" ]]; then
    echo "→ ensuring screenpipe is running…"
    launchctl enable    "${GUI_TARGET}/${LABEL_SCREENPIPE}" 2>/dev/null || true
    launchctl bootstrap "${GUI_TARGET}" "$SP_PLIST"        2>/dev/null || true
    # bootstrap + RunAtLoad starts screenpipe immediately; no kickstart needed
    # (kickstart -k would block waiting for screenpipe to re-initialise camera/screen capture)
    echo "  ✓ screenpipe (re)started"
else
    echo "  ⚠ screenpipe plist not found — run: bash install-dev.sh"
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
echo "  To stop: Ctrl-C in each window + meridian stop (for launchd services)
  To restore production MLX daemon: meridian start"
