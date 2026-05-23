#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions

set -euo pipefail
IFS=$'\n\t'

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

NO_UI=0
DRY_RUN=0
NO_DAEMON=0
SKIP_PERMISSIONS=0
SKIP_ENV=0

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

info()  { echo "→ $*" >&2; }
ok()    { echo "  ✓ $*" >&2; }
warn()  { echo "  ⚠ $*" >&2; }
err()   { echo "✗ $*" >&2; }

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
    # Check if already set in any file
    for f in "${files[@]}"; do
        local existing
        existing="$(get_env_value "$key" "$f")"
        if [[ -n "$existing" ]]; then
            ok "${key} already set in $(basename "$(dirname "$f")")/$(basename "$f") — keeping"
            return 0
        fi
    done
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

prompt_env_vars() {
    if [[ "${SKIP_ENV:-0}" == "1" ]]; then
        info "Skipping env-var prompts (--skip-env)"
        return 0
    fi
    info "Collecting credentials — press Enter on any prompt to skip"
    echo "    (you can re-run later: meridian config edit)"
    echo

    local root_env="${HOME}/.meridian/.env"
    local svcs_env="${REPO_ROOT}/services/.env"
    local hermes_env="${REPO_ROOT}/services/.hermes/.env"

    # Ensure parent dirs and files exist. services/.hermes/ is created later by
    # setup-services.sh, but we need it now so env collection can write to it.
    for f in "$root_env" "$svcs_env" "$hermes_env"; do
        mkdir -p "$(dirname "$f")"
        [[ -f "$f" ]] || touch "$f"
    done

    info "→ Cloud LLM (for task classification)"
    echo "    Skip if you're running a local LLM (LM Studio, Ollama, mlx)."
    prompt_env_var "OPENROUTER_API_KEY" "OpenRouter API key (or any cloud LLM key)" 1 \
        "$hermes_env" "$svcs_env"
    echo

    if prompt_category "Jira"; then
        prompt_env_var "JIRA_BASE_URL" "Jira URL (e.g. https://your-org.atlassian.net)" 0 "$root_env"
        # The python-side variable name is JIRA_URL, not JIRA_BASE_URL — write both.
        local jira_url
        jira_url="$(get_env_value JIRA_BASE_URL "$root_env")"
        [[ -n "$jira_url" ]] && set_env_value JIRA_URL "$jira_url" "$svcs_env"
        prompt_env_var "JIRA_EMAIL" "Jira email" 0 "$root_env" "$svcs_env"
        prompt_env_var "JIRA_API_TOKEN" "Jira API token" 1 "$root_env" "$svcs_env"
        prompt_env_var "JIRA_PROJECT_KEYS" "Jira project keys (optional, comma-sep, e.g. KAN,ENG)" 0 "$root_env"
    fi
    echo

    if prompt_category "GitHub"; then
        prompt_env_var "GITHUB_TOKEN" "GitHub personal access token" 1 "$root_env"
        prompt_env_var "GITHUB_ORG" "GitHub organization" 0 "$root_env"
        prompt_env_var "GITHUB_REPOS" "GitHub repos (optional, comma-sep, e.g. org/repo1,org/repo2)" 0 "$root_env"
    fi
    echo

    if prompt_category "Linear"; then
        prompt_env_var "LINEAR_API_KEY" "Linear API key" 1 "$root_env"
        prompt_env_var "LINEAR_TEAM_IDS" "Linear team IDs (optional, comma-sep)" 0 "$root_env"
    fi
    echo

    if prompt_category "Observability (OpenObserve)"; then
        prompt_env_var "MERIDIAN_OO_AUTH" "base64(user:password) for OpenObserve" 1 "$root_env" "$svcs_env"
        prompt_env_var "MERIDIAN_OTLP_ENDPOINT" "OTLP HTTP traces endpoint (Rust side, e.g. http://localhost:5080/api/default/v1/traces)" 0 "$root_env"
        prompt_env_var "MERIDIAN_OTLP_TRACES_ENDPOINT" "OTLP HTTP traces endpoint (Python side; same URL as above is fine)" 0 "$svcs_env"
    fi

    ok "Credential collection complete"
}

prompt_permissions() {
    if [[ "${SKIP_PERMISSIONS:-0}" == "1" ]]; then
        info "Skipping permissions walkthrough (--skip-permissions)"
        return 0
    fi
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
    run open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
    read -r -p "  Press Enter when Screen Recording is granted… " _
    ok "Screen Recording acknowledged"
    read -r -p "  Press Enter to open Accessibility pane (2/3)… " _
    run open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    read -r -p "  Press Enter when Accessibility is granted… " _
    ok "Accessibility acknowledged"
    read -r -p "  Press Enter to open Microphone pane (3/3, optional)… " _
    run open "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
    read -r -p "  Press Enter when Microphone is granted (or skip if screenpipe isn't listed yet)… " _
    ok "Microphone acknowledged"
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
        --help|-h)
            cat >&2 <<'EOF'
Usage: bash install.sh [OPTIONS]

  --no-ui              Skip the Next.js build step
  --dry-run            Print every action with [DRY-RUN] prefix; create/run nothing
  --no-daemon          Build everything but skip launchd registration
  --skip-permissions   Skip the macOS permissions walkthrough (Screen Recording, Accessibility, Microphone)
  --skip-env           Skip the interactive credentials collection step
  --help, -h           Print this usage and exit

After permissions, install.sh walks you through collecting credentials interactively
(API keys for Jira, GitHub, Linear, OpenRouter, and OpenObserve). Existing values
are never overwritten. Press Enter on any prompt to skip it. Use --skip-env to
bypass this step entirely (e.g. in CI or when credentials are already in place).

screenpipe is installed automatically via Homebrew if not already present.
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

if ! command -v screenpipe >/dev/null 2>&1; then
    warn "screenpipe not found."
    echo "    Note: Homebrew's screenpipe formula is deprecated (0.2.x). We install the"
    echo "    current 0.3.x via npm, which is what the launchd plist expects."
    if prompt_install "Install screenpipe via npm (npm install -g screenpipe)?"; then
        run npm install -g screenpipe
    else
        err "screenpipe required — install via https://docs.screenpi.pe"
        exit 1
    fi
else
    _sp_ver="$(screenpipe --version 2>/dev/null | awk '{print $2}' || true)"
    _sp_major_minor="${_sp_ver%.*}"
    if [[ -n "${_sp_ver}" && "${_sp_major_minor}" < "0.3" ]]; then
        warn "screenpipe ${_sp_ver} is from the deprecated Homebrew formula."
        echo "    The launchd plist expects 0.3+ (uses 'screenpipe record')."
        if prompt_install "Upgrade screenpipe via npm (npm install -g screenpipe)?"; then
            run npm install -g screenpipe
        fi
    fi
fi
ok "screenpipe"

if ! command -v ffmpeg >/dev/null 2>&1 || [[ ! -x /opt/homebrew/bin/ffmpeg && ! -x /usr/local/bin/ffmpeg ]]; then
    warn "ffmpeg not found on the launchd PATH (screenpipe can't auto-install it from a daemon context)."
    if prompt_install "Install ffmpeg via Homebrew?"; then
        run brew install ffmpeg
    else
        err "ffmpeg required by screenpipe — install via 'brew install ffmpeg' and re-run."
        exit 1
    fi
fi
ok "ffmpeg"

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

        _oo_ver=""
        if [[ "${DRY_RUN}" -eq 0 ]]; then
            _oo_ver="$(curl -fsSL https://api.github.com/repos/openobserve/openobserve/releases/latest 2>/dev/null \
                | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d'"' -f4 || true)"
        else
            _oo_ver="v0-dry-run"
        fi

        if [[ -z "$_oo_ver" ]]; then
            warn "Could not resolve latest OpenObserve version from GitHub API"
            warn "Install manually: https://openobserve.ai/docs/install/"
        else
            _oo_url="https://github.com/openobserve/openobserve/releases/download/${_oo_ver}/openobserve-${_oo_ver}-darwin-${_oo_arch}.tar.gz"
            info "Fetching OpenObserve ${_oo_ver} (${_oo_arch})"
            run mkdir -p "${HOME}/.openobserve"
            if run curl -fsSL -o "${HOME}/.openobserve/openobserve.tar.gz" "$_oo_url" \
                && run tar -xzf "${HOME}/.openobserve/openobserve.tar.gz" -C "${HOME}/.openobserve"; then
                run chmod +x "${HOME}/.openobserve/openobserve"
                run rm -f "${HOME}/.openobserve/openobserve.tar.gz"
                _oo_installed=1
                if [[ "${DRY_RUN}" -eq 0 ]]; then
                    ok "OpenObserve ${_oo_ver} installed at ~/.openobserve/openobserve"
                    info "Start manually:  ~/.openobserve/openobserve"
                    echo "    Then visit http://localhost:5080 to create your account."
                    echo "    Use those credentials as MERIDIAN_OO_AUTH = base64(email:password)."
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
# Permissions walkthrough (skipped in --dry-run or --skip-permissions)
# ---------------------------------------------------------------------------

if [[ "${DRY_RUN}" -eq 0 ]]; then
    prompt_permissions
else
    info "Skipping permissions walkthrough (--dry-run)"
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

if [[ ! -f "${HOME}/.meridian/.env" ]]; then
    if [[ -f "${REPO_ROOT}/.env.example" ]]; then
        run cp "${REPO_ROOT}/.env.example" "${HOME}/.meridian/.env"
        warn "Created ~/.meridian/.env from .env.example — edit it to add your credentials."
    else
        warn ".env.example not found; skipping ~/.meridian/.env creation."
    fi
else
    ok "~/.meridian/.env already exists — not overwriting"
fi

if [[ ! -f "${REPO_ROOT}/services/.env" ]]; then
    if [[ -f "${REPO_ROOT}/services/.env.example" ]]; then
        run cp "${REPO_ROOT}/services/.env.example" "${REPO_ROOT}/services/.env"
        warn "Created services/.env from .env.example — edit it to add your credentials."
    else
        warn "services/.env.example not found; skipping services/.env creation."
    fi
else
    ok "services/.env already exists — not overwriting"
fi

# ---------------------------------------------------------------------------
# Step 3: Rust daemon build + symlink
# ---------------------------------------------------------------------------

info "Building Rust daemon..."
run cargo build --release --manifest-path "${REPO_ROOT}/Cargo.toml"
ok "cargo build --release"

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

run ln -sfn "${REPO_ROOT}/target/release/meridian" "${BIN_DIR}/meridian-daemon"
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
    info "Building Next.js UI..."
    run bash -c "cd '${REPO_ROOT}/ui' && npm ci && npm run build"
    ok "UI built"
else
    info "Skipping UI build (--no-ui)"
fi

# ---------------------------------------------------------------------------
# Step 6: Python services setup
# ---------------------------------------------------------------------------

info "Setting up Python services..."
run bash "${REPO_ROOT}/scripts/setup-services.sh"
ok "Python services ready"

# ---------------------------------------------------------------------------
# Step 7: Daemon install (skippable)
# ---------------------------------------------------------------------------

if [[ "${NO_DAEMON}" -eq 0 ]]; then
    info "Installing screenpipe launchd agent..."
    run bash "${REPO_ROOT}/scripts/install-screenpipe-daemon.sh"
    ok "screenpipe launchd agent installed"

    info "Installing Rust daemon launchd agent..."
    run bash "${REPO_ROOT}/scripts/install-daemon.sh"
    ok "Rust daemon launchd agent installed"

    info "Installing jira-updater launchd agent..."
    if ! run bash "${REPO_ROOT}/services/scripts/install-jira-updater-daemon.sh"; then
        warn "jira-updater install skipped (set MERIDIAN_OO_AUTH in services/.env to enable)"
    fi

    info "Installing UI launchd agent..."
    run bash "${REPO_ROOT}/scripts/install-ui-daemon.sh"
    ok "UI launchd agent installed"
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
# Step 9: Final instructions
# ---------------------------------------------------------------------------

echo ""
echo "✓ Meridian installed."
echo ""
echo "  meridian start          # start all three daemons (screenpipe + daemon + jira-updater)"
echo "  meridian permissions    # re-run the permissions walkthrough"
echo "  meridian status         # check running state"
echo "  meridian logs           # tail Rust daemon log"
echo "  meridian doctor         # diagnose"
echo "  meridian config edit    # open ~/.meridian/.env"
echo ""
echo "Required before Jira/GitHub/Linear sync:"
echo "  ~/.meridian/.env             # screenpipe + Rust daemon"
echo "  services/.env                # Python agents (LLM + Jira)"
echo "  services/.hermes/.env        # OLLAMA_API_KEY for hermes"

case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *)
        echo ""
        echo "  Add this to your shell rc file:"
        echo "    export PATH=\"${BIN_DIR}:\$PATH\""
        ;;
esac
