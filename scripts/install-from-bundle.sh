#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Install a PREBUILT release bundle (no cargo/npm build). Run from inside an
# unpacked bundle at ~/.meridian/app — bootstrap.sh downloads + unpacks the
# release tarball and execs this. Installs prerequisites, builds the Python venv
# from PyPI via uv sync, and registers the four launchd daemons pointing at this bundle.
#
#   bash ~/.meridian/app/scripts/install-from-bundle.sh [--skip-permissions]
set -euo pipefail

APP_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCREENPIPE_VERSION="0.4.6"
MLX_PORT="${MLX_PORT:-7823}"
UI_PORT="${MERIDIAN_UI_PORT:-3939}"   # dashboard port (override via MERIDIAN_UI_PORT)
SKIP_PERMISSIONS=0
[[ "${1:-}" == "--skip-permissions" ]] && SKIP_PERMISSIONS=1

# Component-hash file lives OUTSIDE APP_ROOT so it survives `rm -rf` on updates.
# Used for differential installs: only restart daemons / re-extract assets that
# actually changed since the previous release.
_HASH_FILE="${HOME}/.meridian/.component-hashes"
_load_old_hash() { grep "^$1=" "${_HASH_FILE}" 2>/dev/null | cut -d= -f2 || true; }
_OLD_DAEMON_HASH="$(_load_old_hash daemon_bin)"
_OLD_UI_HASH="$(_load_old_hash ui_tarball)"
_OLD_TRAY_HASH="$(_load_old_hash tray_bin)"

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
        # OAuth-first; the prebuilt bundle binary is available, so log in inline.
        _connect_jira "$env_file" "${APP_ROOT}/bin/meridian"
    fi
    echo >&2
    if prompt_category "GitHub"; then
        if ! _try_gh_token "$env_file"; then
            echo >&2
            echo "    Alternatively, create a personal access token (classic) at:" >&2
            echo "      https://github.com/settings/tokens/new" >&2
            echo "    Required scopes: repo, read:org, read:project" >&2
            echo "    (read:project lets meridian read your GitHub Projects; repo posts worklog comments)" >&2
            echo >&2
            prompt_env_var "GITHUB_TOKEN" "GitHub personal access token" 1 "$env_file"
        fi
        _pick_github_projects "$env_file"
    fi
    echo >&2
    if prompt_category "Linear"; then
        prompt_env_var "LINEAR_API_KEY"  "Linear API key" 1 "$env_file"
        prompt_env_var "LINEAR_TEAM_IDS" "Linear team IDs (optional, comma-sep)" 0 "$env_file"
    fi
    echo >&2
    if prompt_category "Trello"; then
        _connect_trello "$env_file" "${APP_ROOT}/bin/meridian"
    fi
    ok "Credential collection complete"
}

# GitHub + Jira setup helpers — shared with install.sh.
source "${APP_ROOT}/scripts/lib-github-setup.sh"
source "${APP_ROOT}/scripts/lib-jira-setup.sh"
source "${APP_ROOT}/scripts/lib-trello-setup.sh"

GUI_TARGET="gui/$(id -u)"
LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"

# Resolve the Node runtime the dashboard must run on. The better-sqlite3 addon in
# ui.tar.gz is built against one exact Node version (recorded in
# bin/node-runtime.meta by package-release.sh); running any other Node major
# triggers a NODE_MODULE_VERSION (ABI) mismatch and the dashboard crash-loops.
# The 113 MB Node binary is NOT shipped through npm (it would blow the registry
# payload limit), so we download that exact official build from nodejs.org once,
# verify the pinned SHA-256, and cache it under ~/.meridian (survives the APP_ROOT
# rm-rf on `meridian update`). Echoes the node path on stdout. Failure fallbacks
# to system node are LOUD because they may not match the addon's ABI.
resolve_node_runtime() {
    local meta="${APP_ROOT}/bin/node-runtime.meta"
    # Dev/source install (no meta file): use system/Homebrew node as-is. Such a
    # build compiles its own better-sqlite3 against that node, so ABI matches.
    if [[ ! -f "${meta}" ]]; then
        local _n
        for _n in /opt/homebrew/bin/node /usr/local/bin/node /usr/bin/node; do
            [[ -x "${_n}" ]] && { echo "${_n}"; return 0; }
        done
        return 1
    fi
    local ver sha
    ver="$(grep '^NODE_RUNTIME_VERSION=' "${meta}" | cut -d= -f2 | tr -d '[:space:]')"
    sha="$(grep '^NODE_RUNTIME_SHA=' "${meta}" | cut -d= -f2 | tr -d '[:space:]')"
    if [[ -z "${ver}" || -z "${sha}" ]]; then
        warn "node-runtime.meta is malformed (missing VERSION or SHA) — falling back to system node"
        for _n in /opt/homebrew/bin/node /usr/local/bin/node /usr/bin/node; do
            [[ -x "${_n}" ]] && { echo "${_n}"; return 0; }
        done
        return 1
    fi
    local cache_dir="${HOME}/.meridian/node-runtime/v${ver}"
    local cache_bin="${cache_dir}/bin/node"
    if [[ -x "${cache_bin}" ]]; then echo "${cache_bin}"; return 0; fi
    local tmp tgz url got
    tmp="$(mktemp -d)"; tgz="${tmp}/node.tar.gz"
    url="https://nodejs.org/dist/v${ver}/node-v${ver}-darwin-arm64.tar.gz"
    info "Downloading Node ${ver} runtime for the dashboard (one-time, ~40 MB)…"
    if curl -fsSL --retry 3 "${url}" -o "${tgz}"; then
        got="$(shasum -a 256 "${tgz}" | cut -d' ' -f1)"
        if [[ "${got}" == "${sha}" ]]; then
            tar -xzf "${tgz}" -C "${tmp}"
            rm -rf "${cache_dir}"; mkdir -p "$(dirname "${cache_dir}")"
            mv "${tmp}/node-v${ver}-darwin-arm64" "${cache_dir}"
            rm -rf "${tmp}"
            ok "Node ${ver} runtime cached (ABI-matched to the dashboard)"
            echo "${cache_bin}"; return 0
        fi
        warn "Node ${ver} SHA-256 mismatch (expected ${sha}, got ${got}) — not using it"
    else
        warn "Node ${ver} download failed (offline?) — the dashboard needs it to match better-sqlite3's ABI"
    fi
    rm -rf "${tmp}"
    local _n
    for _n in /opt/homebrew/bin/node /usr/local/bin/node /usr/bin/node; do
        if [[ -x "${_n}" ]]; then
            warn "Falling back to ${_n} — if the dashboard fails to load, re-run 'meridian update' with a connection"
            echo "${_n}"; return 0
        fi
    done
    return 1
}

# Register the dashboard as a launchd agent that runs the prebuilt Next.js
# standalone server (`node ui/server.js`) — no `npm start`, no node_modules
# install. Mirrors the EIO-safe bootout/bootstrap pattern of the other agents.
install_ui_standalone() {
    local label="com.meridiona.ui"
    local plist="${LAUNCH_AGENTS}/${label}.plist"
    # Resolve the ABI-matched Node runtime (downloads + caches it on first use).
    local node_bin
    node_bin="$(resolve_node_runtime)" || { err "node not found — install Node.js: brew install node"; return 1; }
    local start_script="${APP_ROOT}/scripts/ui-start.sh"
    chmod +x "${start_script}" 2>/dev/null || true
    mkdir -p "${HOME}/.meridian/logs" "${LAUNCH_AGENTS}"
    cat > "${plist}" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>${label}</string>
  <key>ProgramArguments</key>
  <array><string>${start_script}</string></array>
  <key>WorkingDirectory</key><string>${APP_ROOT}/ui</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PORT</key><string>${UI_PORT}</string>
    <key>HOSTNAME</key><string>127.0.0.1</string>
    <key>MERIDIAN_DB_PATH</key><string>${HOME}/.meridian/meridian.db</string>
    <key>MERIDIAN_NODE_BIN</key><string>${node_bin}</string>
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
    # App bundles, index-matched to `editors`, for running-process detection.
    local app_bundles=("Visual Studio Code.app" "Cursor.app" "Antigravity IDE.app")
    local any=0 i ed dir settings
    local needs_restart=()
    for i in "${!editors[@]}"; do
        ed="${editors[$i]}"
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
            # Sanity check: the key must appear in the file; if not, the regex
            # found no `{` (unusual) or perl silently failed — restore backup.
            if ! grep -q '"editor.accessibilitySupport"' "$settings" 2>/dev/null; then
                warn "${ed}: settings.json edit failed — restoring backup"
                cp "${settings}.meridian-bak" "$settings"
                continue
            fi
        fi
        ok "${ed}: enabled editor.accessibilitySupport = on"
        # The setting is read ONCE at editor boot. If the editor is running
        # right now, it booted before this write and will keep capturing
        # nothing until relaunched — these apps routinely run for days, so
        # without an explicit restart the setting sits inert on disk and the
        # editor's activity is silently invisible to screenpipe.
        if pgrep -qf "/Applications/${app_bundles[$i]}/" 2>/dev/null; then
            needs_restart+=("${ed}")
        fi
    done
    if [[ ${#needs_restart[@]} -gt 0 ]]; then
        warn "RESTART REQUIRED: ${needs_restart[*]} — running editors only read"
        warn "editor.accessibilitySupport at launch. Quit and reopen them now, or"
        warn "their activity will NOT be captured until the next relaunch."
    fi
    [[ "$any" -eq 0 ]] && info "No VS Code / Cursor / Antigravity install found — skipping editor a11y setup"
    return 0
}

# ── 0. Platform gate ────────────────────────────────────────────────────────
[[ "$(uname -s)" == "Darwin" ]]  || { err "Meridian requires macOS."; exit 1; }
# hw.optional.arm64 reports the HARDWARE: it stays 1 in a Rosetta (x86_64)
# shell on Apple Silicon, where uname -m would lie and wrongly refuse.
[[ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" == "1" ]] \
    || { err "Meridian requires Apple Silicon (arm64). This bundle is macOS-arm64 only."; exit 1; }

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
if [[ -z "${PYTHON_BIN}" ]]; then
    info "Installing Python 3.11…"
    brew install python@3.11
    # `command -v` may return empty in a non-interactive shell immediately after
    # `brew install` because the formula's bin dir isn't in launchd's PATH yet.
    # Resolve via `brew --prefix` which is always accurate.
    PYTHON_BIN="$(brew --prefix python@3.11)/bin/python3.11"
    [[ -x "${PYTHON_BIN}" ]] || PYTHON_BIN="$(command -v python3.11 2>/dev/null || true)"
fi
# uv is the package/venv manager for Python services. Install via Homebrew (already
# required by this installer) rather than the astral curl|sh installer.
UV_BIN=""
if command -v uv >/dev/null 2>&1; then
    UV_BIN="$(command -v uv)"
else
    info "Installing uv (Python package manager)…"
    brew install uv
    UV_BIN="$(brew --prefix uv)/bin/uv"
    [[ -x "${UV_BIN}" ]] || UV_BIN="$(command -v uv 2>/dev/null || true)"
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
# Stage the real screenpipe Mach-O to ~/.meridian/bin/screenpipe — a stable path
# that is independent of the npm prefix (nvm users get a version-specific path
# under ~/.nvm that breaks on `nvm use` and is too deep to navigate in System
# Settings). The launchd plist and TCC grants are written against this path.
mkdir -p "${HOME}/.meridian/bin"
_sp_npm_root="$(npm root -g 2>/dev/null || true)"
_sp_real=""
if [[ -n "${_sp_npm_root}" && -d "${_sp_npm_root}/screenpipe" ]]; then
    while IFS= read -r _sp_cand; do
        if file "${_sp_cand}" 2>/dev/null | grep -q "Mach-O"; then _sp_real="${_sp_cand}"; break; fi
    done < <(find "${_sp_npm_root}/screenpipe" -type f -name screenpipe -perm +0111 2>/dev/null)
fi
if [[ -n "${_sp_real}" ]]; then
    cmp -s "${_sp_real}" "${HOME}/.meridian/bin/screenpipe" 2>/dev/null \
        || cp "${_sp_real}" "${HOME}/.meridian/bin/screenpipe"
    chmod +x "${HOME}/.meridian/bin/screenpipe"
    ok "screenpipe staged → ${HOME}/.meridian/bin/screenpipe"
fi
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
# Put a single `meridian` on PATH that owns every command. The npm launcher
# (`meridian.js`) owns `setup`/`update` and delegates start/stop/doctor to this
# bundle's CLI; the CLI alone does NOT know `setup`/`update`. So we point
# ~/.local/bin/meridian AT the launcher (not the CLI) whenever a launcher exists.
#
# Why ~/.local/bin and not rely on the npm bin dir being on PATH: the launcher's
# own bin dir depends on the npm prefix (/usr/local/bin by default, but
# ~/.npm-global/bin when the EACCES bootstrap redirected the prefix). That dir is
# only added to PATH via a shell-rc patch, which doesn't apply to already-open
# shells and may never apply for bash users. ~/.local/bin is a standard user bin
# dir that is reliably on PATH already, so pointing it at the launcher makes
# `meridian update` work in every shell immediately — no rc reload required, no
# CLI shadow hiding `update`. Fall back to the CLI only for a standalone bundle
# install where no npm launcher is present.
_launcher=""
_npm_prefix="$(npm config get prefix 2>/dev/null || true)"
for _cand in ${_npm_prefix:+"${_npm_prefix}/bin/meridian"} "/usr/local/bin/meridian"; do
    [[ -e "${_cand}" ]] || continue
    # Distinguish the launcher (node shim) from a self-referential CLI symlink:
    # the launcher never resolves to meridian-cli.sh.
    if [[ "$(readlink "${_cand}" 2>/dev/null || echo "${_cand}")" != *meridian-cli.sh ]]; then
        _launcher="${_cand}"; break
    fi
done
if [[ -n "${_launcher}" ]]; then
    ln -sfn "${_launcher}" "${HOME}/.local/bin/meridian"
    ok "meridian-daemon + meridian → ~/.local/bin  (meridian → npm launcher at ${_launcher})"
else
    ln -sfn "${APP_ROOT}/scripts/meridian-cli.sh" "${HOME}/.local/bin/meridian"
    ok "meridian-daemon + meridian → ~/.local/bin  (CLI; no npm launcher found)"
fi

# ── 3b. Detect component changes for differential restart ────────────────────
_new_daemon_hash="$(shasum -a 256 "${APP_ROOT}/bin/meridian" 2>/dev/null | cut -d' ' -f1 || true)"
_daemon_changed=1
[[ -n "${_OLD_DAEMON_HASH}" && -n "${_new_daemon_hash}" && \
   "${_new_daemon_hash}" == "${_OLD_DAEMON_HASH}" ]] && _daemon_changed=0

_new_tray_hash="$(shasum -a 256 "${APP_ROOT}/bin/meridian-tray" 2>/dev/null | cut -d' ' -f1 || true)"
_tray_changed=1
[[ -n "${_OLD_TRAY_HASH}" && -n "${_new_tray_hash}" && \
   "${_new_tray_hash}" == "${_OLD_TRAY_HASH}" ]] && _tray_changed=0

# Snapshot MLX health before any venv work — if already healthy and services
# don't change we skip the restart + model-load wait entirely.
_mlx_was_healthy=0
curl -sf "http://127.0.0.1:${MLX_PORT}/health" >/dev/null 2>&1 && _mlx_was_healthy=1

# ── 4. Python venv + MLX deps ────────────────────────────────────────────────
# Installed from PyPI via uv sync at install time — no pre-built venv shipped.
# PyPI MLX wheels are arm64-only (mlx is Metal-based), so the venv interpreter
# MUST be a native arm64 CPython. PATH python3 is NOT trustworthy here: user
# machines often carry x86_64 builds running under Rosetta (Intel Homebrew,
# pyenv), which fail the mlx resolve outright — or worse, leave a
# mixed-architecture venv behind. So the venv is built from a uv-MANAGED
# interpreter pinned by full build key: deterministic on every machine,
# independent of whatever python3 (or uv binary arch) the user has.
# Differential skip: if uv.lock + interpreter build are unchanged, the venv
# python is really arm64, and mlx.core imports cleanly — no network round-trip.
MERIDIAN_PY_BUILD="cpython-3.11-macos-aarch64-none"
VENV="${APP_ROOT}/services/.venv"
VENV_STAMP="${VENV}/.meridian-venv-hash"
_LOCK_HASH="$(shasum -a 256 "${APP_ROOT}/services/uv.lock" 2>/dev/null | cut -d' ' -f1 || true)"
# Stamp records lock hash + interpreter build — bumping either rebuilds the venv.
_WANT_STAMP="${_LOCK_HASH} ${MERIDIAN_PY_BUILD}"

_venv_changed=1
_have_stamp=""; [[ -f "${VENV_STAMP}" ]] && _have_stamp="$(cat "${VENV_STAMP}" 2>/dev/null)"
_venv_arch=""
[[ -x "${VENV}/bin/python" ]] \
    && _venv_arch="$("${VENV}/bin/python" -c 'import platform; print(platform.machine())' 2>/dev/null || true)"

if [[ -n "${_LOCK_HASH}" && "${_WANT_STAMP}" == "${_have_stamp}" && "${_venv_arch}" == "arm64" ]] \
   && "${VENV}/bin/python" -c "import mlx.core" 2>/dev/null; then
    ok "Python deps unchanged — skipping uv sync"
    _venv_changed=0
else
    # Self-heal: a venv whose interpreter is not arm64 (built by a Rosetta
    # python3 before this pin existed) can never be fixed in place — wipe it.
    if [[ -d "${VENV}" && -n "${_venv_arch}" && "${_venv_arch}" != "arm64" ]]; then
        warn "venv interpreter is ${_venv_arch}, not arm64 (mixed-architecture venv) — rebuilding from scratch"
        rm -rf "${VENV}"
    fi
    "${UV_BIN}" python install "${MERIDIAN_PY_BUILD}" \
        || warn "managed Python pre-install failed — uv sync retries the download"
    info "Installing Python services from PyPI (mlx-lm/outlines/fastapi/agno; first run ~40–120s)…"
    if "${UV_BIN}" sync \
            --project "${APP_ROOT}/services" \
            --extra mlx \
            --extra pm_worklog_update \
            --frozen \
            --python "${MERIDIAN_PY_BUILD}" \
            --python-preference only-managed; then
        [[ -n "${_LOCK_HASH}" ]] && printf '%s\n' "${_WANT_STAMP}" > "${VENV_STAMP}"
        ok "Python services ready ($("${VENV}/bin/python" --version 2>&1), $("${VENV}/bin/python" -c 'import platform; print(platform.machine())' 2>/dev/null))"
    else
        warn "uv sync failed — leaving venv as-is; re-run 'meridian setup' to retry"
    fi
fi

# On macOS 26+, install apple-fm-sdk so Apple Intelligence is used instead of
# downloading a large MLX model. Runs after uv sync so the venv exists.
# apple-fm-sdk only installs on macOS 26+ (links against system frameworks);
# on older macOS pip will fail gracefully and MLX is used as the fallback.
_macos_major="$(sw_vers -productVersion 2>/dev/null | cut -d. -f1)"
if [[ "${_macos_major:-0}" -ge 26 ]]; then
    if "${VENV}/bin/python" -c "import apple_fm_sdk" 2>/dev/null; then
        ok "apple-fm-sdk already installed — Apple Intelligence will be used"
    else
        info "macOS ${_macos_major} detected — installing apple-fm-sdk for Apple Intelligence (no MLX model download needed)…"
        if "${UV_BIN}" pip install --python "${VENV}/bin/python" --quiet "apple-fm-sdk" 2>/dev/null; then
            ok "apple-fm-sdk installed — Apple Intelligence will be used"
        else
            warn "apple-fm-sdk install failed — MLX model download will be used instead"
        fi
    fi
fi

# ── 5. macOS permissions for screenpipe (manual — can't be automated) ────────
# Stage the a11y helper binary first so its path exists when the user adds it
# in the Accessibility pane below (the agent itself is installed in §6).
if [[ -f "${APP_ROOT}/scripts/a11y-helper/meridian-a11y-helper" ]]; then
    mkdir -p "${HOME}/.meridian/bin"
    cmp -s "${APP_ROOT}/scripts/a11y-helper/meridian-a11y-helper" "${HOME}/.meridian/bin/meridian-a11y-helper" 2>/dev/null \
        || cp "${APP_ROOT}/scripts/a11y-helper/meridian-a11y-helper" "${HOME}/.meridian/bin/meridian-a11y-helper"
    chmod +x "${HOME}/.meridian/bin/meridian-a11y-helper"
fi
if [[ "${SKIP_PERMISSIONS}" -eq 0 ]]; then
    echo "→ screenpipe needs 2 macOS permissions: Screen Recording and Accessibility."
    echo "  (Audio capture is disabled, so no Microphone permission is required.)"
    echo "  You will add these 2 binaries — both live at the same stable path:"
    echo "      ${HOME}/.meridian/bin/screenpipe          ← Screen Recording + Accessibility"
    echo "      ${HOME}/.meridian/bin/meridian-a11y-helper ← Accessibility only"
    echo "  In each pane: click '+' → press ⌘⇧G → paste the path above → Open → toggle ON."
    read -r -p "  Press Enter to open Screen Recording settings… " _ || true
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture" 2>/dev/null || true
    echo "  Add: ${HOME}/.meridian/bin/screenpipe"
    read -r -p "  Press Enter to open Accessibility settings… " _ || true
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" 2>/dev/null || true
    echo "  Add both:"
    echo "      ${HOME}/.meridian/bin/screenpipe"
    echo "      ${HOME}/.meridian/bin/meridian-a11y-helper"
    echo "  Without the a11y helper, Electron apps (Claude, Codex, Slack, …) stay invisible to capture."
    read -r -p "  Press Enter once all are granted… " _ || true
fi

# Enable a11y mode in installed VS Code-family editors (idempotent). Without
# this, screenpipe falls back to OCR for those editors instead of their a11y tree.
configure_editor_accessibility

# ── 5b. Unpack the dashboard (Turbopack standalone, shipped as a tarball) ─────
# The UI ships as ui.tar.gz rather than an expanded ui/ dir so that Turbopack's
# relative symlinks under .next/node_modules (serverExternalPackages: better-
# sqlite3, pino, @opentelemetry/*) survive `npm publish`, which strips symlinks.
# When meridian-npm-setup.sh detected the tarball hash was unchanged it preserved
# the existing ui/ dir and deleted ui.tar.gz — in that case skip extraction too.
_ui_changed=1
_new_ui_hash=""
if [[ -f "${APP_ROOT}/ui.tar.gz" ]]; then
    _new_ui_hash="$(shasum -a 256 "${APP_ROOT}/ui.tar.gz" | cut -d' ' -f1)"
    info "Unpacking dashboard…"
    rm -rf "${APP_ROOT}/ui"
    mkdir -p "${APP_ROOT}/ui"
    tar -xzf "${APP_ROOT}/ui.tar.gz" -C "${APP_ROOT}/ui"
    rm -f "${APP_ROOT}/ui.tar.gz"
    ok "dashboard unpacked ($(find "${APP_ROOT}/ui/.next/node_modules" -type l 2>/dev/null | wc -l | tr -d ' ') external symlink(s) restored)"
elif [[ -d "${APP_ROOT}/ui" ]]; then
    # ui/ was preserved by meridian-npm-setup.sh — hash matched, no re-extraction needed
    ok "dashboard unchanged — reusing existing build"
    _ui_changed=0
else
    err "Dashboard bundle missing from ${APP_ROOT} — re-run the installer: curl -fsSL https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.sh | bash"
    exit 1
fi

# ── 6. Daemons — restart only what changed ───────────────────────────────────
# screenpipe: external npm binary, plist may have changed → always refresh.
info "Installing screenpipe launchd agent…"
bash "${APP_ROOT}/scripts/install-screenpipe-daemon.sh" || warn "screenpipe agent install failed"

# a11y-helper: enables accessibility on Electron apps so screenpipe can
# capture them (Claude, Codex, Slack, …) — see scripts/a11y-helper/main.swift.
info "Installing a11y-helper launchd agent…"
bash "${APP_ROOT}/scripts/install-a11y-helper-daemon.sh" || warn "a11y-helper agent install failed"

# MLX: skip restart + model-load wait when server was already healthy and
# neither the venv nor the Python source files changed.
_PY_SRC_STAMP="${HOME}/.meridian/py-src.sha256"
_py_src_hash="$(find "${APP_ROOT}/services/agents" -name '*.py' | sort | xargs shasum -a 256 2>/dev/null | shasum -a 256 | cut -d' ' -f1 || true)"
_py_src_changed=1
if [[ -f "${_PY_SRC_STAMP}" && "$(cat "${_PY_SRC_STAMP}")" == "${_py_src_hash}" ]]; then
    _py_src_changed=0
fi
if [[ "${_mlx_was_healthy}" -eq 1 && "${_venv_changed}" -eq 0 && "${_py_src_changed}" -eq 0 ]]; then
    ok "Python services unchanged — MLX server kept running"
    # Stamp only when the server is confirmed healthy (skip case = already healthy).
    echo "${_py_src_hash}" > "${_PY_SRC_STAMP}"
else
    info "Installing MLX inference server launchd agent…"
    bash "${APP_ROOT}/services/scripts/install-mlx-server-daemon.sh" --port "${MLX_PORT}" || warn "MLX agent install failed"
    info "Waiting for the MLX server to load the model…"
    _MLX_LOG="${HOME}/.meridian/logs/mlx-server.log"
    _MLX_ERR="${HOME}/.meridian/logs/mlx-server-error.log"
    _w=0

    # Stream both log files while polling so the user can see which model is
    # loading and whether a download is in progress.
    # mlx-server.log  — JSON structured logs: model selection, readiness
    # mlx-server-error.log — raw stderr: huggingface_hub download progress
    # tail -F follows by name and retries if the file doesn't exist yet.
    (tail -F -n 0 "${_MLX_LOG}" 2>/dev/null | python3 -u -c '
import sys, json
for line in sys.stdin:
    line = line.rstrip()
    if not line:
        continue
    try:
        d = json.loads(line)
        lvl = d.get("level", "INFO")
        msg = d.get("message", line)
        prefix = "  ⚠ " if lvl in ("WARNING", "ERROR") else "  · "
        print(prefix + msg, flush=True)
    except Exception:
        print("  " + line, flush=True)
') &
    _log_pid=$!
    (tail -F -n 0 "${_MLX_ERR}" 2>/dev/null | while IFS= read -r _eline; do
        # tqdm progress lines contain '%' — update in-place with \r so users
        # see a live download bar instead of a flood of static lines.
        if [[ "${_eline}" == *%* && "${_eline}" == *it/s* || "${_eline}" == *Fetching* ]]; then
            printf '\r  %-80s' "${_eline}"
        else
            printf '\n  %s' "${_eline}"
        fi
    done; printf '\n') &
    _err_pid=$!

    until curl -sf "http://127.0.0.1:${MLX_PORT}/health" >/dev/null 2>&1; do
        sleep 3; _w=$((_w+3))
        [[ $_w -ge 300 ]] && { warn "MLX not ready after 300s — check: meridian logs mlx-server"; break; }
    done
    # kill python3/bash first, then explicitly kill the tail processes —
    # tail -F ignores SIGPIPE and survives as an orphan if only the reader dies.
    { kill "${_log_pid}" "${_err_pid}" 2>/dev/null; \
      pkill -f "tail.*mlx-server\\.log" 2>/dev/null; \
      pkill -f "tail.*mlx-server-error\\.log" 2>/dev/null; \
      wait "${_log_pid}" "${_err_pid}" 2>/dev/null; } || true

    # Only stamp after confirmed ready — prevents stale stamp on a failed restart.
    if curl -sf "http://127.0.0.1:${MLX_PORT}/health" >/dev/null 2>&1; then
        ok "MLX server ready (${_w}s)"
        echo "${_py_src_hash}" > "${_PY_SRC_STAMP}"
    fi
fi

# Rust daemon: skip restart when binary is identical.
if [[ "${_daemon_changed}" -eq 0 ]]; then
    ok "Rust daemon unchanged — skipping restart"
else
    info "Installing Rust daemon launchd agent…"
    bash "${APP_ROOT}/scripts/install-daemon.sh" || warn "daemon agent install failed"
fi

# Tray app: skip restart when the binary is identical.
if [[ -x "${APP_ROOT}/bin/meridian-tray" ]]; then
    if [[ "${_tray_changed}" -eq 0 ]]; then
        ok "Tray app unchanged — skipping restart"
    else
        info "Installing Meridian Tray launchd agent…"
        bash "${APP_ROOT}/scripts/install-tray-daemon.sh" || warn "tray agent install failed"
    fi
else
    warn "meridian-tray binary not found — tray app not installed (not yet in this release)"
fi

# Claude Code SessionEnd hook: seals each Claude session into app_sessions the
# instant it ends (the indexer sweep + 1 h idle seal are only the fallback).
# Idempotent merge into ~/.claude/settings.json; also purges retired Python
# hook entries on upgrade. Pin the binary to the installed bundle copy.
info "Installing Claude Code coding-agent SessionEnd hook…"
if MERIDIAN_BIN="${APP_ROOT}/bin/meridian" bash "${APP_ROOT}/services/scripts/install-claude-hook.sh" >/dev/null 2>&1; then
    ok "Claude Code SessionEnd hook installed"
else
    warn "coding-agent hook install skipped (Claude sessions still seal via the idle backstop)"
fi

# Coding-agent summariser engines (informational): each agent's sessions are
# summarised by its OWN CLI when present; a missing CLI is never fatal — those
# sessions fall back to the local MLX model. Surface what the daemon will use
# so users know why a summary came from MLX. `meridian doctor` re-checks all
# of this any time.
info "Coding-agent summariser engines:"
for _eng in claude codex copilot; do
    if command -v "${_eng}" >/dev/null 2>&1; then
        ok "${_eng} CLI found — those sessions summarise natively"
    else
        info "  ${_eng} CLI not found — those sessions will use the local model (MLX)"
    fi
done
if command -v cursor >/dev/null 2>&1 || [[ -d "${HOME}/Library/Application Support/Cursor" ]]; then
    if command -v cursor-agent >/dev/null 2>&1; then
        ok "cursor-agent CLI found — verify auth with: cursor-agent status"
    else
        info "  Cursor detected but the cursor-agent CLI is missing — Cursor summaries will use the local model (MLX)."
        info "  To summarise with Cursor's own CLI:  curl https://cursor.com/install -fsS | bash  then: cursor-agent login"
        info "  Or let the daemon install it on demand: add CURSOR_AGENT_AUTO_INSTALL=1 to ${HOME}/.meridian/app/.env"
    fi
fi

# UI: skip daemon restart when the build didn't change (tarball hash matched).
if [[ "${_ui_changed}" -eq 0 ]]; then
    ok "Dashboard unchanged — skipping restart"
else
    info "Installing the dashboard (UI) launchd agent…"
    install_ui_standalone
fi

# Persist component hashes for the next update's differential check.
# Write to a temp file and rename atomically so a crash mid-write never leaves
# a half-written or empty hash file (which would force a full reinstall).
_final_ui_hash="${_new_ui_hash:-${_OLD_UI_HASH}}"
_final_tray_hash="${_new_tray_hash:-${_OLD_TRAY_HASH}}"
{
    [[ -n "${_new_daemon_hash}" ]] && printf 'daemon_bin=%s\n' "${_new_daemon_hash}"
    [[ -n "${_final_ui_hash}" ]] && printf 'ui_tarball=%s\n' "${_final_ui_hash}"
    [[ -n "${_final_tray_hash}" ]] && printf 'tray_bin=%s\n' "${_final_tray_hash}"
} > "${_HASH_FILE}.tmp" && mv "${_HASH_FILE}.tmp" "${_HASH_FILE}"

ok "all daemons installed"

# Install session-summary Claude Code command so `claude -p /session-summary` resolves.
_skill_src="${APP_ROOT}/services/skills/coding-agent/session-summary/SKILL.md"
_skill_dst="${HOME}/.claude/commands/session-summary.md"
mkdir -p "${HOME}/.claude/commands"
if [[ -f "${_skill_src}" ]]; then
    cp "${_skill_src}" "${_skill_dst}"
    ok "session-summary command → ~/.claude/commands/session-summary.md"
else
    warn "session-summary skill not found in bundle — skipping (${_skill_src})"
fi

# Pipeline smoke test — verify both LLM stages return valid output (no DB writes).
echo ""
info "Running pipeline smoke test (this exercises the model — may take ~30s)…"
if bash "${APP_ROOT}/scripts/meridian-cli.sh" smoke; then
    ok "pipeline smoke passed — classification and worklog synthesis are working"
else
    warn "pipeline smoke found issues — run 'meridian doctor' for remedies"
fi

echo ""
echo "✓ Meridian installed at ${APP_ROOT}"
echo "  meridian status            # check the daemons"
echo "  meridian logs -f           # watch the pipeline"
echo "  meridian config edit       # add Jira creds"
echo "  open http://localhost:${UI_PORT} # the dashboard"
echo ""
echo "Jira worklogs are DRAFTED only — approve them in the dashboard (Worklogs"
echo "view) and the daemon posts approved worklogs within ~60s."
