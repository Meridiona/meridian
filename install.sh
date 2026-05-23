#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions

set -euo pipefail
IFS=$'\n\t'

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

NO_UI=0
DRY_RUN=0
NO_DAEMON=0
SKIP_PERMISSIONS=0

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
    echo "    In each pane that opens: click the '+' button, navigate to the binary"
    echo "    path above, add it to the list, and toggle it ON."
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
    read -r -p "  Press Enter when Microphone is granted (or skip)… " _
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
        --help|-h)
            cat >&2 <<'EOF'
Usage: bash install.sh [OPTIONS]

  --no-ui              Skip the Next.js build step
  --dry-run            Print every action with [DRY-RUN] prefix; create/run nothing
  --no-daemon          Build everything but skip launchd registration
  --skip-permissions   Skip the macOS permissions walkthrough (Screen Recording, Accessibility, Microphone)
  --help, -h           Print this usage and exit

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
    if prompt_install "Install screenpipe via Homebrew?"; then
        run brew install screenpipe
    else
        err "screenpipe required — install via https://docs.screenpi.pe"
        exit 1
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

# ---------------------------------------------------------------------------
# Permissions walkthrough (skipped in --dry-run or --skip-permissions)
# ---------------------------------------------------------------------------

if [[ "${DRY_RUN}" -eq 0 ]]; then
    prompt_permissions
else
    info "Skipping permissions walkthrough (--dry-run)"
fi

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
