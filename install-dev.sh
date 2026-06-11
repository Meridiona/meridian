#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Dev-mode install: build deps and register only the infrastructure launchd agents
# (screenpipe, a11y-helper). The Rust daemon and MLX server are NOT registered
# as launchd agents — run them in watch mode via: bash dev-start.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Run main install: builds Rust (debug), installs UI/tray deps, sets up Python
# venv + MLX. --no-daemon skips all launchd registration so we can selectively
# register only screenpipe and a11y-helper below.
bash "${REPO_ROOT}/install.sh" --dev --no-daemon "$@"

# Write 'dev' into ~/.meridian/app/VERSION so the UI update-available banner
# never shows in dev mode (the version check returns false when current='dev').
mkdir -p "${HOME}/.meridian/app"
echo "dev" > "${HOME}/.meridian/app/VERSION"
echo "  ✓ ~/.meridian/app/VERSION set to 'dev' (suppresses update banner)"

# Register only the infrastructure agents that we don't actively develop.
# The Rust daemon and MLX server are intentionally excluded — dev-start.sh
# runs them with hot-reload instead.
echo ""
echo "→ Installing infrastructure launchd agents (screenpipe + a11y-helper)..."
bash "${REPO_ROOT}/scripts/install-screenpipe-daemon.sh"
bash "${REPO_ROOT}/scripts/install-a11y-helper-daemon.sh"
echo "  ✓ screenpipe + a11y-helper registered"

echo ""
echo "✓ Dev environment ready."
echo ""
echo "Start all services with hot-reload:"
echo "  bash dev-start.sh"
echo ""
echo "What dev-start.sh opens (4 Terminal windows):"
echo "  1. Rust daemon   — cargo watch, rebuilds on every .rs save"
echo "  2. MLX server    — uvicorn --reload, reloads on .py changes"
echo "  3. Next.js UI    — http://localhost:3939 (hot reload)"
echo "  4. Tauri tray    — hot reload"
echo ""
echo "screenpipe + a11y-helper run via launchd and restart automatically."
