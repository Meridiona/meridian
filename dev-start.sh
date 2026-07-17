#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Start all Meridian services in watch/hot-reload mode for local development.
#
# Prereqs (run once):
#   bash install-dev.sh          # installs deps, Claude Code integrations
#   cargo install cargo-watch    # Rust file watcher
#
# What this opens (3 Terminal windows):
#   1. Rust daemon  — cargo watch, rebuilds + restarts on a daemon-source save
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
#
# Also stops the CANONICAL launchd-managed daemon (com.meridiona.daemon), if
# a packaged/npm install of Meridian is also present on this machine. Without
# this, both the launchd daemon and the dev-build daemon started below run
# concurrently against the same meridian.db, each independently firing the
# clock-aligned worklog-hour trigger — producing two worklog runs (two
# distinct trace_ids, near-duplicate results) for the same hour window.
# ---------------------------------------------------------------------------
echo "→ stopping any previous dev run…"
pkill -f 'cargo-watch.*--bin meridian'  2>/dev/null || true   # daemon file-watcher
pkill -f 'target/debug/meridian$'       2>/dev/null || true   # daemon binary
pkill -f 'uvicorn agents.server:app'    2>/dev/null || true   # MLX dev server
pkill -f 'tauri dev'                    2>/dev/null || true   # tray file-watcher
pkill -f 'target/debug/meridian-tray$'  2>/dev/null || true   # tray binary
pkill -f 'Meridian Dev.app'             2>/dev/null || true   # stale dev .app bundle
# The tray SUPERVISES the MLX server: every poll tick, an unhealthy port with a
# runtime present is restarted (mlx_server.rs). So this must come AFTER the tray is
# dead — kill the server first and the supervisor simply puts it back.
#
# And it must match BY PATH, not by the uvicorn pattern above: `resolve_mlx_command`
# prefers the DOWNLOADED runtime (~/.meridian/runtime) over the checkout, so on a
# machine that also has Meridian installed the tray spawns
# `~/.meridian/runtime/bin/python server.py` — which `uvicorn agents.server:app`
# never matches. It is spawned detached, so it outlives the tray (orphaned to
# launchd) and holds port 7823 across dev runs. The symptom is the MLX window dying
# instantly with "[Errno 48] Address already in use" while a months-old runtime
# quietly serves every request — i.e. your services/ edits do nothing.
pkill -f '\.meridian/runtime/bin/python.*server\.py' 2>/dev/null || true  # installed-runtime MLX server
rm -f "${HOME}/.meridian/mlx-server.pid"                                 # its pid file (tray-owned)
# Anything else still holding 7823 (a hand-started server, a crashed reloader child
# whose cmdline no longer names uvicorn) — the port is the contract, so go by it.
# (BSD xargs has no `-r`, and `set -e` is on, so test before killing.)
MLX_PORT_PIDS="$(lsof -ti tcp:7823 2>/dev/null || true)"
if [ -n "$MLX_PORT_PIDS" ]; then
    # shellcheck disable=SC2086  # deliberate word-split: lsof returns one pid per line
    kill $MLX_PORT_PIDS 2>/dev/null || true
fi
# next dev is spawned by the tray's beforeDevCommand as a child of `tauri dev` —
# if tauri dev is killed abruptly (rapid restarts) it can be orphaned and keep
# holding port 3939, causing the next run's beforeDevCommand to fail outright.
pkill -f 'next dev --turbopack -p 3939' 2>/dev/null || true   # orphaned Next.js dev server
# Stop the canonical launchd daemon UNCONDITIONALLY and durably. Previously this
# was guarded on `launchctl print` succeeding and relied on bootout alone; if the
# bootout didn't take (KeepAlive respawn, wrong-instant timing) nothing else caught
# the installed daemon and it kept running next to the dev one — two daemons, both
# firing the HH:03 worklog trigger + ETL against the same meridian.db. So: `disable`
# (launchd won't re-launch it at login / kickstart), `bootout` (stop the live one),
# then pkill the installed binary BY PATH as a backstop (the dev pkills above only
# match `target/debug/meridian`, never `~/.meridian/**/bin/meridian`). Re-enable the
# installed daemon later with `meridian start`.
DAEMON_LABEL="gui/$(id -u)/com.meridiona.daemon"
launchctl disable "$DAEMON_LABEL" 2>/dev/null || true
launchctl bootout "$DAEMON_LABEL" 2>/dev/null || true
pkill -f '\.meridian/bin/meridian$'     2>/dev/null || true   # DMG-staged installed daemon
pkill -f '\.meridian/app/bin/meridian$' 2>/dev/null || true   # npm-bundle installed daemon
if pgrep -f '\.meridian/.*bin/meridian$' >/dev/null 2>&1; then
    echo "  ⚠ an installed daemon (~/.meridian/**/bin/meridian) is STILL running — quit the Meridian app and re-run" >&2
else
    echo "  ✓ canonical launchd daemon stopped + disabled (re-enable later with: meridian start)"
fi
# Say so if the port is still taken. Silence here is what let a months-old runtime
# serve every request for a whole session: the MLX window dies in its own Terminal
# with [Errno 48] and nothing else ever mentions it.
if [ -n "$(lsof -ti tcp:7823 2>/dev/null || true)" ]; then
    echo "  ⚠ something is STILL holding port 7823 — the MLX window will die with [Errno 48]" >&2
    lsof -nP -iTCP:7823 -sTCP:LISTEN 2>/dev/null | tail -n +2 | sed 's/^/      /' >&2
else
    echo "  ✓ port 7823 free (any installed-runtime MLX server stopped)"
fi
# Also stops the legacy launchd-managed a11y-helper, if present. Capture now runs
# in-process inside the dev tray binary, so a lingering a11y-helper would be a
# second, independent capture writer into the same meridian.db capture tables.
if launchctl print "gui/$(id -u)/com.meridiona.a11y-helper" >/dev/null 2>&1; then
    launchctl disable "gui/$(id -u)/com.meridiona.a11y-helper" 2>/dev/null || true
    launchctl bootout "gui/$(id -u)/com.meridiona.a11y-helper" 2>/dev/null || true
    echo "  ✓ stopped legacy launchd a11y-helper (com.meridiona.a11y-helper)"
fi
sleep 1   # let sockets / ports free before the new windows bind them
# Clear the Next.js build cache so stale module references (e.g. a deleted
# instrumentation.ts) don't cause beforeDevCommand to fail on the next run.
rm -rf "${REPO_ROOT}/ui/.next"
echo "  ✓ previous dev run stopped"

# ---------------------------------------------------------------------------
# Launch each service in its own Terminal window
# ---------------------------------------------------------------------------

# Optional: watch the screenpipe-fork alongside the daemon for hot-reload.
# Set SCREENPIPE_FORK_PATH in your shell or .env to a local clone of the fork.
# Example: export SCREENPIPE_FORK_PATH=~/src/screenpipe-fork
FORK_WATCH_FLAG=""
if [ -n "${SCREENPIPE_FORK_PATH:-}" ] && [ -d "${SCREENPIPE_FORK_PATH}" ]; then
    FORK_WATCH_FLAG="--watch '${SCREENPIPE_FORK_PATH}'"
fi

# Watch ONLY what actually rebuilds the daemon binary: its own sources, its two
# path dependencies, and the build inputs. `--watch .` (the old value) watches the
# entire repo, so a write anywhere — ui/, services/, tray/, a stray untracked file —
# restarts the daemon even though none of it can change the binary.
#
# That is not merely wasteful: SIGKILL mid-run is data-visible. The worklog pipeline
# needs many uninterrupted minutes to process an hour (await_coding_ready alone waits
# up to CODING_MAX_WAIT = 20 min), and a kill leaves pm_worklog_hours stranded at
# 'generating' — mark_hour_pending only runs on a clean Err, never on a kill. The
# next restart re-enters catch_up_today and restarts the 20-minute wait from zero,
# so a busy repo can starve the hour indefinitely and the timeline stops advancing.
# Editing daemon code still kills an in-flight hour; this stops everything else from.
DAEMON_WATCH="--watch src --watch meridian-core/src --watch meridian-oauth/src --watch build.rs --watch Cargo.toml"

osascript <<APPLESCRIPT
tell application "Terminal"
    activate

    -- 1. Rust daemon (cargo watch)
    do script "echo '=== Rust daemon (cargo watch) ===' && cd '${REPO_ROOT}' && cargo watch ${DAEMON_WATCH} ${FORK_WATCH_FLAG} -x 'run --bin meridian'"

    -- 2. MLX server (uvicorn --reload, watches services/agents/ only)
    do script "echo '=== MLX server (uvicorn --reload) ===' && cd '${REPO_ROOT}/services' && HF_TOKEN='${HF_TOKEN:-}' HF_XET_HIGH_PERFORMANCE=1 .venv/bin/uvicorn agents.server:app --reload --reload-dir '${REPO_ROOT}/services/agents' --host 127.0.0.1 --port 7823"

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

# Push any edited dashboard JSONs to the running OpenObserve instance.
# Runs in the background so it doesn't block dev-start; degrades silently if OO is off.
"${REPO_ROOT}/scripts/push-dashboards.sh" &
