#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

set -euo pipefail

# --- repo root resolution (works when invoked via symlink) ---
SELF="$(readlink "$0" 2>/dev/null || echo "$0")"
case "$SELF" in /*) ;; *) SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")" ;; esac
REPO_ROOT="$(cd "$(dirname "$SELF")/.." && pwd)"

# --- constants ---
LABEL_DAEMON="com.meridiona.daemon"
# Capture runs in-process in the tray (no screenpipe launchd agent since Bucket-2).
# Jira worklogs and coding-agent ingest run inside the Rust daemon, and generation
# goes through the user's chosen AI CLI — no separate launchd agents. Only the
# daemon is managed here.
readonly LABELS=("${LABEL_DAEMON}")
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
  start              Start the Rust daemon
  stop               Stop the Rust daemon
  restart            Stop, wait 1s, start
  status             Show running state
  logs [target]      Tail log files
                     target: daemon|daemon-error|tray|tray-error
    -f               Follow (stream)
    -n N             Last N lines (default 100)
  doctor             Run environment health checks
  migrate-db         Apply pending database migrations (if UI shows schema errors)
  worklog-status     Show today's PM worklogs (done/pending/drafted/posted + comments)
                     [--day YYYY-MM-DD]
  config edit        Open the repo-root .env in $EDITOR
  oauth-login jira   Connect Jira via your browser (OAuth — no API token)
  permissions        Open macOS permission panes for Meridian
  update             Pull latest changes, rebuild, and restart (source checkout only)
  uninstall          Stop the daemon and remove CLI symlinks
  version            Print installed version
  --help | -h        Show this help
EOF
    # Dev commands only make sense (and only work) in a source checkout.
    if [[ -f "${REPO_ROOT}/Cargo.toml" ]]; then
        cat <<'EOF'

Dev (source checkout — builds from this repo):
  dev                Start the full dev environment: Rust daemon (cargo-watch,
                     rebuilds on save) + Tauri tray (hot reload) in 2 windows
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
    # Kill any orphaned dev-build daemons (`cargo watch`/`cargo run --bin
    # meridian` from dev-start.sh, or a bare `cargo run`) — not tracked by
    # launchd, so `bootout` above never touches them. Closing the Terminal
    # window a dev session runs in (instead of Ctrl-C, or re-running
    # dev-start.sh, which has its own cleanup) reparents this chain to PID 1
    # and it silently keeps running — including its clock-aligned worklog
    # trigger, which then races the canonical daemon and double-runs every
    # hour's worklog pipeline. Anchored to the debug/release binary path so
    # this never matches the canonical `~/.meridian/bin/meridian` or the
    # `meridian`/`meridian-daemon` CLI wrappers.
    if pgrep -f "target/(debug|release)/meridian$" >/dev/null 2>&1; then
        pkill -f "target/(debug|release)/meridian$" 2>/dev/null || true
        info "killed orphaned dev-build daemon process(es)"
    fi
    if pgrep -f "cargo-watch.*--bin meridian" >/dev/null 2>&1; then
        pkill -f "cargo-watch.*--bin meridian" 2>/dev/null || true
        info "killed orphaned cargo-watch process(es)"
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
# Resolve the compiled `meridian` binary (distinct from THIS bash wrapper,
# which is what gets invoked as `meridian` on PATH). Mirrors
# tray/src-tauri/src/install.rs's `meridian_bin()`: packaged native locations
# first, a dev build only for a source checkout.
_meridian_native_bin() {
    local p
    for p in "${HOME}/.meridian/bin/meridian" "${HOME}/.meridian/app/bin/meridian"; do
        [[ -x "$p" ]] && { echo "$p"; return 0; }
    done
    if _is_source_checkout; then
        for p in "${REPO_ROOT}/target/release/meridian" "${REPO_ROOT}/target/debug/meridian"; do
            [[ -x "$p" ]] && { echo "$p"; return 0; }
        done
    fi
    return 1
}

# `meridian logs [target] [-n N] [-f]` decodes the local OTel telemetry spool
# (`~/.meridian/telemetry/{pending,sent}/*.otlp`) via the compiled binary's
# `logs` subcommand (`src/telemetry_spool/render.rs`) — the OTel spool is now
# the ONLY log/trace sink (see `observability.rs`'s module doc), so this is
# the one supported way to read logs locally without OpenObserve. `target`
# maps to `--service <name>`; omit it to see every service interleaved.
cmd_logs() {
    local target=""
    if [[ $# -gt 0 && "${1:-}" != -* ]]; then
        target="$1"; shift
    fi

    # `*-error` targets used to map to a distinct launchd StandardErrorPath
    # file capturing WARN+ output only. The OTel spool has no such separate
    # file, so these now pass `--min-severity WARN` to the decoder instead —
    # same filtering intent (errors/warnings only), applied at read time.
    local service_args=()
    case "$target" in
        ""|all)                     ;;
        daemon)                     service_args=(--service meridian-rust) ;;
        daemon-error)               service_args=(--service meridian-rust --min-severity WARN) ;;
        tray)                       service_args=(--service meridian-tray) ;;
        tray-error)                 service_args=(--service meridian-tray --min-severity WARN) ;;
        screenpipe|screenpipe-error|ui|ui-error)
            # Retired pre-in-process-capture / pre-Tauri-fold services — they
            # never participated in the OTel pipeline. Fall back to raw-tailing
            # whatever plain-text launchd file might still exist from before.
            local log_file="${LOG_DIR}/${target}.log"
            [[ -f "$log_file" ]] || { err "no log file at ${log_file}"; exit 1; }
            local lines=100 follow=()
            while [[ $# -gt 0 ]]; do
                case "$1" in
                    -f) follow=(-f); shift ;;
                    -n) lines="${2:?-n requires a value}"; shift 2 ;;
                    *) err "unknown option: $1"; exit 1 ;;
                esac
            done
            # macOS ships bash 3.2 as /bin/bash — under `set -u`, expanding an
            # empty array with "${arr[@]}" throws "unbound variable" (fixed
            # only in bash 4.4+). The `${arr[@]+"${arr[@]}"}` guard skips
            # expansion entirely when the array is empty/unset.
            exec tail -n "$lines" ${follow[@]+"${follow[@]}"} "$log_file"
            ;;
        *) err "unknown log target: ${target} (daemon|tray, or omit for all)"; exit 1 ;;
    esac

    local bin
    bin="$(_meridian_native_bin)" || { err "meridian binary not found — run ./install.sh"; exit 1; }
    # Same bash-3.2-unbound-variable guard as above — service_args is empty
    # for the ""/all case, which is the most common invocation.
    exec "$bin" logs ${service_args[@]+"${service_args[@]}"} "$@"
}

# --- doctor ---
# The daemon binary owns the comprehensive, colourised, by-daemon health table
# (system, meridian daemon, capture, jira, coding-agent CLIs, mcp). The wrapper
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
            return $rc
        fi
        warn "health engine timed out or is stale — rebuild: cargo build --release"
    fi
    _doctor_fallback
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
    _group "builds"
    _row "$([[ -f "${REPO_ROOT}/packages/meridian-mcp/dist/index.js" ]] && echo ok || echo fail)" "mcp built" ""
    _row "$([[ -d "${REPO_ROOT}/ui/.next" ]] && echo ok || echo fail)" "ui built" ""
    echo
    _row info "next step" "cargo build --release && meridian doctor"
    [[ $DOCTOR_FAILURES -eq 0 ]]
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
        err "meridian update is only for a source checkout."
        err "Packaged installs update themselves from within the Meridian app."
        exit 1
    fi
}

# --- uninstall ---
cmd_uninstall() {
    local ans
    read -r -p "This will stop the daemon, remove the app bundle and CLI symlinks (plus any leftover runtime/models from older versions). Your data (~/.meridian/*.db, logs) will NOT be deleted. Continue? [y/N] " ans
    if [[ "$ans" != "y" && "$ans" != "Y" ]]; then
        printf "(cancelled)\n"
        exit 0
    fi

    set +e

    # 1. Stop and remove all launchd agents (incl. legacy ones from old versions:
    #    the standalone UI server + MLX server no longer exist, but boot out any
    #    leftover agent from an install that predates their removal).
    bash "${REPO_ROOT}/scripts/uninstall-daemon.sh" 2>/dev/null
    bash "${REPO_ROOT}/scripts/uninstall-screenpipe-daemon.sh" 2>/dev/null
    bash "${REPO_ROOT}/scripts/uninstall-tray-daemon.sh" 2>/dev/null
    bash "${REPO_ROOT}/scripts/uninstall-openobserve-daemon.sh" 2>/dev/null
    for _legacy in com.meridiona.mlx-server com.meridiona.ui com.meridiona.a11y-helper; do
        launchctl bootout "${GUI_TARGET}/${_legacy}" 2>/dev/null || true
        rm -f "${HOME}/Library/LaunchAgents/${_legacy}.plist"
    done

    # 2. Kill any orphaned processes (legacy MLX server)
    pkill -f "mlx_lm.server" 2>/dev/null || true

    # 3. Remove CLI symlinks
    rm -f /usr/local/bin/meridian /usr/local/bin/meridian-daemon \
          "${HOME}/.local/bin/meridian" "${HOME}/.local/bin/meridian-daemon"

    # 4. Remove the app bundle (binaries, venv, scripts, UI build)
    rm -rf "${HOME}/.meridian/app"

    # 5. Remove staged binaries and the bin dir if empty afterwards
    rm -f "${HOME}/.meridian/bin/meridian" \
          "${HOME}/.meridian/bin/meridian-daemon" \
          "${HOME}/.meridian/bin/meridian-tray" \
          "${HOME}/.meridian/bin/screenpipe" \
          "${HOME}/.meridian/bin/meridian-a11y-helper"
    rmdir "${HOME}/.meridian/bin" 2>/dev/null || true

    # 5a. Remove leftover runtime assets from older versions (the Python venv,
    #     bundled Node runtime, model runtime) — no longer created, cleaned up here.
    rm -rf "${HOME}/.meridian/mlx-server-venv"
    rm -rf "${HOME}/.meridian/node-runtime"
    rm -rf "${HOME}/.meridian/runtime"
    rm -f  "${HOME}/.meridian/py-src.sha256" \
           "${HOME}/.meridian/doctor-bundle.txt" \
           "${HOME}/.meridian/.component-hashes"

    # 5b. Remove OAuth token store (user credentials — deleted on uninstall)
    rm -rf "${HOME}/.meridian/oauth"

    # 6. Uninstall the legacy npm package, if this machine has an old npm install.
    command -v npm >/dev/null 2>&1 && npm uninstall -g @meridiona/meridian 2>/dev/null || true

    # 7. Remove the Claude Code SessionEnd hook
    local settings="${HOME}/.claude/settings.json"
    if [[ -f "${settings}" ]] && command -v python3 >/dev/null 2>&1; then
        python3 - "${settings}" <<'PYEOF' 2>/dev/null || true
import json, sys
path = sys.argv[1]
with open(path) as f: d = json.load(f)
hooks = d.get("hooks", {})
se = hooks.get("SessionEnd", [])
hooks["SessionEnd"] = [e for e in se if "meridian" not in str(e)]
d["hooks"] = hooks
with open(path, "w") as f: json.dump(d, f, indent=2)
PYEOF
        info "Claude Code SessionEnd hook removed"
    fi

    # 8. Remove downloaded MLX model weights from the HuggingFace cache.
    # Only delete models in meridian's catalog — avoids touching models the user
    # downloaded separately for other tools (Ollama, LM Studio, etc.).
    local _hf_cache="${HOME}/.cache/huggingface/hub"
    local _meridian_models=(
        "models--mlx-community--Llama-3.3-70B-Instruct-4bit"
        "models--mlx-community--DeepSeek-R1-Distill-Llama-70B-4bit"
        "models--mlx-community--Qwen3.6-35B-A3B-4bit"
        "models--mlx-community--DeepSeek-R1-Distill-Qwen-32B-4bit"
        "models--mlx-community--phi-4-4bit"
        "models--mlx-community--DeepSeek-R1-Distill-Qwen-14B-4bit"
        "models--mlx-community--gemma-3-12b-it-qat-4bit"
        "models--mlx-community--Qwen3.5-2B-OptiQ-4bit"
        "models--mlx-community--Qwen3.5-4B-MLX-4bit"
        "models--mlx-community--Llama-3.2-3B-Instruct-4bit"
    )
    for _mdir in "${_meridian_models[@]}"; do
        local _mpath="${_hf_cache}/${_mdir}"
        if [[ -d "${_mpath}" ]]; then
            rm -rf "${_mpath}"
            info "removed model: ${_mdir}"
        fi
    done

    set -e

    ok "Meridian uninstalled"
    printf "  Kept: ~/.meridian/meridian.db, ~/.meridian/logs/ (your data)\n"
    printf "  Removed: app bundle, OAuth tokens, CLI symlinks, and any leftover runtime/models\n"
}

# --- permissions ---
cmd_permissions() {
    info "Meridian needs two macOS permissions to capture activity (granted to the"
    info "Meridian tray app, which captures in-process):"
    echo
    echo "    • Screen Recording   • Accessibility"
    echo
    echo "    In each pane: click '+' → ⌘⇧G → paste the Meridian.app path → Open → toggle ON."
    echo
    read -r -p "  Press Enter to open Screen Recording pane (1/2)… " _
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
    read -r -p "  Press Enter when Screen Recording is granted… " _
    ok "Screen Recording acknowledged"
    read -r -p "  Press Enter to open Accessibility pane (2/2)… " _
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    read -r -p "  Press Enter when Accessibility is granted… " _
    ok "Accessibility acknowledged"
    info "After granting permissions, restart the daemon:"
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
# `meridian dev` runs the full watch/hot-reload environment (dev-start.sh:
# cargo-watch daemon + `tauri dev` tray, debug builds, no launchd). `dev build`
# does a one-off release build to verify the shipped bundle. Gated on Cargo.toml:
# a prebuilt install has no source, so these are hidden/disabled there.
_is_source_checkout() { [[ -f "${REPO_ROOT}/Cargo.toml" ]]; }

_dev_build_daemon() { info "building daemon (cargo --release)…"; ( cd "${REPO_ROOT}" && cargo build --release ); }
_dev_build_ui()     { info "building UI (npm run build)…";       ( cd "${REPO_ROOT}/ui" && npm run build ); }

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
            # The full dev environment: stops any previous dev/installed daemon,
            # then opens 2 Terminal windows — the Rust daemon under cargo-watch
            # (rebuilds + restarts on save) and the Tauri tray via `npm run tauri
            # dev` (which starts the Next.js hot-reload server itself). This is
            # the single source of truth for a dev session; `bash dev-start.sh`
            # runs the exact same script.
            exec bash "${REPO_ROOT}/dev-start.sh"
            ;;
        build)      _dev_build_daemon; _dev_build_ui; ok "built production bundles (no run)" ;;
        *) err "unknown dev target: ${target}";
           echo "  targets: (none) | build"; exit 1 ;;
    esac
}

case "$CMD" in
    start)            cmd_start ;;
    stop)             cmd_stop ;;
    restart)          cmd_restart ;;
    status)           cmd_status ;;
    logs)             cmd_logs "$@" ;;
    doctor)           cmd_doctor "$@" ;;
    migrate-db)       cmd_migrate_db "$@" ;;
    config)           cmd_config "$@" ;;
    dev)              cmd_dev "$@" ;;
    update)           cmd_update ;;
    uninstall)        cmd_uninstall ;;
    permissions)      cmd_permissions ;;
    version|--version|-v) cat "${REPO_ROOT}/VERSION" 2>/dev/null || echo "unknown" ;;
    worklog-status|coding-agent-hook|coding-agent-summarise|coding-agent-install-skill|oauth-login|tasks-sync|ticket-update|ticket-parents|ticket-statuses|ticket-set-status|worklog-generate|worklog-generate-get|worklog-generate-approve|worklog-post-approved) cmd_daemon_passthrough "$CMD" "$@" ;;
    --help|-h|help|"") cmd_help ;;
    *) err "unknown command: ${CMD}"; echo; cmd_help; exit 1 ;;
esac
