#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Install a PREBUILT release bundle (no cargo/npm build). Run from inside an
# unpacked bundle at ~/.meridian/app — bootstrap.sh downloads + unpacks the
# release tarball and execs this. Installs prerequisites, the Python venv + MLX
# deps, and registers the four launchd daemons pointing at this bundle.
#
#   bash ~/.meridian/app/scripts/install-from-bundle.sh [--skip-permissions]
set -euo pipefail

APP_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCREENPIPE_VERSION="0.3.350"
MLX_PORT="${MLX_PORT:-7823}"
UI_PORT="${MERIDIAN_UI_PORT:-3939}"   # dashboard port (override via MERIDIAN_UI_PORT)
SKIP_PERMISSIONS=0
[[ "${1:-}" == "--skip-permissions" ]] && SKIP_PERMISSIONS=1

info() { echo "→ $*" >&2; }
ok()   { echo "  ✓ $*" >&2; }
warn() { echo "  ⚠ $*" >&2; }
err()  { echo "✗ $*" >&2; }

# True when `npm i -g` can write the global prefix without root (Homebrew/user
# prefix). On a root-owned prefix (e.g. /usr/local) the one npm step is elevated
# on its own, rather than running this whole script as root.
npm_global_writable() {
    local prefix; prefix="$(npm config get prefix 2>/dev/null)"
    [[ -n "$prefix" && -w "${prefix}/lib/node_modules" ]]
}

# ── .env credential collection (mirrors install.sh's interactive walkthrough) ──
# Read an uncommented KEY=value from an .env file; empty if missing or commented.
get_env_value() {
    local key="$1" file="$2"
    [[ -f "$file" ]] || return 0
    grep -E "^${key}=" "$file" 2>/dev/null | tail -1 | cut -d= -f2- || true
}
# Set KEY=value in FILE (replace existing uncommented line, else append). Idempotent.
set_env_value() {
    local key="$1" value="$2" file="$3"
    [[ -f "$file" ]] || touch "$file"
    if grep -qE "^${key}=" "$file" 2>/dev/null; then
        local tmp; tmp="$(mktemp)"
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
# Prompt for ONE var; skip silently if already set so re-runs/updates never re-ask.
# Args: <key> <description> <secret 0|1> <env_file>
prompt_env_var() {
    local key="$1" desc="$2" secret="$3" file="$4" value=""
    if [[ -n "$(get_env_value "$key" "$file")" ]]; then ok "${key} already set — keeping"; return 0; fi
    if [[ "$secret" == "1" ]]; then read -r -s -p "    ${desc}: " value; echo >&2
    else read -r -p "    ${desc}: " value; fi
    [[ -z "$value" ]] && { info "  (skipped ${key})"; return 0; }
    set_env_value "$key" "$value" "$file"; ok "${key} written"
}
# Ask whether to configure a tracker. Returns 0 (yes) / 1 (no/skip).
prompt_category() {
    local label="$1" ans
    read -r -p "  Configure ${label}? [y/N] " ans
    [[ "$ans" =~ ^[Yy] ]]
}
# Interactive tracker-credential walkthrough → writes to the bundle .env.
collect_credentials() {
    local env_file="$1"
    info "Collecting tracker credentials — press Enter to skip any prompt"
    echo "    (edit later anytime: meridian config edit)" >&2
    echo >&2
    if prompt_category "Jira"; then
        prompt_env_var "JIRA_BASE_URL" "Jira URL (e.g. https://your-org.atlassian.net)" 0 "$env_file"
        # The Python side reads JIRA_URL, the Rust side JIRA_BASE_URL — keep both in sync.
        local jira_url; jira_url="$(get_env_value JIRA_BASE_URL "$env_file")"
        [[ -n "$jira_url" ]] && set_env_value JIRA_URL "$jira_url" "$env_file"
        prompt_env_var "JIRA_EMAIL" "Jira email" 0 "$env_file"
        prompt_env_var "JIRA_API_TOKEN" "Jira API token" 1 "$env_file"
        prompt_env_var "JIRA_PROJECT_KEYS" "Jira project keys (optional, comma-sep, e.g. KAN,ENG)" 0 "$env_file"
    fi
    echo >&2
    if prompt_category "GitHub"; then
        prompt_env_var "GITHUB_TOKEN" "GitHub personal access token" 1 "$env_file"
        prompt_env_var "GITHUB_ORG"   "GitHub organization (or your username)" 0 "$env_file"
        prompt_env_var "GITHUB_REPOS" "GitHub repos (optional, comma-sep owner/repo)" 0 "$env_file"
    fi
    echo >&2
    if prompt_category "Linear"; then
        prompt_env_var "LINEAR_API_KEY"  "Linear API key" 1 "$env_file"
        prompt_env_var "LINEAR_TEAM_IDS" "Linear team IDs (optional, comma-sep)" 0 "$env_file"
    fi
    ok "Credential collection complete"
}

GUI_TARGET="gui/$(id -u)"
LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"

# Register the dashboard as a launchd agent that runs the prebuilt Next.js
# standalone server (`node ui/server.js`) — no `npm start`, no node_modules
# install. Mirrors the EIO-safe bootout/bootstrap pattern of the other agents.
install_ui_standalone() {
    local label="com.meridiona.ui"
    local plist="${LAUNCH_AGENTS}/${label}.plist"
    local node_bin; node_bin="$(command -v node)"
    mkdir -p "${HOME}/.meridian/logs" "${LAUNCH_AGENTS}"
    cat > "${plist}" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>${label}</string>
  <key>ProgramArguments</key>
  <array><string>/bin/sh</string><string>-c</string>
    <string>exec '${node_bin}' '${APP_ROOT}/ui/server.js'</string></array>
  <key>WorkingDirectory</key><string>${APP_ROOT}/ui</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PORT</key><string>${UI_PORT}</string>
    <key>HOSTNAME</key><string>127.0.0.1</string>
    <key>MERIDIAN_DB</key><string>${HOME}/.meridian/meridian.db</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${HOME}/.meridian/logs/ui.log</string>
  <key>StandardErrorPath</key><string>${HOME}/.meridian/logs/ui-error.log</string>
  <key>ProcessType</key><string>Background</string>
</dict></plist>
PLIST
    plutil -lint "${plist}" >/dev/null 2>&1 || { warn "ui plist failed lint"; return 1; }
    launchctl bootout "${GUI_TARGET}/${label}" 2>/dev/null || true
    local w=0
    while launchctl print "${GUI_TARGET}/${label}" >/dev/null 2>&1; do
        sleep 1; w=$((w+1)); [[ $w -ge 15 ]] && break
    done
    launchctl enable "${GUI_TARGET}/${label}" 2>/dev/null || true
    launchctl bootstrap "${GUI_TARGET}" "${plist}"
    launchctl kickstart -k "${GUI_TARGET}/${label}" 2>/dev/null || true
}

# Enable accessibility mode in VS Code / Cursor / Antigravity so screenpipe
# captures their a11y tree instead of falling back to OCR. Chromium/Electron
# editors only expose their AX tree when editor.accessibilitySupport = "on"
# (the force-renderer-accessibility argv switch is Linux-only on macOS).
# Idempotent + JSONC-safe (preserves existing keys/comments). Without this, npm
# users' editors silently OCR-only until they discover the setting by hand.
configure_editor_accessibility() {
    local support_root="${HOME}/Library/Application Support"
    local editors=("Code" "Cursor" "Antigravity IDE")
    local any=0 ed dir settings
    for ed in "${editors[@]}"; do
        dir="${support_root}/${ed}"
        [[ -d "$dir" ]] || continue           # editor not installed → skip
        any=1
        settings="${dir}/User/settings.json"
        if [[ -f "$settings" ]] && grep -q '"editor.accessibilitySupport"' "$settings"; then
            ok "${ed}: editor.accessibilitySupport already set — keeping"
            continue
        fi
        mkdir -p "${dir}/User"
        if [[ ! -s "$settings" ]]; then
            printf '{\n\t"editor.accessibilitySupport": "on"\n}\n' > "$settings"
        else
            cp "$settings" "${settings}.meridian-bak"
            # Insert the key after the first `{`. VS Code-family parsers are
            # JSONC-tolerant, so this preserves existing keys/comments/formatting.
            perl -0777 -i -pe 's/\{/\{\n\t"editor.accessibilitySupport": "on",/ unless $done++' "$settings"
        fi
        ok "${ed}: enabled editor.accessibilitySupport = on (restart the editor)"
    done
    [[ "$any" -eq 0 ]] && info "No VS Code / Cursor / Antigravity install found — skipping editor a11y setup"
    return 0
}

# ── 0. Platform gate ────────────────────────────────────────────────────────
[[ "$(uname -s)" == "Darwin" ]]  || { err "Meridian requires macOS."; exit 1; }
[[ "$(uname -m)" == "arm64" ]]   || { err "Meridian requires Apple Silicon (arm64). This bundle is macOS-arm64 only."; exit 1; }

# Refuse root: this installs per-user launchd agents (gui/$(id -u)) and writes
# ~/.meridian. As root, launchd bootstrap fails and files end up root-owned. The
# npm launcher elevates only the steps that need root; the rest stays per-user.
if [[ "$(id -u)" -eq 0 ]]; then
    err "Do not run as root / with sudo — run 'meridian setup' as your normal user."
    exit 1
fi

echo "→ Installing Meridian $(cat "${APP_ROOT}/VERSION" 2>/dev/null || echo '?') from ${APP_ROOT}"

# ── 1. Prerequisites (no Rust/Node-build toolchain — artifacts are prebuilt) ──
if ! command -v brew >/dev/null 2>&1; then
    err "Homebrew required — install from https://brew.sh and re-run."; exit 1
fi
command -v node >/dev/null 2>&1 || { info "Installing Node.js…"; brew install node; }
PYTHON_BIN=""
for p in python3.11 python3; do command -v "$p" >/dev/null 2>&1 && { PYTHON_BIN="$(command -v "$p")"; break; }; done
[[ -n "${PYTHON_BIN}" ]] || { info "Installing Python 3.11…"; brew install python@3.11; PYTHON_BIN="$(command -v python3.11)"; }
# uv is the package/venv manager for Python services. Install via Homebrew (already
# required by this installer) rather than the astral curl|sh installer.
UV_BIN=""
if command -v uv >/dev/null 2>&1; then
    UV_BIN="$(command -v uv)"
else
    info "Installing uv (Python package manager)…"
    brew install uv
    UV_BIN="$(command -v uv)"
fi
ok "node + python ($(${PYTHON_BIN} --version 2>&1)) + uv ($(${UV_BIN} --version 2>&1))"

if ! command -v screenpipe >/dev/null 2>&1; then
    info "Installing screenpipe ${SCREENPIPE_VERSION} via npm…"
    if npm_global_writable; then
        npm install -g "screenpipe@${SCREENPIPE_VERSION}"
    else
        warn "global npm prefix needs root — elevating just this install (you may be prompted)…"
        sudo npm install -g "screenpipe@${SCREENPIPE_VERSION}"
    fi
fi
ok "screenpipe"
if ! command -v ffmpeg >/dev/null 2>&1; then info "Installing ffmpeg…"; brew install ffmpeg; fi
ok "ffmpeg"

# ── 2. Config: single repo-local .env ────────────────────────────────────────
ENV_FILE="${APP_ROOT}/.env"
if [[ ! -f "${ENV_FILE}" ]]; then
    cp "${APP_ROOT}/.env.example" "${ENV_FILE}"
    info "created ${ENV_FILE} from template — add your Jira creds later: meridian config edit"
fi
# MLX is the default backend.
grep -q '^CLASSIFIER_BACKEND=' "${ENV_FILE}" || echo "CLASSIFIER_BACKEND=mlx" >> "${ENV_FILE}"
grep -q '^MLX_SERVER_PORT='    "${ENV_FILE}" || echo "MLX_SERVER_PORT=${MLX_PORT}" >> "${ENV_FILE}"
# Dashboard port — honour an existing .env value, otherwise record the default.
if grep -q '^MERIDIAN_UI_PORT=' "${ENV_FILE}"; then
    UI_PORT="$(grep '^MERIDIAN_UI_PORT=' "${ENV_FILE}" | tail -n1 | cut -d= -f2 | tr -d '[:space:]')"
else
    echo "MERIDIAN_UI_PORT=${UI_PORT}" >> "${ENV_FILE}"
fi
ok "config at ${ENV_FILE} (dashboard port ${UI_PORT})"

# Interactive tracker-credential walkthrough — parity with install.sh. A fresh
# `meridian setup` collects Jira/GitHub/Linear keys here; `meridian update`
# passes --skip-permissions and keeps the preserved .env, so it never re-prompts.
# Each prompt also self-skips when its value is already set. Guarded on a TTY so
# non-interactive runs (CI, piped) don't block.
if [[ "${SKIP_PERMISSIONS}" -eq 0 && -t 0 ]]; then
    collect_credentials "${ENV_FILE}"
fi

# ── 3. Binary + CLI symlinks ─────────────────────────────────────────────────
mkdir -p "${HOME}/.local/bin"
ln -sfn "${APP_ROOT}/bin/meridian"        "${HOME}/.local/bin/meridian-daemon"
# Do NOT shadow the npm launcher with a second `meridian` on PATH. When installed
# via npm, /usr/local/bin/meridian (the launcher) owns `setup`/`update` and
# delegates start/stop/doctor to this bundle's CLI by its real path. ~/.local/bin
# usually precedes /usr/local/bin on PATH, so a ~/.local/bin/meridian symlink
# would hide `meridian setup` / `meridian update` (it can't reach the launcher).
# Only create the CLI symlink as a fallback when no launcher is present (e.g. a
# standalone bundle install); when the launcher exists, remove any stale shadow
# so `meridian update` self-heals an install made by an older bundle.
if [[ -e /usr/local/bin/meridian ]]; then
    rm -f "${HOME}/.local/bin/meridian"
    ok "meridian-daemon → ~/.local/bin  (meridian CLI = npm launcher at /usr/local/bin/meridian)"
else
    ln -sfn "${APP_ROOT}/scripts/meridian-cli.sh" "${HOME}/.local/bin/meridian"
    ok "meridian-daemon + meridian → ~/.local/bin"
fi

# ── 4. Python venv + MLX deps ────────────────────────────────────────────────
# uv reads services/uv.lock (hashed, cross-platform, committed) and installs the
# exact pinned set with no PyPI resolution at install time. On subsequent runs
# `uv sync --frozen` is a no-op when the venv is already up-to-date — faster than
# the old DEPS_STAMP approach and handles cross-version upgrades correctly.
# PYTHON_BIN is the brew-installed Python we know runs MLX/Metal; --python pins
# the interpreter used when uv creates a fresh venv (existing venvs are unchanged).
VENV="${APP_ROOT}/services/.venv"

info "Installing Python + MLX deps (mlx-lm/outlines/fastapi; first run may download a few hundred MB)…"
if "${UV_BIN}" sync \
        --project "${APP_ROOT}/services" \
        --extra mlx \
        --frozen \
        --python "${PYTHON_BIN}"; then
    ok "Python services ready ($(${VENV}/bin/python --version 2>&1))"
else
    warn "uv sync failed — leaving venv as-is; re-run 'meridian setup' to retry"
fi

# ── 5. macOS permissions for screenpipe (manual — can't be automated) ────────
if [[ "${SKIP_PERMISSIONS}" -eq 0 ]]; then
    echo "→ screenpipe needs 2 macOS permissions: Screen Recording and Accessibility."
    echo "  (Audio capture is disabled, so no Microphone permission is required.)"
    read -r -p "  Press Enter to open Screen Recording settings… " _ || true
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture" 2>/dev/null || true
    read -r -p "  Press Enter to open Accessibility settings… " _ || true
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" 2>/dev/null || true
    read -r -p "  Press Enter once both are granted… " _ || true
fi

# Enable a11y mode in installed VS Code-family editors (idempotent). Without
# this, screenpipe falls back to OCR for those editors instead of their a11y tree.
configure_editor_accessibility

# ── 5b. Unpack the dashboard (Turbopack standalone, shipped as a tarball) ─────
# The UI ships as ui.tar.gz rather than an expanded ui/ dir so that Turbopack's
# relative symlinks under .next/node_modules (serverExternalPackages: better-
# sqlite3, pino, @opentelemetry/*) survive `npm publish`, which strips symlinks.
# Extract the exact built tree into place before the UI agent starts.
if [[ -f "${APP_ROOT}/ui.tar.gz" ]]; then
    info "Unpacking dashboard…"
    rm -rf "${APP_ROOT}/ui"
    mkdir -p "${APP_ROOT}/ui"
    tar -xzf "${APP_ROOT}/ui.tar.gz" -C "${APP_ROOT}/ui"
    rm -f "${APP_ROOT}/ui.tar.gz"
    ok "dashboard unpacked ($(find "${APP_ROOT}/ui/.next/node_modules" -type l 2>/dev/null | wc -l | tr -d ' ') external symlink(s) restored)"
fi

# ── 6. Daemons (reuse the hardened installers; UI runs the standalone server) ─
info "Installing screenpipe launchd agent…"
bash "${APP_ROOT}/scripts/install-screenpipe-daemon.sh" || warn "screenpipe agent install failed"

info "Installing MLX inference server launchd agent…"
bash "${APP_ROOT}/services/scripts/install-mlx-server-daemon.sh" --port "${MLX_PORT}" || warn "MLX agent install failed"

# Wait for MLX to answer before starting the daemon (it hard-exits if MLX is down).
info "Waiting for the MLX server to load the model…"
_w=0
until curl -sf "http://127.0.0.1:${MLX_PORT}/health" >/dev/null 2>&1; do
    sleep 3; _w=$((_w+3)); [[ $_w -ge 300 ]] && { warn "MLX not ready after 300s — check: meridian logs mlx-server"; break; }
done
curl -sf "http://127.0.0.1:${MLX_PORT}/health" >/dev/null 2>&1 && ok "MLX server ready (${_w}s)"

info "Installing Rust daemon launchd agent…"
bash "${APP_ROOT}/scripts/install-daemon.sh" || warn "daemon agent install failed"

info "Installing the dashboard (UI) launchd agent…"
install_ui_standalone

ok "all daemons installed"

echo ""
echo "✓ Meridian installed at ${APP_ROOT}"
echo "  meridian status            # check the daemons"
echo "  meridian logs -f           # watch the pipeline"
echo "  meridian config edit       # add Jira creds"
echo "  open http://localhost:${UI_PORT} # the dashboard"
echo ""
echo "Jira worklogs are DRAFTED only — approve them in the dashboard (Worklogs"
echo "view) and the daemon posts approved worklogs within ~60s."
