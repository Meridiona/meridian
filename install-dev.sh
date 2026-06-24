#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Dev-mode install: build deps and register the infrastructure launchd agents
# (OpenObserve) plus the Claude Code integrations the production install ships
# (SessionEnd hook, session-summary command). The Rust daemon, MLX server, and
# Tauri tray are NOT registered as launchd agents — run them in watch mode via:
#   bash dev-start.sh
#
# Capture (v1.64.0+): runs in-process inside the Tauri tray binary — no
# separate screenpipe or a11y-helper launchd agent is needed or registered.
# If you have those agents from a previous dev setup, remove them:
#   launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.screenpipe.plist
#   launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.a11y-helper.plist
#
# Parity rule: everything a production install provides must exist in dev too —
# infrastructure under launchd, actively-developed services via dev-start.sh
# hot-reload. If install.sh gains a new component, add it here or to
# dev-start.sh, never neither.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Run main install: builds Rust (debug), installs UI/tray deps, sets up Python
# venv + MLX. --no-daemon skips all launchd registration (capture agents are
# not needed — capture runs in-process inside the Tauri tray).
bash "${REPO_ROOT}/install.sh" --dev --no-daemon "$@"

# Write 'dev' into ~/.meridian/app/VERSION so the UI update-available banner
# never shows in dev mode (the version check returns false when current='dev').
mkdir -p "${HOME}/.meridian/app"
echo "dev" > "${HOME}/.meridian/app/VERSION"
echo "  ✓ ~/.meridian/app/VERSION set to 'dev' (suppresses update banner)"

# OpenObserve — same guard as install.sh's daemon block: register the agent only
# when the binary is present (install.sh's prereq step offers the download).
# Without this, dev observability work has no OTLP backend while prod does.
if command -v openobserve >/dev/null 2>&1 || [[ -x "${HOME}/.openobserve/openobserve" ]]; then
    echo "→ Installing OpenObserve launchd agent..."
    if bash "${REPO_ROOT}/scripts/install-openobserve-daemon.sh"; then
        echo "  ✓ OpenObserve registered"
    else
        echo "  ⚠ OpenObserve agent install skipped (set MERIDIAN_OO_AUTH in <repo>/.env to enable)"
    fi
else
    echo "  ⚠ OpenObserve not installed — skipping its launchd agent (optional)"
fi

# Claude Code integrations — prod installs these inside install.sh's daemon
# block, which --no-daemon skips; without them dev machines silently lose
# real-time coding-agent session sealing and the /session-summary command.
echo "→ Installing Claude Code coding-agent SessionEnd hook..."
if bash "${REPO_ROOT}/services/scripts/install-claude-hook.sh"; then
    echo "  ✓ SessionEnd hook installed"
else
    echo "  ⚠ coding-agent hook install skipped"
fi

echo "→ Installing session-summary Claude Code command..."
_skill_src="${REPO_ROOT}/services/skills/coding-agent/session-summary/SKILL.md"
if [[ -f "${_skill_src}" ]]; then
    mkdir -p "${HOME}/.claude/commands"
    cp "${_skill_src}" "${HOME}/.claude/commands/session-summary.md"
    echo "  ✓ session-summary command installed"
else
    echo "  ⚠ session-summary command skipped (source not found: ${_skill_src})"
fi

echo ""
echo "✓ Dev environment ready."
echo ""
echo "Start all services with hot-reload:"
echo "  bash dev-start.sh"
echo ""
echo "What dev-start.sh opens (3 Terminal windows):"
echo "  1. Rust daemon   — cargo watch, rebuilds on every .rs save"
echo "  2. MLX server    — uvicorn --reload, reloads on .py changes"
echo "  3. Tauri tray    — npm run tauri dev (starts Next.js hot-reload automatically)"
echo ""
echo "OpenObserve runs via launchd and restarts automatically (if installed)."
echo "Capture runs in-process inside the Tauri tray — no separate agent needed."
