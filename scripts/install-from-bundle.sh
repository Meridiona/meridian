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
SKIP_PERMISSIONS=0
[[ "${1:-}" == "--skip-permissions" ]] && SKIP_PERMISSIONS=1

info() { echo "→ $*" >&2; }
ok()   { echo "  ✓ $*" >&2; }
warn() { echo "  ⚠ $*" >&2; }
err()  { echo "✗ $*" >&2; }

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
    <key>PORT</key><string>3000</string>
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

# ── 0. Platform gate ────────────────────────────────────────────────────────
[[ "$(uname -s)" == "Darwin" ]]  || { err "Meridian requires macOS."; exit 1; }
[[ "$(uname -m)" == "arm64" ]]   || { err "Meridian requires Apple Silicon (arm64). This bundle is macOS-arm64 only."; exit 1; }

echo "→ Installing Meridian $(cat "${APP_ROOT}/VERSION" 2>/dev/null || echo '?') from ${APP_ROOT}"

# ── 1. Prerequisites (no Rust/Node-build toolchain — artifacts are prebuilt) ──
if ! command -v brew >/dev/null 2>&1; then
    err "Homebrew required — install from https://brew.sh and re-run."; exit 1
fi
command -v node >/dev/null 2>&1 || { info "Installing Node.js…"; brew install node; }
PYTHON_BIN=""
for p in python3.11 python3; do command -v "$p" >/dev/null 2>&1 && { PYTHON_BIN="$(command -v "$p")"; break; }; done
[[ -n "${PYTHON_BIN}" ]] || { info "Installing Python 3.11…"; brew install python@3.11; PYTHON_BIN="$(command -v python3.11)"; }
ok "node + python ($(${PYTHON_BIN} --version 2>&1))"

if ! command -v screenpipe >/dev/null 2>&1; then
    info "Installing screenpipe ${SCREENPIPE_VERSION} via npm…"
    npm install -g "screenpipe@${SCREENPIPE_VERSION}"
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
ok "config at ${ENV_FILE}"

# ── 3. Binary + CLI symlinks ─────────────────────────────────────────────────
mkdir -p "${HOME}/.local/bin"
ln -sfn "${APP_ROOT}/bin/meridian"        "${HOME}/.local/bin/meridian-daemon"
ln -sfn "${APP_ROOT}/scripts/meridian-cli.sh" "${HOME}/.local/bin/meridian"
ok "meridian-daemon + meridian → ~/.local/bin"

# ── 4. Python venv + MLX deps (the one install-time download) ────────────────
info "Setting up Python venv + MLX inference deps (downloads ~ a few hundred MB)…"
VENV="${APP_ROOT}/services/.venv"
[[ -d "${VENV}" ]] || "${PYTHON_BIN}" -m venv "${VENV}"
"${VENV}/bin/pip" install --quiet --upgrade pip
"${VENV}/bin/pip" install --quiet -e "${APP_ROOT}/services[mlx]"
ok "Python services ready"

# ── 5. macOS permissions for screenpipe (manual — can't be automated) ────────
if [[ "${SKIP_PERMISSIONS}" -eq 0 ]]; then
    echo "→ screenpipe needs 3 macOS permissions: Screen Recording, Accessibility, Microphone."
    read -r -p "  Press Enter to open Screen Recording settings… " _ || true
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture" 2>/dev/null || true
    read -r -p "  Press Enter to open Accessibility settings… " _ || true
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" 2>/dev/null || true
    read -r -p "  Press Enter once both are granted (Microphone is requested on first run)… " _ || true
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
echo "  open http://localhost:3000 # the dashboard"
echo ""
echo "Jira worklog posting is OFF by default — set PM_WORKLOG_POST_ENABLED=true"
echo "in ${ENV_FILE} when you're ready to write worklogs."
