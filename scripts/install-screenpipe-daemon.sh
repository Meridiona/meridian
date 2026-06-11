#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Install screenpipe as a launchd LaunchAgent under the current user.
# screenpipe runs continuously, recording the screen (audio disabled via
# --disable-audio) on its default port 3030 with data stored in ~/.screenpipe.
#
#   ./scripts/install-screenpipe-daemon.sh
#
# Re-running this script is safe — it bootouts the existing agent first,
# rewrites the plist with current paths, and reloads it.
#
# Uninstall:
#   ./scripts/uninstall-screenpipe-daemon.sh
#   Or manually:
#     launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.screenpipe.plist
#     rm ~/Library/LaunchAgents/com.meridiona.screenpipe.plist

set -euo pipefail

LABEL="com.meridiona.screenpipe"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="${SCRIPT_DIR}/com.meridiona.screenpipe.plist"

LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"

GUI_TARGET="gui/$(id -u)"

if [[ ! -f "${TEMPLATE}" ]]; then
    echo "✗ template not found: ${TEMPLATE}" >&2
    exit 1
fi

# Locate the screenpipe binary. The npm package ships a Node *wrapper* at
# `command -v screenpipe` (a cli.js shim) that spawns the real arm64 Mach-O. If
# the launchd agent launches that wrapper, macOS attributes Screen Recording /
# Accessibility to `node` (the responsible process) — a broad, fragile grant
# (breaks on node upgrades) that also shows a scary "node wants to record your
# screen" prompt. Resolve the real Mach-O and launch it directly so the grant
# attaches to a stable binary named `screenpipe` (and survives reinstalls of the
# same version, since its path is fixed). Falls back to whatever `command -v`
# found when screenpipe is a native binary (Homebrew) rather than the npm shim.
STAGED_BIN="${HOME}/.meridian/bin/screenpipe"

# Determine the binary path to write into the plist.
#
# Priority order — chosen to avoid invalidating existing TCC grants:
#
# 1. Existing plist path is still a valid Mach-O → keep it.
#    macOS TCC grants are per absolute path. Changing the path silently
#    revokes the grant and breaks screenpipe on the next start. Preserve
#    the old path on updates so working installs stay working.
#
# 2. Already-staged stable binary exists → use it.
#    Covers fresh installs and repairs where no old plist exists.
#
# 3. Resolve the real Mach-O from the npm tree → stage it → use it.
#    Handles new installs where neither a plist nor a staged binary exists.
#    nvm users get a version-specific path under ~/.nvm that breaks on
#    `nvm use` or Node upgrades, so we always copy to the stable
#    ~/.meridian/bin/screenpipe location.
_old_plist_bin=""
if [[ -f "${PLIST_DEST}" ]]; then
    _old_plist_bin="$(plutil -extract ProgramArguments.0 raw "${PLIST_DEST}" 2>/dev/null || true)"
fi

if [[ -n "${_old_plist_bin}" ]] && [[ -x "${_old_plist_bin}" ]] && file "${_old_plist_bin}" 2>/dev/null | grep -q "Mach-O"; then
    SCREENPIPE_BIN="${_old_plist_bin}"
    echo "→ keeping existing screenpipe binary (preserves TCC grant): ${SCREENPIPE_BIN}"
elif [[ -x "${STAGED_BIN}" ]] && file "${STAGED_BIN}" 2>/dev/null | grep -q "Mach-O"; then
    SCREENPIPE_BIN="${STAGED_BIN}"
    echo "→ using staged screenpipe binary: ${SCREENPIPE_BIN}"
else
    SCREENPIPE_BIN="$(command -v screenpipe 2>/dev/null || true)"
    if [[ -z "${SCREENPIPE_BIN}" ]]; then
        echo "✗ screenpipe not found in PATH — install with: npm install -g screenpipe" >&2
        exit 1
    fi
    _npm_root="$(npm root -g 2>/dev/null || true)"
    if [[ -n "${_npm_root}" && -d "${_npm_root}/screenpipe" ]]; then
        _real=""
        while IFS= read -r _cand; do
            if file "${_cand}" 2>/dev/null | grep -q "Mach-O"; then _real="${_cand}"; break; fi
        done < <(find "${_npm_root}/screenpipe" -type f -name screenpipe -perm +0111 2>/dev/null)
        if [[ -n "${_real}" ]]; then
            SCREENPIPE_BIN="${_real}"
        fi
    fi
    mkdir -p "${HOME}/.meridian/bin"
    cp "${SCREENPIPE_BIN}" "${STAGED_BIN}"
    chmod +x "${STAGED_BIN}"
    SCREENPIPE_BIN="${STAGED_BIN}"
    echo "→ staged screenpipe binary: ${SCREENPIPE_BIN}"
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${LAUNCH_AGENTS}"

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{SCREENPIPE_BIN}}|${SCREENPIPE_BIN}|g" \
    "${TEMPLATE}" > "${PLIST_DEST}"

# Validate the plist before loading.
if ! plutil -lint "${PLIST_DEST}" >/dev/null; then
    echo "✗ plist failed validation" >&2
    exit 1
fi

# Always attempt bootout by label — launchctl print can return non-zero even when
# the label is still registered (e.g. service stopped but domain entry exists),
# causing bootstrap to fail with EIO. Label-based bootout is also more reliable
# when the plist content changed since the last load.
echo "→ bootout ${LABEL} (if loaded)"
launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
# bootout is async — wait until the domain entry actually clears before
# bootstrapping, otherwise launchctl bootstrap can fail with EIO (errno 5).
_bootout_wait=0
while launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; do
    sleep 1
    _bootout_wait=$(( _bootout_wait + 1 ))
    if [[ "${_bootout_wait}" -ge 15 ]]; then
        echo "⚠ ${LABEL} still in launchd domain after 15s — proceeding anyway" >&2
        break
    fi
done

echo "→ bootstrap ${LABEL}"
launchctl enable "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
launchctl bootstrap "${GUI_TARGET}" "${PLIST_DEST}"
launchctl enable "${GUI_TARGET}/${LABEL}"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}"

echo
echo "✓ screenpipe installed and started"
echo
echo "Useful follow-ups:"
echo "  launchctl print  ${GUI_TARGET}/${LABEL}              # status"
echo "  tail -f ~/.meridian/logs/screenpipe.log               # live stdout"
echo "  tail -f ~/.meridian/logs/screenpipe-error.log         # live stderr"
echo "  ${SCRIPT_DIR}/uninstall-screenpipe-daemon.sh          # remove"

# Note: make this script executable after cloning:
#   chmod +x scripts/install-screenpipe-daemon.sh
