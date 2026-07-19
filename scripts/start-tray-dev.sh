#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Start tray app in dev mode (with hot reload)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

info()  { echo "→ $*" >&2; }
ok()    { echo "  ✓ $*" >&2; }

info "Starting tray app in dev mode..."
info "When you're done, stop it with Ctrl+C"
echo

# Keep the macOS Screen Recording grant alive across rebuilds. `tauri dev`
# ad-hoc-signs the tray binary on every rebuild, which invalidates the TCC grant
# (macOS anchors it to the signing identity) and, on macOS 26, made the capture
# stack abort. We give cargo a `runner` that re-signs each freshly-built binary
# with the stable self-signed "Meridian Dev" identity before launching it, so a
# grant given ONCE persists. Scoped to this process via CARGO_TARGET_<triple>_RUNNER
# so the daemon build and CI are untouched. Grant once (System Settings > Privacy
# & Security > Screen Recording) after the first run and it sticks.
if [[ "$(uname -s)" == "Darwin" ]]; then
  bash "${REPO_ROOT}/scripts/dev-signing.sh" setup || true
  HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
  RUNNER_VAR="CARGO_TARGET_$(echo "${HOST_TRIPLE}" | tr 'a-z-' 'A-Z_')_RUNNER"
  export "${RUNNER_VAR}=${REPO_ROOT}/scripts/dev-sign-run.sh"
  ok "signing each rebuild with 'Meridian Dev' (${RUNNER_VAR})"
fi

cd "${REPO_ROOT}/tray"
npm run tauri dev
