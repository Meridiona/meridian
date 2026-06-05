#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Bootstrap installer. Fixes the npm global prefix when it is root-owned (the
# most common reason `npm install -g` fails with EACCES on a stock macOS Node
# install), installs @meridiona/meridian, then hands off to `meridian setup`.
#
# One-liner usage (download + inspect first if you prefer):
#   curl -fsSL https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.sh | bash
#
# Or run directly from a clone:
#   bash scripts/bootstrap.sh
#
# Re-running is safe — all steps are idempotent.

set -euo pipefail

info() { printf '→ %s\n'   "$*"; }
ok()   { printf '  ✓ %s\n' "$*"; }
warn() { printf '  ⚠ %s\n' "$*" >&2; }
err()  { printf '✗ %s\n'   "$*" >&2; exit 1; }

# ── 0. Platform + safety guards ──────────────────────────────────────────────
[[ "$(uname -s)" == "Darwin" ]] || err "Meridian requires macOS."
[[ "$(uname -m)" == "arm64"  ]] || err "Meridian requires Apple Silicon (arm64)."
[[ "$(id -u)" -ne 0          ]] || err "Do not run as root / with sudo. Re-run as your normal user."

# ── 1. Homebrew ───────────────────────────────────────────────────────────────
command -v brew >/dev/null 2>&1 \
    || err "Homebrew is required. Install it from https://brew.sh then re-run."
ok "Homebrew"

# ── 2. Node.js ────────────────────────────────────────────────────────────────
if ! command -v node >/dev/null 2>&1; then
    info "Installing Node.js via Homebrew…"
    brew install node
fi
ok "Node $(node --version)"

# ── 3. npm global prefix — ensure it is user-writable ─────────────────────
# A system Node install (/usr/local) has a root-owned prefix; npm install -g
# fails with EACCES. Fix: redirect the prefix to ~/.npm-global (user-owned)
# and add ~/.npm-global/bin to PATH in the user's shell profile. This is a
# permanent, one-time change — subsequent `npm install -g` commands (including
# `meridian update`) just work without sudo.
npm_global_writable() {
    local prefix; prefix="$(npm config get prefix 2>/dev/null || true)"
    [[ -n "$prefix" ]] || return 1
    # Use ownership of the prefix itself — more reliable than -w on the
    # node_modules dir. -w returns true even when ACLs or missing @scope
    # subdirs prevent mkdir, as seen on /usr/local with system Node.
    local owner; owner="$(stat -f '%Su' "${prefix}" 2>/dev/null || true)"
    [[ -n "$owner" && "$owner" == "$(id -un)" ]]
}

NPM_GLOBAL="${HOME}/.npm-global"

if npm_global_writable; then
    ok "npm prefix ($(npm config get prefix)) is user-writable — no fix needed"
else
    _old_prefix="$(npm config get prefix 2>/dev/null || true)"
    info "npm prefix (${_old_prefix}) is root-owned — redirecting to ${NPM_GLOBAL}…"
    mkdir -p "${NPM_GLOBAL}"
    npm config set prefix "${NPM_GLOBAL}"

    # Patch the user's shell profile so the fix survives new terminals.
    _profile=""
    case "${SHELL:-}" in
        */zsh)  _profile="${ZDOTDIR:-${HOME}}/.zshrc" ;;
        */bash) _profile="${HOME}/.bash_profile" ;;
    esac
    _export='export PATH="${HOME}/.npm-global/bin:${PATH}"'
    if [[ -n "${_profile}" ]] && ! grep -qF '.npm-global/bin' "${_profile}" 2>/dev/null; then
        {
            printf '\n# Added by meridian bootstrap — npm global prefix\n'
            printf '%s\n' "${_export}"
        } >> "${_profile}"
        ok "Added ~/.npm-global/bin to PATH in ${_profile}"
        warn "Open a new terminal (or run: source ${_profile}) after setup to pick up PATH."
    fi

    # Apply for this session so meridian is immediately on PATH after install.
    export PATH="${NPM_GLOBAL}/bin:${PATH}"
    ok "npm prefix → ${NPM_GLOBAL} (user-writable, no sudo needed)"
fi

# ── 4. Install @meridiona/meridian ───────────────────────────────────────────
info "Installing @meridiona/meridian@latest…"
info "  The npm package is small; the Python venv + Node runtime (~200 MB) download"
info "  once during 'meridian setup' below. Budget 2–4 min on a typical connection."
npm install -g @meridiona/meridian@latest
ok "meridian installed ($(meridian --version 2>/dev/null || echo 'version unknown'))"

# ── 5. Hand off to meridian setup ────────────────────────────────────────────
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  meridian is installed. Running setup now…"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# When run interactively (direct bash invocation), exec meridian setup so it
# owns the terminal for the permission walkthrough. When piped (curl | bash)
# there is no TTY for interactive prompts — skip permissions and print a
# reminder; the user runs `meridian setup` once they have a full terminal.
if [[ -t 0 && -t 1 ]]; then
    exec meridian setup
else
    meridian setup --skip-permissions
    echo ""
    echo "  Next step: open a new terminal and run:"
    echo ""
    echo "    meridian setup"
    echo ""
    echo "  This grants macOS Screen Recording + Accessibility to screenpipe"
    echo "  and collects your Jira / GitHub / Linear credentials."
fi
