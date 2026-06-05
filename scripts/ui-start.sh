#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Startup wrapper for the Next.js UI daemon.
#
# MERIDIAN_NODE_BIN is set by the launchd plist to the bundled Node 22 runtime
# at APP_ROOT/bin/node-runtime. That binary was used in CI to build the
# better-sqlite3 addon shipped in ui.tar.gz, so ABI is always correct.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_ROOT="$(dirname "${SCRIPT_DIR}")"

# Resolve node: prefer the path baked in at install time (MERIDIAN_NODE_BIN),
# fall back to well-known Homebrew / system locations. launchd agents don't
# inherit the user's PATH, so we can't rely on `command -v node`.
NODE="${MERIDIAN_NODE_BIN:-}"
if [[ -z "${NODE}" ]] || [[ ! -x "${NODE}" ]]; then
    for _n in /opt/homebrew/bin/node /usr/local/bin/node /usr/bin/node; do
        if [[ -x "${_n}" ]]; then NODE="${_n}"; break; fi
    done
fi
[[ -x "${NODE:-}" ]] || { echo "[meridian-ui] node not found — cannot start UI server" >&2; exit 1; }

exec "${NODE}" "${APP_ROOT}/ui/server.js"
