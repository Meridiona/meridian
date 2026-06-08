#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Dev-mode install: debug Rust binary, no Next.js production build, no tray build.
# Background services (screenpipe, MLX server, Rust daemon) still register as launchd agents.
# UI:  cd ui && npm run dev
# Tray: automatically starts in a new terminal via npm run tauri dev

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Run main install with --dev flag
bash "${REPO_ROOT}/install.sh" --dev "$@"

# Start tray app in a new Terminal window
echo ""
echo "→ Starting tray app in a new Terminal window..."
osascript <<APPLESCRIPT
tell application "Terminal"
    activate
    do script "cd '${REPO_ROOT}/tray' && npm run tauri dev"
end tell
APPLESCRIPT

echo "  ✓ Tray app starting — it will open in a new Terminal window"
echo ""
echo "You can now:"
echo "  cd ui && npm run dev          # start Next.js dashboard (separate terminal)"
echo "  tail -f ~/.meridian/logs/*.log # monitor background services"

