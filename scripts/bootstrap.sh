#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Bootstrap installer. Downloads the latest signed + notarized Meridian.app
# (macOS, universal — Apple Silicon + Intel) from the GitHub release, installs
# it to /Applications,
# and launches it. The app stages its own background daemon on first run and
# opens the setup wizard (permissions + AI provider + tracker) - there is no npm,
# Node, or Python to install; the .app is fully self-contained.
#
# One-liner usage (download + inspect first if you prefer):
#   curl -fsSL https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.sh | bash
#
# Or run directly from a clone:
#   bash scripts/bootstrap.sh
#
# Re-running is safe - it reinstalls the latest build over any existing one.

set -euo pipefail

info() { printf '→ %s\n'   "$*"; }
ok()   { printf '  ✓ %s\n' "$*"; }
warn() { printf '  ⚠ %s\n' "$*" >&2; }
err()  { printf '✗ %s\n'   "$*" >&2; exit 1; }

# ── 0. Platform + safety guards ──────────────────────────────────────────────
[[ "$(uname -s)" == "Darwin" ]] || err "Meridian requires macOS."
[[ "$(id -u)" -ne 0          ]] || err "Do not run as root / with sudo. Re-run as your normal user."

DMG_URL="https://github.com/Meridiona/meridian/releases/latest/download/Meridian.dmg"
APP_NAME="Meridian.app"
DEST="/Applications"

_tmp="$(mktemp -d)"
_mount=""
cleanup() {
    [[ -n "${_mount}" ]] && hdiutil detach "${_mount}" -quiet 2>/dev/null || true
    rm -rf "${_tmp}"
}
trap cleanup EXIT

# ── 1. Download the DMG ──────────────────────────────────────────────────────
_dmg="${_tmp}/Meridian.dmg"
info "Downloading the latest Meridian release…"
curl -fSL --progress-bar -o "${_dmg}" "${DMG_URL}" \
    || err "Download failed from ${DMG_URL}
    Check your connection, or download the DMG manually from:
    https://github.com/Meridiona/meridian/releases/latest"
ok "downloaded ($(du -h "${_dmg}" | cut -f1 | tr -d ' '))"

# ── 2. Mount the image and copy the .app to /Applications ─────────────────────
info "Mounting the disk image…"
_mount="$(hdiutil attach "${_dmg}" -nobrowse -readonly 2>/dev/null \
    | grep -Eo '/Volumes/.*$' | tail -1)"
[[ -n "${_mount}" && -d "${_mount}/${APP_NAME}" ]] \
    || err "Could not find ${APP_NAME} inside the disk image."

# Quit a running instance so the copy replaces it cleanly.
osascript -e 'quit app "Meridian"' 2>/dev/null || true
sleep 1

info "Installing ${APP_NAME} to ${DEST}…"
rm -rf "${DEST:?}/${APP_NAME}"
cp -R "${_mount}/${APP_NAME}" "${DEST}/"
# The build is notarized, but strip the download quarantine flag so the very
# first launch never shows a Gatekeeper prompt.
xattr -dr com.apple.quarantine "${DEST}/${APP_NAME}" 2>/dev/null || true
hdiutil detach "${_mount}" -quiet 2>/dev/null || true
_mount=""
ok "installed → ${DEST}/${APP_NAME}"

# ── 3. Launch — the app self-installs its daemon + opens the setup wizard ─────
info "Launching Meridian…"
open "${DEST}/${APP_NAME}"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Meridian is installed and starting in your menu bar."
echo ""
echo "  The setup wizard opens on first launch - it walks you through:"
echo "    - macOS Screen Recording + Accessibility permissions"
echo "    - the AI that writes your summaries (a coding-agent CLI, or a cloud endpoint)"
echo "    - connecting your issue tracker (Jira / Linear / GitHub / Trello / Azure DevOps)"
echo ""
echo "  Updates install themselves from within the app - no need to re-run this."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
