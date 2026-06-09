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
                     target: daemon|daemon-error|screenpipe|screenpipe-error|ui|ui-error|mlx-server|mlx-server-error
    -f               Follow (stream)
    -n N             Last N lines (default 100)
  doctor             Run environment health checks (includes pipeline smoke)
  smoke              Dry-run both LLM pipeline stages — no DB writes
  migrate-db         Apply pending database migrations (if UI shows schema errors)
  worklog-status     Show today's PM worklogs (done/pending/drafted/posted + comments)
                     [--day YYYY-MM-DD]
  config edit        Open the repo-root .env in $EDITOR
  oauth-login jira   Connect Jira via your browser (OAuth — no API token)
  permissions        Open macOS permission panes for screenpipe
  update             Pull latest changes, rebuild, and restart (source checkout only)
  uninstall          Stop daemons and remove CLI symlinks
  version            Print installed version
  --help | -h        Show this help
EOF
    # Dev commands only make sense (and only work) in a source checkout.
    if [[ -f "${REPO_ROOT}/Cargo.toml" ]]; then
        cat <<'EOF'

Dev (source checkout — builds from this repo, MLX stays loaded):
  dev                Backing services up (bg) + UI dev server in foreground (hot reload)
  dev ui             UI dev server only — hot reload, foreground (Ctrl-C to stop)
  dev daemon         Rebuild Rust + restart the daemon (bg)   ← backend loop (2nd terminal)
  dev mlx            Restart only the MLX server (reloads the model)
  dev screenpipe     Restart only screenpipe
  dev build          Production build of daemon + UI (verify the shipped build; no run)
EOF
    fi
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
        mlx-server-error)  log_file="${LOG_DIR}/mlx-server-error.log" ;;
        *) err "unknown log target: ${target} (daemon|daemon-error|screenpipe|screenpipe-error|ui|ui-error|mlx-server|mlx-server-error)"; exit 1 ;;
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
# The daemon binary owns the comprehensive, colourised, by-daemon health table
# (system, meridian daemon, screenpipe, mlx-server, jira, ui, mcp). The wrapper
# just delegates to it; if that binary is missing or stale, a minimal bash-only
# fallback runs so `meridian doctor` always produces something useful.

_group() { printf "\n  ── %s ─────────────────────────────────────────────\n" "$1"; }

_row() {  # status check detail
    local status="$1" check="$2" detail="${3:-}" glyph
    case "$status" in
        ok)   glyph="✓" ;;
        warn) glyph="⊘" ;;
        info) glyph="·" ;;
        *)    glyph="✗"; DOCTOR_FAILURES=$(( DOCTOR_FAILURES + 1 )) ;;
    esac
    printf "  %s %-26s %s\n" "$glyph" "$check" "$detail"
}

_plist_row() {  # label check-label
    local plist="${LAUNCH_AGENTS}/$1.plist"
    if [[ -f "$plist" ]] && plutil -lint "$plist" >/dev/null 2>&1; then
        _row ok "$2" ""
    else
        _row fail "$2" "run ./install.sh"
    fi
}

_daemon_bin() {
    local p
    for p in /usr/local/bin/meridian-daemon "${HOME}/.local/bin/meridian-daemon"; do
        [[ -x "$p" ]] && { printf '%s\n' "$p"; return 0; }
    done
    return 1
}

cmd_doctor() {
    local bin rc=0
    if bin="$(_daemon_bin)"; then
        set +e
        if [[ "$*" == *--fix* ]]; then
            # --fix has interactive guided prompts — the user is present, so run
            # without the alarm (which would kill a prompt waiting for input).
            "$bin" doctor "$@"
            rc=$?
        else
            # Guard with a perl alarm so a stale binary (one that predates
            # `doctor` and would fall through to starting the daemon) can never
            # hang the terminal. The Rust report colourises itself on a tty.
            perl -e 'alarm shift @ARGV; exec @ARGV' 30 "$bin" doctor "$@"
            rc=$?
        fi
        set -e
        # 0 = healthy, 1 = critical issues found — both are real doctor runs.
        if [[ $rc -eq 0 || $rc -eq 1 ]]; then
            # Append classification smoke (fast path, ~30s max). Failures are
            # informational — they don't override the doctor exit code, since the
            # doctor already surfaces the MLX health state.
            cmd_smoke --classify-only || true
            return $rc
        fi
        warn "health engine timed out or is stale — rebuild: cargo build --release"
    fi
    _doctor_fallback
    cmd_smoke --classify-only || true
}

# Minimal bash-only checks for when the daemon binary is unavailable.
_doctor_fallback() {
    DOCTOR_FAILURES=0
    printf "\n  Meridian doctor (fallback — daemon binary unavailable)\n"
    printf "  ════════════════════════════════════════════════════════\n"
    _group "system"
    _row "$([[ "$(uname -s)" == "Darwin" ]] && echo ok || echo fail)" "macOS" ""
    _row "$([[ -f "${REPO_ROOT}/.env" ]] && echo ok || echo fail)" "config (.env)" ""
    _group "services (plists)"
    _plist_row "$LABEL_DAEMON" "daemon plist"
    _plist_row "$LABEL_SCREENPIPE" "screenpipe plist"
    _plist_row "$LABEL_MLX" "mlx plist"
    _plist_row "$LABEL_UI" "ui plist"
    _group "builds"
    _row "$([[ -f "${REPO_ROOT}/packages/meridian-mcp/dist/index.js" ]] && echo ok || echo fail)" "mcp built" ""
    _row "$([[ -d "${REPO_ROOT}/ui/.next" ]] && echo ok || echo fail)" "ui built" ""
    echo
    _row info "next step" "cargo build --release && meridian doctor"
    [[ $DOCTOR_FAILURES -eq 0 ]]
}

# --- smoke (pipeline dry run) ---
# Sends synthetic requests (no DB writes) to both LLM stages:
#   --classify-only  fast path (~30s max) called automatically from cmd_doctor
#   (no flag)        full run: classification + worklog synthesis

_smoke_read_env() {
    local key="$1" env_file="${REPO_ROOT}/.env"
    [[ -f "$env_file" ]] || return 0
    grep -E "^${key}=" "$env_file" 2>/dev/null | tail -1 | cut -d= -f2- || true
}

_smoke_row() {  # glyph ansi-color label detail
    local glyph="$1" color="$2" label="$3" detail="${4:-}"
    if [[ -t 1 ]]; then
        printf "    \033[%sm%s\033[0m  %-26s \033[2m%s\033[0m\n" "$color" "$glyph" "$label" "$detail"
    else
        printf "    %s  %-26s %s\n" "$glyph" "$label" "$detail"
    fi
}

_smoke_remedy() {
    local msg="$1"
    if [[ -t 1 ]]; then printf "       \033[2m→ %s\033[0m\n" "$msg"
    else printf "       → %s\n" "$msg"; fi
}

cmd_smoke() {
    local classify_only=0
    [[ "${1:-}" == "--classify-only" ]] && classify_only=1

    local mlx_port
    mlx_port="$(_smoke_read_env MLX_SERVER_PORT)"
    mlx_port="${mlx_port:-7823}"
    local base="http://127.0.0.1:${mlx_port}"
    local classify_timeout=60
    [[ $classify_only -eq 1 ]] && classify_timeout=30
    local all_ok=1

    if [[ -t 1 ]]; then
        printf "\n  \033[36m▸ smoke (pipeline dry run)\033[0m\n"
        printf "  \033[2m%s\033[0m\n" "════════════════════════════════════════════════════════"
    else
        printf "\n  ▸ smoke (pipeline dry run)\n"
        printf "  %s\n" "════════════════════════════════════════════════════════"
    fi

    # Quick reachability probe — if the server isn't up, nothing else can run.
    local reach_ok=0
    set +e
    curl -sf --max-time 5 "${base}/health" >/dev/null 2>&1 && reach_ok=1
    set -e
    if [[ $reach_ok -eq 0 ]]; then
        _smoke_row "✗" "31" "mlx reachable" "server not responding at ${base}"
        _smoke_remedy "meridian start  (or: meridian logs mlx-server)"
        echo ""
        return 1
    fi

    # Stage 1: classification smoke.
    # POST /classify takes {"input":"..."} — pure model inference, zero DB access.
    local t0 classify_resp classify_ok=0
    t0=$SECONDS
    set +e
    classify_resp="$(curl -sf --max-time "${classify_timeout}" \
        -X POST "${base}/classify" \
        -H "Content-Type: application/json" \
        -d '{"input":"App: Xcode\nWindow: ContentView.swift — MyApp\nOCR: func body: some View { Text(\"Hello World\") }\nDuration: 600s"}' \
        2>/dev/null)"
    local classify_curl_rc=$?
    set -e
    local classify_elapsed=$(( SECONDS - t0 ))

    if [[ $classify_curl_rc -ne 0 || -z "$classify_resp" ]]; then
        _smoke_row "✗" "31" "classification" "no response from /classify (timeout or error)"
        _smoke_remedy "check: meridian logs mlx-server"
        all_ok=0
    else
        local stype conf
        stype="$(printf '%s' "$classify_resp" | grep -o '"session_type":"[^"]*"' | cut -d'"' -f4)" || stype=""
        conf="$(printf '%s' "$classify_resp" | grep -o '"confidence":[0-9.]*' | cut -d: -f2)" || conf="?"
        if [[ -n "$stype" ]]; then
            _smoke_row "✓" "32" "classification" "${classify_elapsed}s  session_type=${stype}  conf=${conf}"
            classify_ok=1
        else
            _smoke_row "✗" "31" "classification" "response did not parse — got: ${classify_resp:0:80}"
            _smoke_remedy "restart MLX server: meridian dev mlx  (or: meridian restart)"
            all_ok=0
        fi
    fi

    # Fast path (called from cmd_doctor): stop here.
    if [[ $classify_only -eq 1 ]]; then
        echo ""
        [[ $classify_ok -eq 1 ]]
        return
    fi

    # Stage 2: worklog synthesis smoke.
    # POST /synthesise_worklog with a synthetic bundle — the agno agent runs the model
    # and returns a JiraUpdate. Nothing is written to the DB; Rust never sees this call.
    local jira_url jira_token linear_key github_token has_pm=0
    jira_url="$(_smoke_read_env JIRA_BASE_URL)"
    [[ -z "$jira_url" ]] && jira_url="$(_smoke_read_env JIRA_URL)"
    jira_token="$(_smoke_read_env JIRA_API_TOKEN)"
    linear_key="$(_smoke_read_env LINEAR_API_KEY)"
    github_token="$(_smoke_read_env GITHUB_TOKEN)"
    [[ -n "$jira_url" && -n "$jira_token" ]] && has_pm=1
    [[ -n "$linear_key" ]] && has_pm=1
    [[ -n "$github_token" ]] && has_pm=1

    if [[ $has_pm -eq 0 ]]; then
        _smoke_row "·" "2" "worklog synthesis" "skipped — no PM credentials in .env"
        echo ""
        [[ $all_ok -eq 1 ]]
        return
    fi

    # Dates are fixed to 2024-01-01 so the output is obviously synthetic.
    local synth_bundle
    synth_bundle='{"bundle":{"task_key":"SMOKE-1","window_start":"2024-01-01T09:00:00","window_end":"2024-01-01T09:30:00","cycle_index":0,"sessions":[{"id":1,"app_name":"Xcode","started_at":"2024-01-01T09:00:00","ended_at":"2024-01-01T09:30:00","duration_s":1800,"idle_frame_s":0,"top_titles":["ContentView.swift — MyApp"],"excerpt":"Implementing SwiftUI body layout. func body: some View { Text(\"Hello World\") }","category":"coding"}],"total_seconds":1800,"real_seconds":1800,"pm_task_title":"Implement ContentView layout"}}'

    local t1 synth_resp synth_ok=0
    t1=$SECONDS
    set +e
    synth_resp="$(curl -sf --max-time 120 \
        -X POST "${base}/synthesise_worklog" \
        -H "Content-Type: application/json" \
        -d "$synth_bundle" \
        2>/dev/null)"
    local synth_curl_rc=$?
    set -e
    local synth_elapsed=$(( SECONDS - t1 ))

    if [[ $synth_curl_rc -ne 0 || -z "$synth_resp" ]]; then
        _smoke_row "✗" "31" "worklog synthesis" "no response from /synthesise_worklog (timeout or error)"
        _smoke_remedy "check: meridian logs mlx-server"
        all_ok=0
    elif printf '%s' "$synth_resp" | grep -q '"summary"'; then
        local bullets conf2
        bullets="$(printf '%s' "$synth_resp" | grep -o '"text":' | wc -l | tr -d ' ')" || bullets="?"
        conf2="$(printf '%s' "$synth_resp" | grep -o '"confidence":[0-9.]*' | cut -d: -f2)" || conf2="?"
        _smoke_row "✓" "32" "worklog synthesis" "${synth_elapsed}s  bullets=${bullets}  conf=${conf2}"
        synth_ok=1
    else
        _smoke_row "✗" "31" "worklog synthesis" "response missing summary — got: ${synth_resp:0:80}"
        _smoke_remedy "restart MLX server: meridian dev mlx  (or: meridian restart)"
        all_ok=0
        synth_ok=0  # explicitly mark unused var for clarity
        : "$synth_ok"
    fi

    echo ""
    [[ $all_ok -eq 1 ]]
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

# --- update ---
cmd_update() {
    if _is_source_checkout; then
        info "pulling latest changes…"
        git -C "${REPO_ROOT}" pull --ff-only || {
            err "git pull failed — resolve conflicts manually, then run 'meridian dev build' and 'meridian restart'"
            exit 1
        }
        info "rebuilding daemon (cargo --release)…"
        ( cd "${REPO_ROOT}" && cargo build --release )
        info "rebuilding UI…"
        ( cd "${REPO_ROOT}/ui" && npm run build )
        info "restarting daemons…"
        cmd_restart
        ok "updated to $(cat "${REPO_ROOT}/VERSION" 2>/dev/null || git -C "${REPO_ROOT}" rev-parse --short HEAD)"
    else
        err "meridian update is not available in a source checkout context."
        err "Run: npm install -g @meridiona/meridian@latest"
        exit 1
    fi
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

# Subcommands implemented by the daemon binary itself (not the launchd manager).
# Forward these straight through to `meridian-daemon <cmd> [args]`.
cmd_daemon_passthrough() {
    local bin=""
    for p in /usr/local/bin/meridian-daemon "${HOME}/.local/bin/meridian-daemon"; do
        [[ -x "$p" ]] && bin="$p" && break
    done
    [[ -n "$bin" ]] || { err "meridian-daemon binary not found — run ./install.sh"; exit 1; }
    exec "$bin" "$@"
}

# --- dev (source checkout only) ---
# Build from THIS repo and restart only what changed. The daemon's launchd plist
# runs ~/.local/bin/meridian-daemon -> target/release/meridian, so a release
# build is picked up in place. Gated on Cargo.toml: a prebuilt install (bundle at
# ~/.meridian/app) has no source, so these are hidden/disabled there.
_is_source_checkout() { [[ -f "${REPO_ROOT}/Cargo.toml" ]]; }

# Bring a service up-to-date: ensure it's loaded (enable + bootstrap), then
# (re)start it so a fresh build is picked up. Works whether it was stopped,
# booted out, or already running — the same sequence cmd_start uses.
_dev_up() {
    local label="$1"
    local plist="${LAUNCH_AGENTS}/${label}.plist"
    if [[ ! -f "$plist" ]]; then
        warn "${label} not installed — run ./install.sh"
        return 0
    fi
    set +e
    launchctl enable "${GUI_TARGET}/${label}" 2>/dev/null
    launchctl bootstrap "${GUI_TARGET}" "$plist" 2>/dev/null
    launchctl kickstart -k "${GUI_TARGET}/${label}" 2>/dev/null
    local rc=$?
    set -e
    if [[ $rc -eq 0 ]]; then
        ok "(re)started ${label}"
    else
        warn "${label} failed to start — check: meridian logs ${label#com.meridiona.}"
    fi
}

# Start a service ONLY if it isn't already up — never reloads a live process
# (so the ~6 GB MLX model isn't reloaded when it's already serving).
_dev_ensure() {
    local label="$1"
    if launchctl print "${GUI_TARGET}/${label}" >/dev/null 2>&1; then
        ok "${label} already up (left as-is)"
    else
        _dev_up "$label"
    fi
}

# The daemon hard-exits if the MLX server isn't reachable, so wait for /health
# before (re)starting it. Returns immediately if MLX is already serving.
_dev_wait_mlx() {
    local port="${MLX_SERVER_PORT:-7823}" w=0
    info "waiting for MLX server (port ${port}) to answer…"
    until curl -sf "http://127.0.0.1:${port}/health" >/dev/null 2>&1; do
        sleep 2; w=$((w+2))
        if [[ $w -ge 120 ]]; then
            warn "MLX not ready after 120s — daemon will retry on its own (KeepAlive)"
            return 0
        fi
    done
    ok "MLX ready (${w}s)"
}

_dev_build_daemon() { info "building daemon (cargo --release)…"; ( cd "${REPO_ROOT}" && cargo build --release ); }
_dev_build_ui()     { info "building UI (npm run build)…";       ( cd "${REPO_ROOT}/ui" && npm run build ); }

# Stop the launchd (production) dashboard so `next dev` can bind its port.
# Disable too, so KeepAlive doesn't race to relaunch the prod server.
_dev_stop_prod_ui() {
    if launchctl print "${GUI_TARGET}/${LABEL_UI}" >/dev/null 2>&1; then
        set +e
        launchctl disable "${GUI_TARGET}/${LABEL_UI}" 2>/dev/null
        launchctl bootout  "${GUI_TARGET}/${LABEL_UI}" 2>/dev/null
        set -e
        info "stopped launchd dashboard (freeing the port for the dev server)"
    fi
}

# Run the Next.js dev server in the FOREGROUND (hot reload). Replaces this shell
# (exec), so Ctrl-C stops just the UI server — backing services keep running.
# Re-enable the prod dashboard later with `meridian start`.
_dev_ui_server() {
    local port="${MERIDIAN_UI_PORT:-3939}"
    _dev_stop_prod_ui
    echo
    info "UI dev server (hot reload) → http://localhost:${port}   ·   Ctrl-C to stop"
    info "edit-and-save reflects instantly; backing services keep running in the background"
    echo
    cd "${REPO_ROOT}/ui" || { err "ui/ not found at ${REPO_ROOT}/ui"; exit 1; }
    if [[ -x ./node_modules/.bin/next ]]; then
        exec ./node_modules/.bin/next dev --turbopack -p "${port}"
    else
        warn "next not found — run 'cd ui && npm install' first"; exec npm run dev
    fi
}

cmd_migrate_db() {
    local db="${HOME}/.meridian/meridian.db"

    if [[ ! -f "${db}" ]]; then
        err "database not found at ${db}"
        exit 1
    fi

    info "Checking database schema…"
    local has_claude_uuid
    has_claude_uuid=$(sqlite3 "${db}" ".schema app_sessions" 2>/dev/null | grep -c "claude_session_uuid" || echo "0")

    if [[ "${has_claude_uuid}" -gt 0 ]]; then
        ok "database schema is up-to-date"
        exit 0
    fi

    info "Database schema is incomplete — applying migrations…"
    info "Stopping daemon (to prevent locks)…"
    launchctl bootout "${GUI_TARGET}/${LABEL_DAEMON}" 2>/dev/null || true
    sleep 2

    local backup="${db}.backup.$(date +%s)"
    info "Backing up database to ${backup}…"
    cp "${db}" "${backup}"
    ok "Backup created"

    info "Running daemon to apply migrations (this may take 10-30 seconds)…"
    local daemon_bin
    daemon_bin="$(_daemon_bin)" || { err "daemon binary not found"; exit 1; }

    # Run the daemon in the background; it will apply migrations on startup
    set +e
    timeout 60 "${daemon_bin}" >/dev/null 2>&1 &
    local daemon_pid=$!
    sleep 15  # Give it time to initialize and apply migrations
    kill $daemon_pid 2>/dev/null || true
    set -e

    # Verify migrations applied
    info "Verifying schema…"
    has_claude_uuid=$(sqlite3 "${db}" ".schema app_sessions" 2>/dev/null | grep -c "claude_session_uuid" || echo "0")

    if [[ "${has_claude_uuid}" -gt 0 ]]; then
        ok "migrations applied successfully"
        info "Restarting daemon…"
        launchctl enable "${GUI_TARGET}/${LABEL_DAEMON}" 2>/dev/null || true
        launchctl kickstart -k "${GUI_TARGET}/${LABEL_DAEMON}" 2>/dev/null || true
        ok "Done. The UI should now work."
        exit 0
    else
        err "migrations failed — database not updated"
        info "To restore the backup: cp ${backup} ${db}"
        exit 1
    fi
}

cmd_dev() {
    if ! _is_source_checkout; then
        err "'meridian dev' needs a source checkout (no Cargo.toml at ${REPO_ROOT})."
        err "This is a prebuilt install — use start / stop / restart / status / logs."
        exit 1
    fi
    local target="${1:-all}"
    case "$target" in
        all)
            # Dev session: backing services in the background (start screenpipe/
            # MLX only if down — don't reload a live model — rebuild & restart the
            # daemon), then the UI dev server in the FOREGROUND (hot reload). Wait
            # for MLX before the daemon (it hard-exits if MLX is unreachable).
            _dev_build_daemon
            _dev_ensure "${LABEL_SCREENPIPE}"
            _dev_ensure "${LABEL_MLX}"
            _dev_wait_mlx
            _dev_up "${LABEL_DAEMON}"
            _dev_ui_server      # foreground (exec) — runs until Ctrl-C
            ;;
        ui)         _dev_ui_server ;;                 # UI dev server only (foreground, hot reload)
        daemon)     _dev_build_daemon; _dev_wait_mlx; _dev_up "${LABEL_DAEMON}" ;;
        mlx)        _dev_up "${LABEL_MLX}" ;;          # python — restart reloads the model
        screenpipe) _dev_up "${LABEL_SCREENPIPE}" ;;
        build)      _dev_build_daemon; _dev_build_ui; ok "built production bundles (no run)" ;;
        *) err "unknown dev target: ${target}";
           echo "  targets: all | ui | daemon | mlx | screenpipe | build"; exit 1 ;;
    esac
}

case "$CMD" in
    start)            cmd_start ;;
    stop)             cmd_stop ;;
    restart)          cmd_restart ;;
    status)           cmd_status ;;
    logs)             cmd_logs "$@" ;;
    doctor)           cmd_doctor "$@" ;;
    smoke)            cmd_smoke "$@" ;;
    migrate-db)       cmd_migrate_db "$@" ;;
    config)           cmd_config "$@" ;;
    dev)              cmd_dev "$@" ;;
    update)           cmd_update ;;
    uninstall)        cmd_uninstall ;;
    permissions)      cmd_permissions ;;
    version|--version|-v) cat "${REPO_ROOT}/VERSION" 2>/dev/null || echo "unknown" ;;
    worklog-status|pm-worklog|coding-agent-hook|coding-agent-summarise|coding-agent-classify|coding-agent-install-skill|oauth-login|tasks-sync) cmd_daemon_passthrough "$CMD" "$@" ;;
    --help|-h|help|"") cmd_help ;;
    *) err "unknown command: ${CMD}"; echo; cmd_help; exit 1 ;;
esac
