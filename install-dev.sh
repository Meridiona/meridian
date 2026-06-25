#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Dev-mode install: build deps and register the OpenObserve observability agent.
# The Rust daemon, MLX server, and Tauri tray run in watch mode via dev-start.sh.
# Capture runs in-process inside the Tauri tray — no screenpipe/a11y-helper needed.
#
# If you have those agents from a previous dev setup, remove them:
#   launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.screenpipe.plist
#   launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.meridiona.a11y-helper.plist

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Build Rust (debug), install UI/tray npm deps, set up Python venv + MLX.
# --no-daemon skips all launchd registration (capture is in-process in the tray).
bash "${REPO_ROOT}/install.sh" --dev --no-daemon "$@"

# Suppress the update-available banner in dev mode.
mkdir -p "${HOME}/.meridian/app"
echo "dev" > "${HOME}/.meridian/app/VERSION"
echo "  ✓ ~/.meridian/app/VERSION set to 'dev' (suppresses update banner)"

# OpenObserve — local OTLP backend for traces + logs. Optional but recommended
# if you're iterating on the pipeline (query at http://localhost:5080).
if command -v openobserve >/dev/null 2>&1 || [[ -x "${HOME}/.openobserve/openobserve" ]]; then
    echo "→ Installing OpenObserve launchd agent..."
    if bash "${REPO_ROOT}/scripts/install-openobserve-daemon.sh"; then
        echo "  ✓ OpenObserve registered"
    else
        echo "  ⚠ OpenObserve agent install skipped (set MERIDIAN_OO_AUTH in <repo>/.env to enable)"
    fi
else
    echo "  ⚠ OpenObserve not installed — skipping (optional, install from https://openobserve.ai)"
fi

echo ""
echo "✓ Dev environment ready. Start all services with:"
echo "  bash dev-start.sh"
