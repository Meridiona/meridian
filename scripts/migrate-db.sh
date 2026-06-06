#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Force-apply all pending database migrations.
# Use this if the UI is showing schema-mismatch errors (e.g. "no such column").
#
# Usage:
#   bash scripts/migrate-db.sh

set -euo pipefail

info() { printf '→ %s\n'   "$*"; }
ok()   { printf '  ✓ %s\n' "$*"; }
err()  { printf '✗ %s\n'   "$*" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Make sure meridian binary exists
if ! command -v meridian-daemon >/dev/null 2>&1; then
    err "meridian-daemon not found on PATH — run ./install.sh first"
fi

DB_PATH="${HOME}/.meridian/meridian.db"

if [[ ! -f "${DB_PATH}" ]]; then
    err "Database not found at ${DB_PATH}"
fi

info "Backing up database…"
BACKUP_PATH="${DB_PATH}.backup.$(date +%s)"
cp "${DB_PATH}" "${BACKUP_PATH}"
ok "Backed up to: ${BACKUP_PATH}"

# Stop the daemon so we can modify the database safely
info "Stopping meridian daemon…"
launchctl bootout "gui/$(id -u)/com.meridiona.daemon" 2>/dev/null || true
sleep 2

# Use SQLx CLI to run migrations (if available), or sqlx::migrate via Rust
info "Running database migrations…"
if command -v sqlx >/dev/null 2>&1; then
    sqlx migrate run --database-url "sqlite://${DB_PATH}" \
        --ignore-missing \
        || err "Failed to run migrations via sqlx CLI"
else
    # Fallback: restart daemon to trigger migrations (it runs them on startup)
    info "sqlx CLI not found — restarting daemon to apply migrations"
    launchctl bootstrap "gui/$(id -u)" "${HOME}/Library/LaunchAgents/com.meridiona.daemon.plist" 2>/dev/null || true
    sleep 3
fi

ok "Database migrations applied"

# Restart the daemon
info "Restarting meridian daemon…"
launchctl enable "gui/$(id -u)/com.meridiona.daemon" 2>/dev/null || true
launchctl kickstart -k "gui/$(id -u)/com.meridiona.daemon"
sleep 2

ok "Daemon restarted"
ok "Migration complete"

echo ""
echo "If the UI still shows schema errors:"
echo "  1. Check the daemon log: tail -f ~/.meridian/logs/daemon-error.log"
echo "  2. Verify the database: sqlite3 ${DB_PATH} '.schema app_sessions' | grep claude_session_uuid"
echo "  3. Consider restoring the backup: cp ${BACKUP_PATH} ${DB_PATH}"
