#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Fix a11y-helper accessibility permission interactively.
# Use this if the initial setup didn't prompt for permission or if the grant
# was revoked.
#
# Usage:
#   bash scripts/fix-a11y-helper-permission.sh

set -euo pipefail

info() { printf '→ %s\n'   "$*"; }
ok()   { printf '  ✓ %s\n' "$*"; }
warn() { printf '  ⚠ %s\n' "$*" >&2; }

# Check if a11y-helper has accessibility permission granted by reading its log
is_a11y_helper_trusted() {
    local log_file="${HOME}/.meridian/logs/a11y-helper.log"
    if [[ ! -f "${log_file}" ]]; then
        return 1
    fi
    # Get the LAST occurrence of "AX trusted:" line
    local last_line
    last_line="$(grep "AX trusted:" "${log_file}" | tail -1)"
    if [[ -z "${last_line}" ]]; then
        return 1
    fi
    if echo "${last_line}" | grep -q "AX trusted: true"; then
        return 0
    fi
    return 1
}

local_a11y_helper_path="${HOME}/.meridian/bin/meridian-a11y-helper"

# Verify the helper binary exists
if [[ ! -f "${local_a11y_helper_path}" ]]; then
    warn "a11y-helper binary not found at ${local_a11y_helper_path}"
    echo "  Run: bash scripts/install-a11y-helper-daemon.sh"
    exit 1
fi

# If already trusted, nothing to do
if is_a11y_helper_trusted; then
    ok "a11y-helper is already trusted"
    exit 0
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Grant accessibility permission to a11y-helper"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  The a11y-helper daemon enables accessibility on Electron/Chromium"
echo "  apps (Claude Desktop, Codex, VS Code, Slack, …) so screenpipe can"
echo "  capture them. This requires a one-time macOS accessibility grant."
echo ""

read -r -p "  Press Enter to open System Settings → Accessibility… " _
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"

echo ""
echo "  Steps:"
echo "    1. Click '+' to add an application"
echo "    2. Navigate to the user's home folder, then to .meridian/bin/"
echo "    3. Select: meridian-a11y-helper"
echo "    4. Toggle the switch ON (to the right)"
echo ""

read -r -p "  Press Enter when the toggle is ON… " _

# Auto-restart the daemon to pick up the new permission
info "Restarting a11y-helper daemon…"
local gui_target="gui/$(id -u)"
local label="com.meridiona.a11y-helper"
launchctl kickstart -k "${gui_target}/${label}" 2>/dev/null || true
sleep 2

# Verify the permission was granted
if is_a11y_helper_trusted; then
    echo ""
    ok "Success! a11y-helper is now trusted."
    ok "Electron apps will be captured on your next window focus."
    exit 0
else
    echo ""
    warn "a11y-helper still reports untrusted"
    echo "  → Make sure the toggle in System Settings is fully ON"
    echo "  → Check System Settings did not close without saving"
    echo "  → Try again: bash scripts/fix-a11y-helper-permission.sh"
    echo ""
    echo "  Or manually check status:"
    echo "    tail -f ~/.meridian/logs/a11y-helper.log"
    exit 1
fi
