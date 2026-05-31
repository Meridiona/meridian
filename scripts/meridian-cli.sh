#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions

set -euo pipefail

# --- repo root resolution (works when invoked via symlink) ---
SELF="$(readlink "$0" 2>/dev/null || echo "$0")"
case "$SELF" in /*) ;; *) SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")" ;; esac
REPO_ROOT="$(cd "$(dirname "$SELF")/.." && pwd)"

# --- constants ---
LABEL_SCREENPIPE="com.meridiona.screenpipe"
LABEL_DAEMON="com.meridiona.daemon"
LABEL_UI="com.meridiona.ui"
LABEL_MLX="com.meridiona.mlx-server"
# Jira worklogs and coding-agent ingest run inside the Rust daemon — no
# separate launchd agents. Only these four are managed.
readonly LABELS=("${LABEL_SCREENPIPE}" "${LABEL_DAEMON}" "${LABEL_UI}" "${LABEL_MLX}")
GUI_TARGET="gui/$(id -u)"
LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
LOG_DIR="${HOME}/.meridian/logs"

# --- output helpers ---
info()  { printf "→ %s\n"   "$*"; }
ok()    { printf "✓ %s\n"   "$*"; }
warn()  { printf "⊘ %s\n"   "$*"; }
err()   { printf "✗ %s\n"   "$*" >&2; }

# --- usage ---
cmd_help() {
    cat <<'EOF'
meridian — Meridian daemon manager

Usage:
  meridian <command> [options]

Commands:
  start              Start all daemons (screenpipe, daemon, ui, mlx-server)
  stop               Stop all daemons (also kills orphaned mlx_lm.server processes)
  restart            Stop, wait 1s, start
  status             Show running state of all daemons
  logs [target]      Tail log files
                     target: daemon|daemon-error|screenpipe|screenpipe-error|ui|ui-error|mlx-server
    -f               Follow (stream)
    -n N             Last N lines (default 100)
  doctor             Run environment health checks
  config edit        Open the repo-root .env in $EDITOR
  permissions        Open macOS permission panes for screenpipe
  uninstall          Stop daemons and remove CLI symlinks
  --help | -h        Show this help
EOF
}

# --- start ---
cmd_start() {
    local any_missing=0
    for label in "${LABELS[@]}"; do
        local plist="${LAUNCH_AGENTS}/${label}.plist"
        if [[ ! -f "$plist" ]]; then
            err "${label} not installed — run ./install.sh"
            any_missing=1
        else
            set +e
            launchctl enable "${GUI_TARGET}/${label}" 2>/dev/null
            launchctl bootstrap "${GUI_TARGET}" "$plist" 2>/dev/null
            launchctl kickstart -k "${GUI_TARGET}/${label}" 2>/dev/null
            set -e
            info "started ${label}"
        fi
    done
    echo
    cmd_status
    [[ $any_missing -eq 0 ]] || exit 1
}

# --- stop ---
# launchctl disable clears the KeepAlive intent so launchd won't auto-restart.
# bootout removes the service from launchd entirely; the agent stays disabled
# until cmd_start re-enables and re-bootstraps it. The plist file is untouched.
cmd_stop() {
    for label in "${LABELS[@]}"; do
        set +e
        launchctl disable "${GUI_TARGET}/${label}" 2>/dev/null
        launchctl bootout "${GUI_TARGET}/${label}" 2>/dev/null
        set -e
        info "stopped ${label}"
    done
    # Kill any orphaned mlx_lm.server processes spawned by the old LLM selector
    # script (not tracked by launchd — must be killed directly).
    if pgrep -f "mlx_lm.server" >/dev/null 2>&1; then
        pkill -f "mlx_lm.server" 2>/dev/null || true
        info "killed orphaned mlx_lm.server process(es)"
    fi
}

# --- restart ---
cmd_restart() {
    cmd_stop
    sleep 1
    cmd_start
}

# --- status ---
_label_status() {
    local label="$1"
    local plist="${LAUNCH_AGENTS}/${label}.plist"
    local output
    set +e
    output="$(launchctl print "${GUI_TARGET}/${label}" 2>/dev/null)"
    local rc=$?
    set -e
    if [[ $rc -eq 0 ]]; then
        local pid
        pid="$(printf '%s\n' "$output" | grep -E '^\s+pid\s*=' | head -1 | grep -oE '[0-9]+')" || true
        if [[ -n "$pid" ]]; then
            ok "${label} running (pid ${pid})"
        else
            warn "${label} loaded but not running"
        fi
    elif [[ -f "$plist" ]]; then
        warn "${label} loaded but not running"
    else
        err "${label} not installed"
    fi
}

cmd_status() {
    for label in "${LABELS[@]}"; do
        _label_status "$label"
    done
}

# --- logs ---
cmd_logs() {
    local target="daemon"
    local follow=0
    local lines=100

    # consume target if it's not a flag
    if [[ $# -gt 0 && "${1:-}" != -* ]]; then
        target="$1"; shift
    fi

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -f) follow=1; shift ;;
            -n) lines="${2:?-n requires a value}"; shift 2 ;;
            *) err "unknown option: $1"; exit 1 ;;
        esac
    done

    local log_file
    case "$target" in
        daemon)            log_file="${LOG_DIR}/daemon.log" ;;
        daemon-error)      log_file="${LOG_DIR}/daemon-error.log" ;;
        screenpipe)        log_file="${LOG_DIR}/screenpipe.log" ;;
        screenpipe-error)  log_file="${LOG_DIR}/screenpipe-error.log" ;;
        ui)                log_file="${LOG_DIR}/ui.log" ;;
        ui-error)          log_file="${LOG_DIR}/ui-error.log" ;;
        mlx-server)        log_file="${LOG_DIR}/mlx-server.log" ;;
        *) err "unknown log target: ${target} (daemon|daemon-error|screenpipe|screenpipe-error|ui|ui-error|mlx-server)"; exit 1 ;;
    esac

    if [[ ! -f "$log_file" ]]; then
        err "no log file at ${log_file}"
        exit 1
    fi

    if [[ $follow -eq 1 ]]; then
        tail -n "$lines" -f "$log_file"
    else
        tail -n "$lines" "$log_file"
    fi
}

# --- doctor ---
_check() {
    local desc="$1" pass="$2" reason="${3:-}"
    if [[ "$pass" == "1" ]]; then
        ok "$desc"
    else
        err "$desc${reason:+ — ${reason}}"
        DOCTOR_FAILURES=$(( DOCTOR_FAILURES + 1 ))
    fi
}

_pid_from_print() {
    local label="$1"
    local output
    set +e
    output="$(launchctl print "${GUI_TARGET}/${label}" 2>/dev/null)"
    local rc=$?
    set -e
    [[ $rc -ne 0 ]] && return 1
    printf '%s\n' "$output" | grep -E '^\s+pid\s*=' | grep -oE '[0-9]+' | head -1
}

cmd_doctor() {
    DOCTOR_FAILURES=0

    # 1. macOS
    _check "macOS" "$([[ "$(uname -s)" == "Darwin" ]] && echo 1 || echo 0)" "run on macOS"

    # 2. daemon binary
    local bin_ok=0
    for p in /usr/local/bin/meridian-daemon "${HOME}/.local/bin/meridian-daemon"; do
        [[ -x "$p" ]] && bin_ok=1 && break
    done
    _check "daemon binary exists and is executable" "$bin_ok" "run ./install.sh"

    # 3. daemon plist lints
    local dplist="${LAUNCH_AGENTS}/${LABEL_DAEMON}.plist"
    if [[ -f "$dplist" ]]; then
        set +e; plutil -lint "$dplist" >/dev/null 2>&1; local pl=$?; set -e
        _check "daemon plist installed and valid" "$([[ $pl -eq 0 ]] && echo 1 || echo 0)" "plutil -lint ${dplist}"
    else
        _check "daemon plist installed and valid" "0" "run ./install.sh"
    fi

    # 4. daemon running
    local dpid; dpid="$(_pid_from_print "$LABEL_DAEMON" 2>/dev/null)" || dpid=""
    _check "daemon running (pid ${dpid:-?})" "$([[ -n "$dpid" ]] && echo 1 || echo 0)" "meridian start"

    # 5. user config
    _check "user config <repo>/.env exists" "$([[ -f "${REPO_ROOT}/.env" ]] && echo 1 || echo 0)" "run ./install.sh"

    # 6. screenpipe plist lints
    local spplist="${LAUNCH_AGENTS}/${LABEL_SCREENPIPE}.plist"
    if [[ -f "$spplist" ]]; then
        set +e; plutil -lint "$spplist" >/dev/null 2>&1; local spl=$?; set -e
        _check "screenpipe plist installed and valid" "$([[ $spl -eq 0 ]] && echo 1 || echo 0)" "plutil -lint ${spplist}"
    else
        _check "screenpipe plist installed and valid" "0" "run ./install.sh"
    fi

    # 7. screenpipe binary in PATH
    set +e; command -v screenpipe >/dev/null 2>&1; local spbin=$?; set -e
    _check "screenpipe binary in PATH" "$([[ $spbin -eq 0 ]] && echo 1 || echo 0)" "install screenpipe (npm install -g screenpipe)"

    # 8. screenpipe DB
    _check "screenpipe DB exists" "$([[ -f "${HOME}/.screenpipe/db.sqlite" ]] && echo 1 || echo 0)" "install and run screenpipe"

    # 9. screenpipe running
    set +e; pgrep -x screenpipe >/dev/null 2>&1; local sp=$?; set -e
    _check "screenpipe running" "$([[ $sp -eq 0 ]] && echo 1 || echo 0)" "start screenpipe"

    # 10. meridian DB
    if [[ -f "${HOME}/.meridian/meridian.db" ]]; then
        ok "meridian DB exists"
    else
        warn "meridian DB not yet created (will be on first run)"
    fi

    # 11. Python venv
    local venv_py="${REPO_ROOT}/services/.venv/bin/python"
    local venv_ok=0
    if [[ -x "$venv_py" ]]; then
        set +e; "$venv_py" -c "import run_agent" 2>/dev/null; local vi=$?; set -e
        [[ $vi -eq 0 ]] && venv_ok=1
    fi
    _check "Python venv and run_agent importable" "$venv_ok" "bash scripts/setup-services.sh"

    # 12. MCP server built
    _check "MCP server built" "$([[ -f "${REPO_ROOT}/packages/meridian-mcp/dist/index.js" ]] && echo 1 || echo 0)" "cd packages/meridian-mcp && npm run build"

    # 13. UI plist lints
    local uiplist="${LAUNCH_AGENTS}/${LABEL_UI}.plist"
    if [[ -f "$uiplist" ]]; then
        set +e; plutil -lint "$uiplist" >/dev/null 2>&1; local uil=$?; set -e
        _check "UI plist installed and valid" "$([[ $uil -eq 0 ]] && echo 1 || echo 0)" "plutil -lint ${uiplist}"
    else
        _check "UI plist installed and valid" "0" "run ./install.sh"
    fi

    # 14. UI built
    _check "UI built (ui/.next exists)" "$([[ -d "${REPO_ROOT}/ui/.next" ]] && echo 1 || echo 0)" "cd ui && npm ci && npm run build"

    echo
    if [[ $DOCTOR_FAILURES -eq 0 ]]; then
        ok "all checks passed"
    else
        printf "  %d check%s failed\n" "$DOCTOR_FAILURES" "$([[ $DOCTOR_FAILURES -ne 1 ]] && echo s || true)"
    fi
}

# --- config ---
cmd_config() {
    local subcmd="${1:-}"
    if [[ "$subcmd" != "edit" ]]; then
        err "usage: meridian config edit"
        exit 1
    fi
    local env_file="${REPO_ROOT}/.env"
    if [[ ! -f "$env_file" ]]; then
        err "${env_file} not found — run ./install.sh first"
        exit 1
    fi
    "${EDITOR:-nano}" "$env_file"
}

# --- uninstall ---
cmd_uninstall() {
    local ans
    read -r -p "This will stop all daemons and remove /usr/local/bin/meridian. Continue? [y/N] " ans
    if [[ "$ans" != "y" && "$ans" != "Y" ]]; then
        printf "(cancelled)\n"
        exit 0
    fi

    set +e
    bash "${REPO_ROOT}/scripts/uninstall-ui-daemon.sh" 2>/dev/null
    bash "${REPO_ROOT}/services/scripts/uninstall-mlx-server-daemon.sh" 2>/dev/null
    bash "${REPO_ROOT}/scripts/uninstall-daemon.sh" 2>/dev/null
    bash "${REPO_ROOT}/scripts/uninstall-screenpipe-daemon.sh" 2>/dev/null
    pkill -f "mlx_lm.server" 2>/dev/null || true
    rm -f /usr/local/bin/meridian /usr/local/bin/meridian-daemon \
          "${HOME}/.local/bin/meridian" "${HOME}/.local/bin/meridian-daemon"
    set -e

    ok "uninstalled"
    printf "  Data at ~/.meridian/ was not removed. Delete it manually if desired.\n"
}

# --- permissions ---
cmd_permissions() {
    local sp_bin
    sp_bin="$(command -v screenpipe 2>/dev/null || echo "/opt/homebrew/bin/screenpipe")"
    info "screenpipe needs three macOS permissions to record activity"
    echo "    binary path: ${sp_bin}"
    echo
    echo "    Screen Recording + Accessibility panes: click '+', navigate to the"
    echo "    binary path above, add it to the list, and toggle it ON."
    echo "    Microphone pane has no '+'. screenpipe will appear there only after"
    echo "    it tries to use the mic — then toggle it ON. If it isn't listed yet,"
    echo "    grant Screen Recording first and screenpipe will request mic access."
    echo
    read -r -p "  Press Enter to open Screen Recording pane (1/3)… " _
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
    read -r -p "  Press Enter when Screen Recording is granted… " _
    ok "Screen Recording acknowledged"
    read -r -p "  Press Enter to open Accessibility pane (2/3)… " _
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    read -r -p "  Press Enter when Accessibility is granted… " _
    ok "Accessibility acknowledged"
    read -r -p "  Press Enter to open Microphone pane (3/3, optional)… " _
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
    read -r -p "  Press Enter when Microphone is granted (or skip if screenpipe isn't listed yet)… " _
    ok "Microphone acknowledged"
    info "After granting permissions, restart screenpipe:"
    echo "    meridian restart"
}

# --- dispatch ---
CMD="${1:-}"
shift || true

case "$CMD" in
    start)            cmd_start ;;
    stop)             cmd_stop ;;
    restart)          cmd_restart ;;
    status)           cmd_status ;;
    logs)             cmd_logs "$@" ;;
    doctor)           cmd_doctor ;;
    config)           cmd_config "$@" ;;
    uninstall)        cmd_uninstall ;;
    permissions)      cmd_permissions ;;
    --help|-h|help|"") cmd_help ;;
    *) err "unknown command: ${CMD}"; echo; cmd_help; exit 1 ;;
esac
