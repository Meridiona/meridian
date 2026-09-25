#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# dev-reset-onboarding.sh — put this machine back into a first-run state so the
# setup wizard can be tested again, over and over.
#
# Onboarding is gated by marker files in ~/.meridian (plus the DB, the credential
# store, the signed-in account session, and the macOS TCC grants). Deleting them
# by hand is unreliable for one reason: BOTH writers are still running. The
# daemon is a KeepAlive launchd agent and the tray watchdog respawns it, so a
# `rm` races a process that immediately recreates meridian.db, daemon.sock and
# the WAL files. This script always stops the tray first, boots out the launchd
# agents second, and only then touches the filesystem.
#
# Tiers (pick one — they are cumulative, not independent):
#   --check   read-only. Print every gate: which markers exist, whether the DB
#             and keychain key exist, TCC grants, what's running. Run it before
#             AND after a wipe — knowing the state is what makes the loop fast.
#   --soft    marker files only (~1s). The wizard reopens; DB, credentials,
#             sign-in and permission grants all survive. This is the loop you
#             want for wizard copy/logic/step-ordering iteration.
#   --data    --soft plus user data: meridian.db, settings.json, oauth/, .env,
#             account.json (the whole sign-in state - no session lives anywhere
#             else) and the WebKit caches, cleared for general webview-cache
#             hygiene. Keeps the keychain key and the TCC grants.
#   --deep    shells out to `meridian uninstall --remove-data --yes` — the real
#             uninstall path: launchd agents, staged binaries, the install
#             marker, ~/.meridian data, the OS keychain entry and a tccutil
#             reset — but KEEPS the downloaded runtime and models. This is the
#             tier to use for install-path testing; --full re-downloads several
#             GB of models every iteration, which --deep does not.
#   --full    shells out to `meridian uninstall --purge --yes` — everything
#             --deep does plus the runtime and the models, and an rm -rf of
#             ~/.meridian. Slowest. Only when you need a genuinely virgin disk.
#
# Modifiers:
#   --tcc       also reset the Accessibility / Screen Recording / Input
#               Monitoring grants on --soft/--data (--deep/--full always do).
#               Deliberately separate: re-granting is a 30-60s tax per iteration
#               and macOS sometimes silently denies instead of re-prompting,
#               which reads as a wizard bug. Only pass it when the three
#               permission steps are what you are actually testing.
#   --relaunch  reopen Meridian.app when the wipe finishes (the tray re-stages
#               the daemon on launch, so this is the whole restart).
#   --yes       skip the confirmation prompt.
#
# Before the first iteration, run scripts/dev-signing.sh. Ad-hoc rebuilds churn
# the cdhash, TCC anchors its grants to that cdhash, and you will re-prompt for
# Screen Recording every single build — indistinguishable from a real bug.
#
# Test the PACKAGED app, not `tauri dev`: the popover 404s under next dev, so dev
# mode cannot reproduce a real first run.

set -euo pipefail

MERIDIAN_DIR="${HOME}/.meridian"
GUI_TARGET="gui/$(id -u)"
TRAY_BUNDLE_ID="com.meridiona.tray"
APP_PATH="/Applications/Meridian.app"
# Mirrors SERVICE/ACCOUNT in tray/src-tauri/src/db_key.rs.
KEYCHAIN_SERVICE="Meridian"
KEYCHAIN_ACCOUNT="db-encryption-key"

TIER=""
RESET_TCC=0
RELAUNCH=0
ASSUME_YES=0

info() { echo "→ $*"; }
ok()   { echo "  ✓ $*"; }
warn() { echo "  ! $*" >&2; }

usage() {
    # The leading comment block is the documentation - print it rather than
    # maintaining a second copy that drifts. Stops at the first non-comment line.
    awk 'NR<3{next} /^[^#]/{exit} {sub(/^# ?/,""); print}' "${BASH_SOURCE[0]}"
    echo
    echo "usage: $(basename "$0") --check | --soft | --data | --deep | --full [--tcc] [--relaunch] [--yes]"
}

# ---------------------------------------------------------------- gate lists

# Marker files that gate onboarding. This is deliberately NOT a copy of
# `data_items()` in src/uninstall.rs — that list omits account.json,
# analytics_state.json, plan_auto_opened, last_seen_version, backend-version and
# the provider caches (`--purge` only catches them via its blanket rm -rf), so
# copying it would inherit the gap.
SOFT_MARKERS=(
    onboarded            # wizard completed — the flag the tray reads on launch
    setup_started        # when the wizard was first opened
    walkthrough_armed    # arms the post-wizard dashboard walkthrough
    plan_auto_opened     # daily-plan nudge suppression
    autostart_configured # left behind, a reinstall never re-registers login item
    last_seen_version    # gates the What's New auto-open
    analytics_state.json # PostHog per-day/email bookkeeping
    setup_state.json     # (harmless if absent — future-proofing)
    # sha256 of the staged daemon. Stale + an emptied bin/ means the tray decides
    # the backend is "up to date" and never re-stages it, so you iterate against
    # a daemon that is not there. Always clear it.
    backend-version
    provider_test_cache.json
    provider_runtime_health.json
)

# Additional user data for --data. account.json ALONE is now the full sign-in
# state — there is no separate client session to also clear (email capture is
# a one-time write, not a login). The WebKit caches below are cleared for
# general webview-cache hygiene, unrelated to sign-in.
DATA_ITEMS=(
    meridian.db
    meridian.db-shm
    meridian.db-wal
    settings.json
    oauth
    account.json
    .env
    logs
    daemon.sock
    icon-cache
)

# macOS OS-managed app data for the tray's bundle id — cookies/localStorage/
# webview cache. No sign-in state lives here anymore (see DATA_ITEMS' note on
# account.json above) — kept for general webview-cache hygiene. Mirrors
# `app_cache_items()`.
app_cache_paths() {
    printf '%s\n' \
        "${HOME}/Library/Application Support/${TRAY_BUNDLE_ID}" \
        "${HOME}/Library/Caches/${TRAY_BUNDLE_ID}" \
        "${HOME}/Library/WebKit/${TRAY_BUNDLE_ID}" \
        "${HOME}/Library/Saved Application State/${TRAY_BUNDLE_ID}.savedState" \
        "${HOME}/Library/HTTPStorages/${TRAY_BUNDLE_ID}"
}

# ---------------------------------------------------------------- check mode

# `|| true` is load-bearing: pgrep exits 1 on no match, and under `set -o
# pipefail` that would abort the script at every bare `$(tray_pid)` assignment -
# i.e. exactly when nothing is running, which is the state the "after" report
# runs in.
tray_pid()   { pgrep -f "${APP_PATH}/Contents/MacOS/meridian-tray" 2>/dev/null | head -1 || true; }
daemon_pid() { pgrep -f "${MERIDIAN_DIR}/bin/meridian" 2>/dev/null | head -1 || true; }

mark() { if [[ -e "$2" ]]; then echo "  present  $1"; else echo "  ABSENT   $1"; fi; }

tcc_state() {
    # Read the user TCC store directly. Requires Full Disk Access for the
    # terminal; degrade to "unknown" rather than lying.
    local db="${HOME}/Library/Application Support/com.apple.TCC/TCC.db"
    local svc="$1"
    local out
    if ! out=$(sqlite3 "${db}" \
        "select auth_value from access where service='${svc}' and client='${TRAY_BUNDLE_ID}';" 2>/dev/null); then
        echo "unknown (grant this terminal Full Disk Access to read TCC)"
        return
    fi
    case "${out}" in
        "")  echo "not set (will prompt)" ;;
        0|1) echo "DENIED" ;;
        2)   echo "granted" ;;
        *)   echo "auth_value=${out}" ;;
    esac
}

do_check() {
    echo "── onboarding gates ─────────────────────────────────────────"
    for m in "${SOFT_MARKERS[@]}"; do mark "${m}" "${MERIDIAN_DIR}/${m}"; done
    echo
    echo "── user data ────────────────────────────────────────────────"
    for m in "${DATA_ITEMS[@]}"; do mark "${m}" "${MERIDIAN_DIR}/${m}"; done
    # Labelled by parent dir - every one of these basenames is the bundle id.
    while IFS= read -r p; do
        mark "$(basename "$(dirname "${p}")")/$(basename "${p}")" "${p}"
    done < <(app_cache_paths)
    echo
    echo "── keychain ─────────────────────────────────────────────────"
    if security find-generic-password -s "${KEYCHAIN_SERVICE}" -a "${KEYCHAIN_ACCOUNT}" >/dev/null 2>&1; then
        echo "  present  SQLCipher DB key (${KEYCHAIN_SERVICE}/${KEYCHAIN_ACCOUNT})"
    else
        echo "  ABSENT   SQLCipher DB key (${KEYCHAIN_SERVICE}/${KEYCHAIN_ACCOUNT})"
    fi
    echo
    echo "── macOS permissions (${TRAY_BUNDLE_ID}) ────────────────────"
    echo "  Accessibility     $(tcc_state kTCCServiceAccessibility)"
    echo "  Screen Recording  $(tcc_state kTCCServiceScreenCapture)"
    echo "  Input Monitoring  $(tcc_state kTCCServiceListenEvent)"
    echo
    echo "── processes / agents ───────────────────────────────────────"
    local t d
    t=$(tray_pid); d=$(daemon_pid)
    echo "  tray    ${t:-not running}"
    echo "  daemon  ${d:-not running}"
    for label in com.meridiona.daemon com.meridiona.a11y-helper; do
        if launchctl print "${GUI_TARGET}/${label}" >/dev/null 2>&1; then
            echo "  agent   ${label}: loaded"
        elif [[ -f "${HOME}/Library/LaunchAgents/${label}.plist" ]]; then
            echo "  agent   ${label}: plist present, not loaded"
        else
            echo "  agent   ${label}: not installed"
        fi
    done
    echo
    if [[ -e "${MERIDIAN_DIR}/onboarded" ]]; then
        echo "verdict: ONBOARDED - the wizard will not auto-open."
    else
        echo "verdict: FIRST RUN - launching Meridian.app opens the wizard."
    fi
}

# ---------------------------------------------------------------- stop / start

stop_everything() {
    info "stopping the tray (kills the daemon watchdog first)"
    if [[ -d "${APP_PATH}" ]]; then
        osascript -e 'quit app "Meridian"' >/dev/null 2>&1 || true
    fi
    local pid
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        pid=$(tray_pid) || true
        [[ -z "${pid}" ]] && break
        sleep 0.5
    done
    if [[ -n "$(tray_pid || true)" ]]; then
        warn "tray did not quit cleanly - sending TERM"
        pkill -f "${APP_PATH}/Contents/MacOS/meridian-tray" 2>/dev/null || true
        sleep 1
    fi
    ok "tray stopped"

    # bootout, not kill: the agents are KeepAlive, so a kill just respawns them.
    for label in com.meridiona.daemon com.meridiona.a11y-helper; do
        local plist="${HOME}/Library/LaunchAgents/${label}.plist"
        if launchctl print "${GUI_TARGET}/${label}" >/dev/null 2>&1; then
            launchctl bootout "${GUI_TARGET}/${label}" 2>/dev/null \
                || launchctl bootout "${GUI_TARGET}" "${plist}" 2>/dev/null || true
            ok "booted out ${label}"
        fi
    done

    for _ in 1 2 3 4 5 6 7 8 9 10; do
        [[ -z "$(daemon_pid || true)" ]] && break
        sleep 0.5
    done
    if [[ -n "$(daemon_pid || true)" ]]; then
        warn "daemon still alive after bootout (pid $(daemon_pid)) - it will race the wipe"
    else
        ok "daemon stopped"
    fi
}

# The wipe is worthless if a writer came back and recreated state mid-flight.
# Checks only what THIS tier actually removed - a warning that fires every run
# trains you to ignore the one time it is real. (`daemon.sock` and `meridian.db`
# are in DATA_ITEMS, so --soft leaves them in place on purpose.)
assert_stayed_wiped() {
    sleep 3
    local resurrected=()
    [[ -e "${MERIDIAN_DIR}/onboarded" ]] && resurrected+=("onboarded")
    [[ -e "${MERIDIAN_DIR}/backend-version" ]] && resurrected+=("backend-version")
    if [[ "${TIER}" != "soft" ]]; then
        [[ -e "${MERIDIAN_DIR}/daemon.sock" ]] && resurrected+=("daemon.sock")
        [[ -e "${MERIDIAN_DIR}/meridian.db" ]] && resurrected+=("meridian.db")
    fi
    if (( ${#resurrected[@]} )); then
        warn "state reappeared after the wipe: ${resurrected[*]}"
        warn "something is still running - re-run --check and stop it before testing"
        return 1
    fi
    ok "wipe held (nothing recreated after 3s)"
}

# ---------------------------------------------------------------- wipes

rm_path() {
    if [[ -e "$1" || -L "$1" ]]; then
        rm -rf "$1"
        ok "removed $(basename "$1")"
    fi
}

wipe_soft() {
    for m in "${SOFT_MARKERS[@]}"; do rm_path "${MERIDIAN_DIR}/${m}"; done
}

wipe_data() {
    for m in "${DATA_ITEMS[@]}"; do rm_path "${MERIDIAN_DIR}/${m}"; done
    while IFS= read -r p; do rm_path "${p}"; done < <(app_cache_paths)
    # NOTE: the keychain entry is deliberately kept. Delete-both-or-neither —
    # a DB with no key in the keychain is unopenable and reads as corruption.
    # A fresh DB under the existing key is fine.
}

wipe_tcc() {
    info "resetting TCC grants for ${TRAY_BUNDLE_ID}"
    for svc in Accessibility ScreenCapture ListenEvent; do
        tccutil reset "${svc}" "${TRAY_BUNDLE_ID}" >/dev/null 2>&1 \
            && ok "reset ${svc}" || warn "could not reset ${svc}"
    done
}

# --purge is an `rm -rf ~/.meridian`, so it also destroys corruption forensics
# bundles and corrupt-DB backups kept from an investigation - irreplaceable, and
# nothing about resetting onboarding needs them gone. Refuse unless waved past.
guard_irreplaceable() {
    local found=()
    while IFS= read -r p; do found+=("$(basename "${p}")"); done < <(
        find "${MERIDIAN_DIR}" -maxdepth 1 \
            \( -name 'forensics-*' -o -name '*.corrupt-backup-*' \) 2>/dev/null
    )
    (( ${#found[@]} )) || return 0
    warn "~/.meridian holds investigation artifacts --full would destroy:"
    printf '      %s\n' "${found[@]}" >&2
    warn "move them elsewhere first, or re-run with MERIDIAN_RESET_FORCE=1"
    [[ "${MERIDIAN_RESET_FORCE:-0}" == "1" ]] || return 1
    warn "MERIDIAN_RESET_FORCE=1 - proceeding anyway"
}

# Shared body for --deep / --full: hand the work to the shipped uninstaller
# rather than reimplementing it, then clear the markers `data_items()` misses.
# Both scopes remove ~/.meridian/bin/meridian - the binary running this - which
# is fine (it is exec'd, not sourced), but it does mean the daemon must be
# re-staged, which relaunching Meridian.app does via backend_install.
wipe_via_uninstaller() {
    local scope="$1"
    local bin="${MERIDIAN_DIR}/bin/meridian"
    if [[ ! -x "${bin}" ]]; then
        warn "${bin} is missing - falling back to --data plus a keychain delete"
        wipe_soft; wipe_data
        security delete-generic-password -s "${KEYCHAIN_SERVICE}" -a "${KEYCHAIN_ACCOUNT}" >/dev/null 2>&1 || true
        return
    fi
    info "running: meridian uninstall ${scope} --yes"
    "${bin}" uninstall "${scope}" --yes || warn "uninstall reported an error"
    # `--remove-data` is itemized and does not know about account.json,
    # last_seen_version, backend-version, plan_auto_opened or the analytics /
    # provider caches, so it leaves the install partly onboarded. Sweep them.
    wipe_soft
}

wipe_deep() { wipe_via_uninstaller --remove-data; }

wipe_full() {
    guard_irreplaceable || return 1
    wipe_via_uninstaller --purge
}

# ---------------------------------------------------------------- main

while [[ $# -gt 0 ]]; do
    case "$1" in
        --check)    TIER="check" ;;
        --soft)     TIER="soft" ;;
        --data)     TIER="data" ;;
        --deep)     TIER="deep" ;;
        --full)     TIER="full" ;;
        --tcc)      RESET_TCC=1 ;;
        --relaunch) RELAUNCH=1 ;;
        --yes|-y)   ASSUME_YES=1 ;;
        -h|--help)  usage; exit 0 ;;
        *)          echo "unknown flag: $1" >&2; usage; exit 2 ;;
    esac
    shift
done

if [[ -z "${TIER}" ]]; then
    usage
    echo
    info "no tier given - showing current state only"
    echo
    do_check
    exit 0
fi

if [[ "${TIER}" == "check" ]]; then
    do_check
    exit 0
fi

echo "── before ───────────────────────────────────────────────────"
do_check
echo

if (( ! ASSUME_YES )); then
    echo "About to run the '--${TIER}' reset$( ((RESET_TCC)) && echo " + TCC reset")."
    read -r -p "Continue? [y/N] " reply
    [[ "${reply}" =~ ^[Yy]$ ]] || { echo "aborted."; exit 1; }
fi

stop_everything

case "${TIER}" in
    soft) wipe_soft ;;
    data) wipe_soft; wipe_data ;;
    deep) wipe_deep ;;
    full) wipe_full ;;
esac

# --deep/--full reset TCC themselves (it rides along with --remove-data).
if (( RESET_TCC )) && [[ "${TIER}" == "soft" || "${TIER}" == "data" ]]; then
    wipe_tcc
fi

assert_stayed_wiped || true

if (( RELAUNCH )); then
    if [[ -d "${APP_PATH}" ]]; then
        info "relaunching Meridian.app"
        open -a "${APP_PATH}"
    else
        warn "${APP_PATH} not found - install the DMG before testing"
    fi
fi

echo
echo "── after ────────────────────────────────────────────────────"
do_check
