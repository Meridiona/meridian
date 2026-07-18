#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Start all Meridian services in watch/hot-reload mode for local development.
#
# Prereqs (run once):
#   bash install-dev.sh          # installs deps, Claude Code integrations
#   cargo install cargo-watch    # Rust file watcher
#
# What this opens (2 Terminal windows):
#   1. Rust daemon  — cargo watch, rebuilds + restarts on a daemon-source save
#   2. Tauri tray   — npm run tauri dev (automatically starts Next.js hot-reload
#                     on port 3939 via beforeDevCommand; dashboard loads in the
#                     native Tauri webview)
#
# Capture (v1.64.0+) runs in-process inside the Tauri tray binary — no separate
# screenpipe or a11y-helper agent is needed. All generation runs through the
# user's chosen AI CLI (no on-device model server to start).
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
# The Rust daemon binds a unix socket (~/.meridian/daemon.sock). `npm run tauri
# dev` manages the Next.js dev server lifecycle internally (beforeDevCommand) —
# killing it here is enough.
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
pkill -f 'tauri dev'                    2>/dev/null || true   # tray file-watcher
pkill -f 'target/debug/meridian-tray$'  2>/dev/null || true   # tray binary
pkill -f 'Meridian Dev.app'             2>/dev/null || true   # stale dev .app bundle
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
# entire repo, so a write anywhere — ui/, tray/, a stray untracked file —
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

    -- 2. Tauri tray (hot reload — also starts Next.js dev server automatically via beforeDevCommand)
    do script "echo '=== Tauri tray (tauri dev) ===' && cd '${REPO_ROOT}/tray' && npm run tauri dev"
end tell
APPLESCRIPT

echo ""
echo "✓ Dev services starting in 2 Terminal windows:"
echo ""
echo "  1. Rust daemon  — rebuilds automatically on .rs save"
echo "  2. Tauri tray   — hot reload (Next.js dev server starts automatically)"
echo ""
echo "  Dashboard: open the Meridian tray icon → Open Dashboard"
echo "  Capture runs in-process inside the tray — no separate agent needed."
echo ""
echo "  To stop: Ctrl-C in each window"

# Push any edited dashboard JSONs to the running OpenObserve instance.
# Runs in the background so it doesn't block dev-start; degrades silently if OO is off.
"${REPO_ROOT}/scripts/push-dashboards.sh" &
