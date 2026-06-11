#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
