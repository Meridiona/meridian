#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Cargo `runner` shim: sign the just-built binary with the stable "Meridian Dev"
# identity, then exec it. Wired in ONLY for the tray dev launcher via the scoped
# env var CARGO_TARGET_<host-triple>_RUNNER (see start-tray-dev.sh), so it never
# touches the daemon build or CI.
#
# Why this exists: `tauri dev` re-links + ad-hoc-signs ("-") the tray binary on
# every rebuild. macOS TCC anchors the Screen Recording grant to the signing
# identity, so an ad-hoc identity (no stable cdhash) invalidates the grant on
# every rebuild -> capture defers (and, on macOS 26, the in-process capture used
# to abort). Signing each rebuild with the SAME self-signed cert keeps one stable
# identity, so the grant granted ONCE survives every rebuild. Hot reload is
# preserved because cargo still drives the build/run loop.
#
# Cargo invokes a runner as: <runner> <path-to-binary> [args...]. We must exec
# the binary through so signals/exit codes pass straight through to tauri dev.
set -euo pipefail

BIN="${1:?dev-sign-run: no binary path passed by cargo}"
shift

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IDENT="$(bash "${SCRIPT_DIR}/dev-signing.sh" identity)"

if [[ "${IDENT}" == "-" ]]; then
  # No stable cert on this machine (e.g. CI) — run as-is; the grant just won't
  # persist across rebuilds, same as plain `tauri dev`.
  echo "→ dev-sign-run: no 'Meridian Dev' cert, running ad-hoc" >&2
else
  # --force replaces the ad-hoc signature cargo/the linker just applied.
  # --timestamp=none: self-signed cert, no trusted timestamp authority.
  codesign --force --sign "${IDENT}" --timestamp=none "${BIN}" >/dev/null 2>&1 \
    && echo "→ dev-sign-run: signed with '${IDENT}'" >&2 \
    || echo "→ dev-sign-run: codesign failed (running anyway)" >&2
fi

exec "${BIN}" "$@"
