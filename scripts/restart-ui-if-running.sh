#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Restart the launchd UI dashboard (com.meridiona.ui) onto a freshly built
# ui/.next bundle — but ONLY when it is currently loaded AND enabled. Wired in
# as the `postbuild` npm hook in ui/package.json so every `next build` against
# a live install picks up the new bundle.
#
# Why this is needed: launchd KeepAlive keeps the OLD `next start` process alive
# across an in-place rebuild; it does NOT reload it. The stale process keeps
# serving HTML that references the previous build's asset hashes, which the
# rebuild has since overwritten — so the browser's CSS/JS requests 500 and the
# page renders blank ("This page couldn't load").
#
# No-ops (exit 0, never fails the build) when:
#   * MERIDIAN_BUILD_STANDALONE is set — this is a release-bundle build, not a
#     local dev rebuild; never touch a developer's running service.
#   * launchctl is unavailable (non-macOS / CI runner without launchd).
#   * the agent is not loaded (fresh checkout, CI).
#   * the agent is disabled — a `meridian dev` session boots it out and disables
#     it to free port 3939 for `next dev`; do not resurrect it.

set -euo pipefail

LABEL="com.meridiona.ui"
GUI_TARGET="gui/$(id -u)"

# Release-bundle build (see .releaserc.json) — leave any running dev service
# alone; the standalone server is run by install-from-bundle.sh, not this hook.
[[ -n "${MERIDIAN_BUILD_STANDALONE:-}" ]] && exit 0

command -v launchctl >/dev/null 2>&1 || exit 0

# Loaded? `launchctl print` fails when the agent is not bootstrapped.
launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1 || exit 0

# Enabled? `meridian dev` disables the agent to hold port 3939 for `next dev`.
# Match the label line in print-disabled regardless of the macOS-version spelling
# (`=> disabled` on recent releases, `=> true` on older ones).
if launchctl print-disabled "${GUI_TARGET}" 2>/dev/null \
     | grep -Eq "\"${LABEL}\"[[:space:]]*=>[[:space:]]*(disabled|true)"; then
    exit 0
fi

echo "→ restarting ${LABEL} onto the freshly built UI bundle"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
