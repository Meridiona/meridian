#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

set -euo pipefail
IFS=$'\n\t'

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

NO_UI=0
DRY_RUN=0
NO_DAEMON=0
SKIP_PERMISSIONS=0
SKIP_ENV=0
DEV_MODE=0  # --dev: debug binary + npm ci only; MLX server + Rust daemon run via launchd
USE_MLX=1   # MLX inference server is the only backend (powers classify + PM-worklog synth)
MLX_PORT=7823

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

info()  { echo "→ $*" >&2; }
ok()    { echo "  ✓ $*" >&2; }
warn()  { echo "  ⚠ $*" >&2; }
err()   { echo "✗ $*" >&2; exit 1; }

run() {
    if [[ "${DRY_RUN}" -eq 1 ]]; then
        local IFS=' '
        echo "[DRY-RUN] $*" >&2
    else
        "$@"
    fi
}

prompt_install() {
    local question="$1"
    if [[ "${DRY_RUN}" -eq 1 ]]; then
        echo "[DRY-RUN] Would ask: ${question} [Y/n] — assuming Y" >&2
        return 0
    fi
    read -r -p "  ${question} [Y/n] " ans
    [[ "${ans:-Y}" =~ ^[Yy] ]]
}

# ---------------------------------------------------------------------------
# Env-var collection helpers
# ---------------------------------------------------------------------------

# Read a value from an .env file. Returns empty string if missing or commented.
get_env_value() {
    local key="$1" file="$2"
    [[ -f "$file" ]] || return 0
    grep -E "^${key}=" "$file" 2>/dev/null | tail -1 | cut -d= -f2- || true
}

# Set KEY=VALUE in FILE. Replaces existing uncommented line, otherwise appends.
# Idempotent — safe to call multiple times.
set_env_value() {
    local key="$1" value="$2" file="$3"
    [[ -f "$file" ]] || touch "$file"
    if grep -qE "^${key}=" "$file" 2>/dev/null; then
        local tmp
        tmp="$(mktemp)"
        awk -v k="$key" -v v="$value" '
            BEGIN { FS=OFS="="; replaced=0 }
            $1==k && !replaced { print k"="v; replaced=1; next }
            { print }
        ' "$file" > "$tmp"
        mv "$tmp" "$file"
    else
        printf '%s=%s\n' "$key" "$value" >> "$file"
    fi
}

# Prompt for ONE variable. Skips silently if value already exists in any of the files.
# Writes to all files listed (space-separated absolute paths).
# Args: <var_name> <human description> <secret? 0|1> <file1> [file2...]
# Returns: 0 always (skipping is not an error).
prompt_env_var() {
    local key="$1" desc="$2" secret="$3"
    shift 3
    local files=("$@")
    # Check if already set in any file; if so, sync to any files that are missing it.
    local found_value=""
    local found_in=""
    for f in "${files[@]}"; do
        local existing
        existing="$(get_env_value "$key" "$f")"
        if [[ -n "$existing" ]]; then
            found_value="$existing"
            found_in="$f"
            break
        fi
    done
    if [[ -n "$found_value" ]]; then
        ok "${key} already set in $(basename "$(dirname "$found_in")")/$(basename "$found_in") — keeping"
        for f in "${files[@]}"; do
            if [[ -z "$(get_env_value "$key" "$f")" ]]; then
                set_env_value "$key" "$found_value" "$f"
                info "  → synced ${key} to $(basename "$(dirname "$f")")/$(basename "$f")"
            fi
        done
        return 0
    fi
    local value=""
    if [[ "${DRY_RUN:-0}" -eq 1 ]]; then
        info "[DRY-RUN] would prompt: $desc"
        return 0
    fi
    if [[ "$secret" == "1" ]]; then
        read -r -s -p "    ${desc}: " value
        echo
    else
        read -r -p "    ${desc}: " value
    fi
    if [[ -z "$value" ]]; then
        info "  (skipped ${key})"
        return 0
    fi
    for f in "${files[@]}"; do
        set_env_value "$key" "$value" "$f"
    done
    ok "${key} written"
}

# Prompt [y/N] for a category. Returns 0 (yes) or 1 (no/skip).
prompt_category() {
    local label="$1"
    if [[ "${DRY_RUN:-0}" -eq 1 ]]; then
        info "[DRY-RUN] would ask: Configure ${label}?"
        return 1
    fi
    local ans
    read -r -p "  Configure ${label}? [y/N] " ans
    [[ "$ans" =~ ^[Yy] ]]
}

# GitHub setup helpers — shared with scripts/install-from-bundle.sh.
source "${REPO_ROOT}/scripts/lib-github-setup.sh"
source "${REPO_ROOT}/scripts/lib-jira-setup.sh"
source "${REPO_ROOT}/scripts/lib-azure-setup.sh"

prompt_env_vars() {
    if [[ "${SKIP_ENV:-0}" == "1" ]]; then
        info "Skipping env-var prompts (--skip-env)"
        return 0
    fi
    info "Collecting credentials — press Enter on any prompt to skip"
    echo "    (you can re-run later: meridian config edit)"
    echo

    local root_env="${REPO_ROOT}/.env"

    # Ensure parent dir and file exist so env collection can write to it.
    mkdir -p "$(dirname "$root_env")"
    [[ -f "$root_env" ]] || touch "$root_env"

    info "→ LLM for task classification"
    echo "    Using the persistent MLX inference server (Apple Silicon). No LLM endpoint"
    echo "    needed — the daemon always calls the MLX classifier; MLX_SERVER_PORT is"
    echo "    written to <repo>/.env automatically."
    echo

    if prompt_category "Jira"; then
        # OAuth-first. The daemon binary isn't built yet at this point, so a
        # browser-OAuth choice is DEFERRED (MERIDIAN_JIRA_OAUTH_PENDING) and run
        # after the build, before the daemon starts. API token is the fallback.
        _connect_jira "$root_env" "${REPO_ROOT}/target/release/meridian"
    fi
    echo

    if prompt_category "GitHub"; then
        if ! _try_gh_token "$root_env"; then
            echo
            echo "    Alternatively, create a personal access token (classic) at:"
            echo "      https://github.com/settings/tokens/new"
            echo "    Required scopes: repo, read:org, read:project"
            echo "    (read:project lets meridian read your GitHub Projects; repo posts worklog comments)"
            echo
            prompt_env_var "GITHUB_TOKEN" "GitHub personal access token" 1 "$root_env"
        fi
        _pick_github_projects "$root_env"
    fi
    echo

    if prompt_category "Linear"; then
        prompt_env_var "LINEAR_API_KEY" "Linear API key" 1 "$root_env"
        prompt_env_var "LINEAR_TEAM_IDS" "Linear team IDs (optional, comma-sep)" 0 "$root_env"
    fi
    echo

    if prompt_category "Azure DevOps (VSTS)"; then
        setup_azure_devops "$root_env"
    fi
    echo

    if prompt_category "Observability (OpenObserve)"; then
        # Check if MERIDIAN_OO_AUTH is already set in the target file.
        local _oo_auth_existing=""
        _oo_auth_existing="$(get_env_value "MERIDIAN_OO_AUTH" "$root_env")"

        if [[ -n "$_oo_auth_existing" ]]; then
            ok "MERIDIAN_OO_AUTH already set in $(basename "$(dirname "$root_env")")/$(basename "$root_env") — keeping"
        elif [[ "${DRY_RUN:-0}" -eq 1 ]]; then
            info "[DRY-RUN] would prompt: OpenObserve admin email + password"
        else
            echo "    OpenObserve runs locally — you are creating its admin account now."
            echo "    Choose any email and password. They will be used the first time"
            echo "    OpenObserve starts (they are not validated against any external service)."
            echo
            local _oo_email="" _oo_pass=""
            read -r -p "    Admin email: " _oo_email
            read -r -s -p "    Admin password: " _oo_pass
            echo
            if [[ -n "$_oo_email" && -n "$_oo_pass" ]]; then
                local _oo_auth
                _oo_auth="$(printf '%s:%s' "$_oo_email" "$_oo_pass" | base64)"
                set_env_value "MERIDIAN_OO_AUTH" "$_oo_auth" "$root_env"
                ok "MERIDIAN_OO_AUTH written (base64-encoded email:password)"
            else
                info "  (skipped MERIDIAN_OO_AUTH — run 'meridian config edit' to add later)"
            fi
        fi

        prompt_env_var "MERIDIAN_OTLP_ENDPOINT" "OTLP HTTP traces endpoint (Rust side, e.g. http://localhost:5080/api/default/v1/traces)" 0 "$root_env"
        prompt_env_var "MERIDIAN_OTLP_TRACES_ENDPOINT" "OTLP HTTP traces endpoint (Python side; same URL as above is fine)" 0 "$root_env"
    fi

    ok "Credential collection complete"
}

prompt_permissions() {
    if [[ "${SKIP_PERMISSIONS:-0}" == "1" ]]; then
        info "Skipping permissions walkthrough (--skip-permissions)"
        return 0
    fi
    info "Meridian needs two macOS permissions to capture activity"
    echo
    echo "    Grant both to Meridian in System Settings → Privacy & Security:"
    echo "      • Screen Recording"
    echo "      • Accessibility"
    echo "    Click '+', press ⌘⇧G, paste the app path, Open, toggle ON."
    echo
    read -r -p "  Press Enter to open Screen Recording pane (1/2)… " _
    run open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
    read -r -p "  Press Enter when Screen Recording is granted… " _
    ok "Screen Recording acknowledged"
    read -r -p "  Press Enter to open Accessibility pane (2/2)… " _
    run open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    read -r -p "  Press Enter when Accessibility is granted… " _
    ok "Accessibility acknowledged"
}


# ---------------------------------------------------------------------------
# Arg parsing
# ---------------------------------------------------------------------------

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-ui)             NO_UI=1 ;;
        --dry-run)           DRY_RUN=1 ;;
        --no-daemon)         NO_DAEMON=1 ;;
        --skip-permissions)  SKIP_PERMISSIONS=1 ;;
        --skip-env)          SKIP_ENV=1 ;;
        --dev)               DEV_MODE=1 ;;
        --mlx)               : ;;   # accepted no-op (MLX is the only backend); kept for back-compat
        --mlx-port)          MLX_PORT="$2"; shift ;;
        --help|-h)
            cat >&2 <<'EOF'
Usage: bash install.sh [OPTIONS]

  --dev                Dev mode: debug Rust binary (faster builds), npm ci only for UI
                       (no next build). MLX server + Rust daemon run via launchd;
                       tray + UI run with hot-reload via dev-start.sh.
  --no-ui              Skip the Next.js build/install step entirely
  --dry-run            Print every action with [DRY-RUN] prefix; create/run nothing
  --no-daemon          Build everything but skip launchd registration
  --skip-permissions   Skip the macOS permissions walkthrough (Screen Recording, Accessibility, Microphone)
  --skip-env           Skip the interactive credentials collection step
  --mlx                Accepted no-op (the persistent MLX inference server is the only
                       backend; Apple Silicon only). Installs mlx-lm + outlines + fastapi
                       into .venv, registers the MLX server LaunchAgent. The MLX server
                       powers classification AND the PM-worklog synthesiser.
  --mlx-port N         MLX server port (default: 7823). Written to <repo>/.env.
  --help, -h           Print this usage and exit

After permissions, install.sh walks you through collecting credentials interactively
(API keys for Jira, GitHub, Linear, OpenRouter, and OpenObserve). Existing values
are never overwritten. Press Enter on any prompt to skip it. Use --skip-env to
bypass this step entirely (e.g. in CI or when credentials are already in place).

Capture runs in-process inside the Meridian tray — no separate screenpipe process needed.
EOF
            exit 0
            ;;
        *)
            err "Unknown option: $1"
            exit 1
            ;;
    esac
    shift
done

# ---------------------------------------------------------------------------
# Step 0: macOS gate
# ---------------------------------------------------------------------------

[[ "$(uname -s)" == "Darwin" ]] || { err "Meridian requires macOS."; exit 1; }

# ---------------------------------------------------------------------------
# Step 1: Prereq detection
# ---------------------------------------------------------------------------

info "Checking prerequisites..."

if ! command -v brew >/dev/null 2>&1; then
    warn "Homebrew not found."
    if prompt_install "Install Homebrew now?"; then
        run /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    else
        err "Homebrew is required. Install it from https://brew.sh and re-run."
        exit 1
    fi
fi
ok "Homebrew"

if ! command -v cargo >/dev/null 2>&1; then
    warn "Rust/cargo not found."
    if prompt_install "Install Rust via rustup now?"; then
        run curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
        # shellcheck source=/dev/null
        [[ "${DRY_RUN}" -eq 0 ]] && source "${HOME}/.cargo/env"
        # Verify cargo is actually on PATH — the source above only works when
        # ~/.cargo/env exists and this is an interactive-style shell invocation.
        if [[ "${DRY_RUN}" -eq 0 ]] && ! command -v cargo >/dev/null 2>&1; then
            err "cargo not found after sourcing ~/.cargo/env. Open a new terminal and re-run install.sh."
        fi
    else
        err "Rust is required. Install from https://rustup.rs and re-run."
        exit 1
    fi
fi
ok "cargo (Rust)"

_node_ok=0
if command -v node >/dev/null 2>&1; then
    _node_ver="$(node --version | sed 's/^v//')"
    _node_major="${_node_ver%%.*}"
    if [[ "${_node_major}" -ge 18 ]]; then
        _node_ok=1
    fi
fi
if [[ "${_node_ok}" -eq 0 ]]; then
    warn "Node.js 18+ not found."
    if prompt_install "Install Node.js via Homebrew now?"; then
        run brew install node
    else
        err "Node.js 18+ is required. Install it and re-run."
        exit 1
    fi
fi
ok "Node.js 18+"

_py_ok=0
if command -v python3.11 >/dev/null 2>&1; then
    _py_ok=1
elif command -v python3 >/dev/null 2>&1; then
    _py_ver="$(python3 --version 2>&1 | awk '{print $2}')"
    _py_minor="${_py_ver#*.}"
    _py_minor="${_py_minor%%.*}"
    _py_major="${_py_ver%%.*}"
    if [[ "${_py_major}" -ge 3 && "${_py_minor}" -ge 11 ]]; then
        _py_ok=1
    fi
fi
if [[ "${_py_ok}" -eq 0 ]]; then
    warn "Python 3.11+ not found."
    if prompt_install "Install Python 3.11 via Homebrew now?"; then
        run brew install python@3.11
        if [[ "${DRY_RUN}" -eq 0 ]] && ! command -v python3 >/dev/null 2>&1; then
            warn "python3 still not on PATH after install — you may need to add $(brew --prefix python@3.11)/bin to PATH"
        fi
    else
        err "Python 3.11+ is required. Install it and re-run."
        exit 1
    fi
fi
ok "Python 3.11+"


# OpenObserve — optional local backend for traces + logs. Not on Homebrew; we
# download the latest release from GitHub directly to ~/.openobserve/.
_oo_installed=0
command -v openobserve >/dev/null 2>&1 && _oo_installed=1
[[ -x "${HOME}/.openobserve/openobserve" ]] && _oo_installed=1

if [[ "$_oo_installed" -eq 0 ]]; then
    warn "OpenObserve not found (optional — local backend for traces + logs)."
    echo "    Skip if you don't need observability — Meridian works without it."
    if prompt_install "Download OpenObserve to ~/.openobserve/?"; then
        _oo_arch="$(uname -m)"
        case "$_oo_arch" in
            arm64)  _oo_arch="arm64" ;;
            x86_64) _oo_arch="amd64" ;;
            *) err "Unsupported arch: $_oo_arch — install manually from https://openobserve.ai"; exit 1 ;;
        esac

        # GitHub stopped attaching binary assets to OO releases, so the binaries
        # live on the official downloads host. Pin a current stable — trace
        # deep-linking (dashboard drilldown into a single trace's spans) needs a
        # modern build; the old v0.11 GitHub asset is long gone.
        # KEEP IN SYNC: the same version is pinned in
        # scripts/install-openobserve-daemon.sh — bump both together.
        _oo_ver="v0.90.3"
        _oo_url="https://downloads.openobserve.ai/releases/openobserve/${_oo_ver}/openobserve-${_oo_ver}-darwin-${_oo_arch}.tar.gz"
        if [[ "${DRY_RUN}" -ne 0 ]]; then
            _oo_ver="v0-dry-run"
            _oo_url="https://example.com/dry-run"
        fi

        if [[ -z "$_oo_url" ]]; then
            warn "Could not find a darwin-${_oo_arch} binary asset for OpenObserve on GitHub"
            warn "Install manually: https://openobserve.ai/docs/install/"
        else
            info "Fetching OpenObserve ${_oo_ver} (${_oo_arch})"
            run mkdir -p "${HOME}/.openobserve"
            if run curl -fsSL -o "${HOME}/.openobserve/openobserve.tar.gz" "$_oo_url" \
                && run tar -xzf "${HOME}/.openobserve/openobserve.tar.gz" -C "${HOME}/.openobserve"; then
                # The binary inside the tarball may be named differently; find and
                # normalise it to 'openobserve' so install-openobserve-daemon.sh
                # always finds it at the expected path.
                if [[ -f "${HOME}/.openobserve/openobserve" ]]; then
                    : # already the right name
                else
                    _oo_bin_found="$(find "${HOME}/.openobserve" -maxdepth 1 -type f -perm +0111 ! -name "*.tar.gz" | head -1 || true)"
                    if [[ -n "$_oo_bin_found" ]]; then
                        mv "$_oo_bin_found" "${HOME}/.openobserve/openobserve"
                    fi
                fi
                run chmod +x "${HOME}/.openobserve/openobserve"
                run rm -f "${HOME}/.openobserve/openobserve.tar.gz"
                _oo_installed=1
                if [[ "${DRY_RUN}" -eq 0 ]]; then
                    ok "OpenObserve ${_oo_ver} installed at ~/.openobserve/openobserve"
                fi
            else
                warn "Download failed from ${_oo_url}"
                warn "Install manually: https://openobserve.ai/docs/install/"
            fi
        fi
    fi
fi
[[ "$_oo_installed" -eq 1 ]] && ok "OpenObserve" || info "OpenObserve skipped (optional)"

# ---------------------------------------------------------------------------
# Permissions walkthrough (skipped in --dry-run, --skip-permissions, or --dev)
# In dev mode the tray's onboarding wizard handles permissions on first launch.
# ---------------------------------------------------------------------------

if [[ "${DRY_RUN}" -eq 1 ]]; then
    info "Skipping permissions walkthrough (--dry-run)"
elif [[ "${DEV_MODE}" -eq 1 ]]; then
    info "Skipping permissions walkthrough (--dev: tray onboarding handles this on first launch)"
else
    prompt_permissions
fi

# ---------------------------------------------------------------------------
# Credentials walkthrough (skipped in --dry-run or --skip-env)
# ---------------------------------------------------------------------------

prompt_env_vars

# ---------------------------------------------------------------------------
# Step 2: Config bootstrap
# ---------------------------------------------------------------------------

info "Bootstrapping config..."

run mkdir -p "${HOME}/.meridian/logs"

if [[ ! -f "${REPO_ROOT}/.env" ]]; then
    if [[ -f "${REPO_ROOT}/.env.example" ]]; then
        run cp "${REPO_ROOT}/.env.example" "${REPO_ROOT}/.env"
        warn "Created <repo>/.env from .env.example — edit it to add your credentials."
    else
        warn ".env.example not found; skipping <repo>/.env creation."
    fi
else
    ok "<repo>/.env already exists — not overwriting"
fi

# ---------------------------------------------------------------------------
# Step 3: Rust daemon build + symlink
# ---------------------------------------------------------------------------

if [[ "${DEV_MODE}" -eq 1 ]]; then
    info "Building Rust daemon (debug — dev mode)..."
    run cargo build --manifest-path "${REPO_ROOT}/Cargo.toml"
    ok "cargo build (debug)"
    MERIDIAN_BIN="${REPO_ROOT}/target/debug/meridian"
else
    info "Building Rust daemon..."
    run cargo build --release --manifest-path "${REPO_ROOT}/Cargo.toml"
    ok "cargo build --release"
    MERIDIAN_BIN="${REPO_ROOT}/target/release/meridian"
fi

# Fulfil a deferred Jira browser-OAuth login (chosen during the credential prompts,
# before the binary existed). Runs now that MERIDIAN_BIN is built and BEFORE the
# daemon starts, so the daemon reads the token store on its first launch — no
# extra command or restart. No-ops unless OAuth was chosen. (--dry-run skips: no
# binary to run.)
if [[ "${DRY_RUN:-0}" -eq 0 ]]; then
    _run_pending_jira_oauth "${MERIDIAN_BIN}" "${REPO_ROOT}/.env"
fi

if [[ -w "/usr/local/bin" ]]; then
    BIN_DIR="/usr/local/bin"
else
    BIN_DIR="${HOME}/.local/bin"
    run mkdir -p "${BIN_DIR}"
    case ":${PATH}:" in
        *":${BIN_DIR}:"*) ;;
        *)
            warn "${BIN_DIR} is not on \$PATH — add it to your shell rc file."
            ;;
    esac
fi

run ln -sfn "${MERIDIAN_BIN}" "${BIN_DIR}/meridian-daemon"
ok "meridian-daemon → ${BIN_DIR}/meridian-daemon"

# ---------------------------------------------------------------------------
# Step 4: MCP server build
# ---------------------------------------------------------------------------

info "Building MCP server..."
run bash -c "cd '${REPO_ROOT}/packages/meridian-mcp' && npm ci && npm run build"
ok "MCP server built"

# ---------------------------------------------------------------------------
# Step 5: UI build (skippable)
# ---------------------------------------------------------------------------

if [[ "${NO_UI}" -eq 0 ]]; then
    if [[ "${DEV_MODE}" -eq 1 ]]; then
        info "Installing UI dependencies (dev mode — skipping production build)..."
        run bash -c "cd '${REPO_ROOT}/ui' && npm ci"
        ok "UI dependencies installed (run manually: cd ui && npm run dev)"
    else
        info "Building Next.js UI..."
        run bash -c "cd '${REPO_ROOT}/ui' && npm ci && npm run build"
        ok "UI built"
    fi
else
    info "Skipping UI build (--no-ui)"
fi

# ---------------------------------------------------------------------------
# Step 5.5: Tray app build
# ---------------------------------------------------------------------------

if [[ "${NO_UI}" -eq 0 ]]; then
    if [[ "${DEV_MODE}" -eq 1 ]]; then
        info "Installing tray app dependencies (dev mode — skipping build)..."
        run bash -c "cd '${REPO_ROOT}/tray' && npm install"
        ok "Tray dependencies installed (run manually: cd tray && npm run tauri dev)"
    else
        info "Building tray app..."
        run bash -c "cd '${REPO_ROOT}/tray' && bash create-icons.sh && npm install && npm run build"
        ok "Tray app built"
    fi
else
    info "Skipping tray build (--no-ui)"
fi

# ---------------------------------------------------------------------------
# Step 6: Python services setup
# ---------------------------------------------------------------------------

info "Setting up Python services..."
run bash "${REPO_ROOT}/scripts/setup-services.sh" --mlx
ok "Python services ready"

# ---------------------------------------------------------------------------
# Step 7: Daemon install (skippable)
# ---------------------------------------------------------------------------

if [[ "${NO_DAEMON}" -eq 0 ]]; then
    if [[ "${_oo_installed}" -eq 1 ]]; then
        info "Installing OpenObserve launchd agent..."
        if ! run bash "${REPO_ROOT}/scripts/install-openobserve-daemon.sh"; then
            warn "OpenObserve daemon install skipped (set MERIDIAN_OO_AUTH in <repo>/.env to enable)"
        else
            ok "OpenObserve launchd agent installed"
        fi
    fi

    # MLX server must be running before the Rust daemon starts — the daemon
    # TCP-connects to it on startup and exits hard if the port is not reachable.
    if [[ "${USE_MLX}" -eq 1 ]]; then
        info "Writing MLX classification env vars to <repo>/.env..."
        set_env_value "MLX_SERVER_PORT"    "${MLX_PORT}"  "${REPO_ROOT}/.env"
        ok "MLX_SERVER_PORT=${MLX_PORT}"

        info "Installing MLX inference server launchd agent..."
        run bash "${REPO_ROOT}/services/scripts/install-mlx-server-daemon.sh" \
            --port "${MLX_PORT}"
        ok "MLX server launchd agent installed"

        if [[ "${DRY_RUN}" -eq 0 ]]; then
            _model_cache="${HOME}/.cache/huggingface/hub/models--mlx-community--Qwen3.5-2B-OptiQ-4bit/snapshots"
            if [[ -d "${_model_cache}" && -n "$(ls -A "${_model_cache}" 2>/dev/null)" ]]; then
                info "MLX server starting (model cached, loading into Metal)..."
            else
                echo
                info "First run: downloading MLX model (~6.6 GB). This takes a few minutes on a fast connection. Do not interrupt."
            fi
            echo "  ─────────────────────────────────────────────────────────────"
            mkdir -p "${HOME}/.meridian/logs"
            : >> "${HOME}/.meridian/logs/mlx-server.log"
            tail -n 0 -f "${HOME}/.meridian/logs/mlx-server.log" &
            _tail_pid=$!
            trap 'kill "${_tail_pid}" 2>/dev/null || true' EXIT
            _mlx_wait=0
            _mlx_timeout=300
            until curl -sf "http://127.0.0.1:${MLX_PORT}/health" >/dev/null 2>&1; do
                sleep 3
                _mlx_wait=$(( _mlx_wait + 3 ))
                if [[ "${_mlx_wait}" -ge "${_mlx_timeout}" ]]; then
                    kill "${_tail_pid}" 2>/dev/null || true
                    echo "  ─────────────────────────────────────────────────────────────"
                    warn "MLX server did not become ready within ${_mlx_timeout}s — check: tail -f ~/.meridian/logs/mlx-server.log"
                    break
                fi
            done
            if curl -sf "http://127.0.0.1:${MLX_PORT}/health" >/dev/null 2>&1; then
                kill "${_tail_pid}" 2>/dev/null || true
                echo "  ─────────────────────────────────────────────────────────────"
                ok "MLX server ready on port ${MLX_PORT} (${_mlx_wait}s)"
            fi
        fi
    fi

    info "Installing Rust daemon launchd agent..."
    run bash "${REPO_ROOT}/scripts/install-daemon.sh"
    ok "Rust daemon launchd agent installed"
    # Worklogs (Stage 4) and coding-agent ingest both run INSIDE the Rust
    # daemon — no separate launchd agents. Worklogs are only DRAFTED; they post
    # to your tracker (Jira/Linear/GitHub) only after you approve them in the
    # dashboard (Worklogs view).

    info "Installing Claude Code coding-agent SessionEnd hook..."
    if ! run bash "${REPO_ROOT}/services/scripts/install-claude-hook.sh"; then
        warn "coding-agent hook install skipped"
    else
        ok "Claude Code coding-agent SessionEnd hook installed"
    fi

    info "Installing session-summary Claude Code command..."
    _skill_src="${REPO_ROOT}/services/skills/coding-agent/session-summary/SKILL.md"
    _skill_dst="${HOME}/.claude/commands/session-summary.md"
    mkdir -p "${HOME}/.claude/commands"
    cp "${_skill_src}" "${_skill_dst}"
    ok "session-summary command → ~/.claude/commands/session-summary.md"

    # Coding-agent summariser engines (informational): each agent's sessions
    # are summarised by its OWN CLI when present; a missing CLI is never fatal
    # — those sessions fall back to the local MLX model. Surface what the
    # daemon will use so users know why a summary came from MLX.
    # `meridian doctor` re-checks all of this any time.
    info "Coding-agent summariser engines:"
    for _eng in claude codex copilot; do
        if command -v "${_eng}" >/dev/null 2>&1; then
            ok "${_eng} CLI found — those sessions summarise natively"
        else
            info "  ${_eng} CLI not found — those sessions will use the local model (MLX)"
        fi
    done
    if command -v cursor-agent >/dev/null 2>&1; then
        ok "cursor-agent CLI found — Cursor sessions summarise natively"
    elif command -v cursor >/dev/null 2>&1 || [[ -d "${HOME}/Library/Application Support/Cursor" ]]; then
        info "  cursor-agent CLI not found — Cursor sessions will use the local model (MLX)."
        info "  To summarise with Cursor's own CLI:  curl https://cursor.com/install -fsS | bash  then: cursor-agent login"
        info "  Or let the daemon install it on demand: add CURSOR_AGENT_AUTO_INSTALL=1 to your .env"
    fi

    if [[ "${DEV_MODE}" -eq 1 ]]; then
        info "Dev mode — skipping UI launchd agent (run: cd ui && npm run dev)"
    else
        info "Installing UI launchd agent..."
        run bash "${REPO_ROOT}/scripts/install-ui-daemon.sh"
        ok "UI launchd agent installed"
    fi

    if [[ "${NO_UI}" -eq 0 ]]; then
        if [[ "${DEV_MODE}" -eq 1 ]]; then
            info "Dev mode — skipping tray launchd agent (run: cd tray && npm run tauri dev)"
        else
            _tray_bin="${REPO_ROOT}/tray/src-tauri/target/release/meridian-tray"
            if [[ -x "${_tray_bin}" ]]; then
                info "Installing tray app launchd agent..."
                run bash "${REPO_ROOT}/scripts/install-tray-daemon.sh"
                ok "Tray app launchd agent installed"
            else
                warn "meridian-tray binary not found — tray app not installed"
            fi
        fi
    fi
else
    info "Skipping daemon install (--no-daemon)"
fi

# ---------------------------------------------------------------------------
# Step 8: CLI symlink
# ---------------------------------------------------------------------------

info "Installing meridian CLI..."
run ln -sfn "${REPO_ROOT}/scripts/meridian-cli.sh" "${BIN_DIR}/meridian"
ok "meridian CLI → ${BIN_DIR}/meridian"

# ---------------------------------------------------------------------------
# Step 9: Pipeline smoke test — verify both LLM stages work (no DB writes)
# ---------------------------------------------------------------------------

echo ""
info "Running pipeline smoke test (this exercises the model — may take ~30s)..."
if bash "${REPO_ROOT}/scripts/meridian-cli.sh" smoke; then
    ok "pipeline smoke passed — classification and worklog synthesis are working"
else
    warn "pipeline smoke found issues — run 'meridian doctor' for remedies"
fi

# Verify database schema has all migrations applied
if [[ "${DRY_RUN}" -eq 0 ]]; then
    _db="${HOME}/.meridian/meridian.db"
    _has_claude_uuid=$(sqlite3 "${_db}" ".schema app_sessions" 2>/dev/null | grep -c "claude_session_uuid" || echo "0")
    if [[ "${_has_claude_uuid}" -lt 1 ]]; then
        warn "Database schema incomplete — migrations may not have run yet"
        echo "  → The daemon is running migrations on startup. If this persists after 30s, run:"
        echo "    meridian migrate-db"
    else
        ok "database schema verified"
    fi
fi

# ---------------------------------------------------------------------------
# Step 10: Final instructions
# ---------------------------------------------------------------------------

echo ""
if [[ "${DEV_MODE}" -eq 1 ]]; then
    echo "✓ Meridian installed (dev mode)."
    echo ""
    echo "  Rust daemon + MLX server + tray run with hot-reload via dev-start.sh."
    echo ""
    echo "Start all services with hot-reload:"
    echo "  bash dev-start.sh"
    echo ""
    echo "  1. Rust daemon   — cargo watch, rebuilds on every .rs save"
    echo "  2. MLX server    — uvicorn --reload, reloads on .py changes"
    echo "  3. Next.js UI    — http://localhost:3939 (hot reload)"
    echo "  4. Tauri tray    — hot reload"
    echo ""
    echo "Useful commands:"
    echo "  meridian doctor         # diagnose"
    echo "  meridian config edit    # open <repo>/.env"
else
    echo "✓ Meridian installed."
    echo ""
    echo "  meridian start          # start all daemons (Rust daemon + MLX server)"
    echo "  meridian permissions    # re-run the permissions walkthrough"
    echo "  meridian status         # check running state"
    echo "  meridian logs           # tail Rust daemon log"
    echo "  meridian doctor         # diagnose"
    echo "  meridian config edit    # open <repo>/.env"
    echo ""
    echo "Required before Jira/GitHub/Linear sync:"
    echo "  <repo>/.env                  # one backend env for the Rust daemon AND Python services"
    echo "  ui/.env.local                # Next.js UI"
    echo ""
    echo "Worklogs (Jira/Linear/GitHub) are DRAFTED only — review, edit, and approve"
    echo "them in the dashboard (Worklogs view); the daemon posts approved worklogs"
    echo "within ~60s of approval."
fi

case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *)
        echo ""
        echo "  Add this to your shell rc file:"
        echo "    export PATH=\"${BIN_DIR}:\$PATH\""
        ;;
esac
