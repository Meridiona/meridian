#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Start tray app in dev mode (with hot reload)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

info()  { echo "→ $*" >&2; }
ok()    { echo "  ✓ $*" >&2; }

info "Starting tray app in dev mode..."
info "When you're done, stop it with Ctrl+C"
echo

cd "${REPO_ROOT}/tray"
npm run tauri dev
