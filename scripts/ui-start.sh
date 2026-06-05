#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Startup wrapper for the Next.js UI daemon.
#
# Checks that better-sqlite3's native addon matches the current Node.js ABI
# before starting the server. If the ABI doesn't match (e.g. CI built with
# Node 24 but the user has Node 22 or 23), downloads the correct pre-built
# binary from GitHub via prebuild-install, then execs the server.
#
# Runs at every UI daemon start — check is fast (<200ms) when the binary is
# already correct (just a require() probe). The slow path (npm install) only
# executes on mismatch, which should only happen once.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_ROOT="$(dirname "${SCRIPT_DIR}")"
SQLITE3_DIR="${APP_ROOT}/ui/node_modules/better-sqlite3"

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

# npm lives alongside node in the same bin directory.
NPM="${NODE%node}npm"
[[ -x "${NPM}" ]] || NPM="$(command -v npm 2>/dev/null || true)"

if [[ -d "${SQLITE3_DIR}" ]] && ! "${NODE}" -e "require('${SQLITE3_DIR}')" 2>/dev/null; then
    echo "[meridian-ui] better-sqlite3 ABI mismatch for Node $("${NODE}" -v) — fetching correct binary…" >&2
    _bsv="$("${NODE}" -e "process.stdout.write(require('${SQLITE3_DIR}/package.json').version)" 2>/dev/null || true)"
    _bsok=0
    if [[ -n "${_bsv}" ]] && [[ -x "${NPM:-}" ]]; then
        _bstmp="$(mktemp -d)"
        if (cd "${_bstmp}" && "${NPM}" install --no-save "better-sqlite3@${_bsv}" >/dev/null 2>&1) && \
           [[ -f "${_bstmp}/node_modules/better-sqlite3/build/Release/better_sqlite3.node" ]]; then
            cp "${_bstmp}/node_modules/better-sqlite3/build/Release/better_sqlite3.node" \
               "${SQLITE3_DIR}/build/Release/better_sqlite3.node"
            _bsok=1
        fi
        rm -rf "${_bstmp}"
    fi
    if [[ "${_bsok}" -eq 1 ]]; then
        echo "[meridian-ui] better-sqlite3 fixed for Node $("${NODE}" -v)" >&2
    else
        echo "[meridian-ui] WARNING: could not fix better-sqlite3 — dashboard will show empty data" >&2
        echo "[meridian-ui]   fix: meridian update  (or: npm install -g @meridiona/meridian)" >&2
    fi
fi

exec "${NODE}" "${APP_ROOT}/ui/server.js"
