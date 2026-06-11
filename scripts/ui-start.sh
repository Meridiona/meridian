#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Startup wrapper for the Next.js UI daemon.
#
# The dashboard's better-sqlite3 native addon is ABI-locked to ONE exact Node
# version — the one CI built it against, recorded in bin/node-runtime.meta as
# NODE_RUNTIME_VERSION. Running any other Node major triggers a
# NODE_MODULE_VERSION (ABI) mismatch and the dashboard crash-loops. So on a
# bundle install we resolve the cached, version-matched runtime under
# ~/.meridian/node-runtime ourselves — we do NOT trust a stale MERIDIAN_NODE_BIN
# or system node that a `meridian update` may have left behind (that is exactly
# how the UI ended up on a mismatched Node). If the matched runtime is missing we
# fail LOUD with remediation rather than silently crash-looping under the wrong
# Node. (A source/dev install has no meta file: there better-sqlite3 was compiled
# against the local node, so we prefer MERIDIAN_NODE_BIN then system node.)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_ROOT="$(dirname "${SCRIPT_DIR}")"
META="${APP_ROOT}/bin/node-runtime.meta"

log() { echo "[meridian-ui] $*" >&2; }

# `node -v` → bare version (strip the leading "v"); empty if it can't run.
node_version() { "${1}" -v 2>/dev/null | sed 's/^v//'; }

# The exact Node version the shipped addon was built against (bundle install
# only). `|| true` so a malformed/lineless meta doesn't trip `set -e`.
REQUIRED_VER=""
if [[ -f "${META}" ]]; then
    REQUIRED_VER="$(grep '^NODE_RUNTIME_VERSION=' "${META}" 2>/dev/null | cut -d= -f2 | tr -d '[:space:]' || true)"
fi

NODE=""
if [[ -n "${REQUIRED_VER}" ]]; then
    # Bundle install: enforce the ABI-matched runtime. The cached runtime that
    # install-from-bundle.sh downloaded for this exact version is the source of
    # truth; accept MERIDIAN_NODE_BIN only if it actually IS that version.
    cached="${HOME}/.meridian/node-runtime/v${REQUIRED_VER}/bin/node"
    if [[ -x "${cached}" ]] && [[ "$(node_version "${cached}")" == "${REQUIRED_VER}" ]]; then
        NODE="${cached}"
    elif [[ -n "${MERIDIAN_NODE_BIN:-}" ]] && [[ -x "${MERIDIAN_NODE_BIN}" ]] \
         && [[ "$(node_version "${MERIDIAN_NODE_BIN}")" == "${REQUIRED_VER}" ]]; then
        NODE="${MERIDIAN_NODE_BIN}"
    else
        log "dashboard requires Node ${REQUIRED_VER} (better-sqlite3 ABI), but the"
        log "cached runtime at ${cached} is missing or wrong-versioned."
        log "Fetch it: run 'meridian update' with a connection, or reinstall:"
        log "  curl -fsSL https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.sh | bash"
        # EX_CONFIG: refuse to start under a mismatched Node. KeepAlive will retry,
        # but the log now states the exact cause instead of a cryptic ERR_DLOPEN.
        exit 78
    fi
else
    # Source/dev install (no meta): better-sqlite3 was compiled against the local
    # node, so prefer MERIDIAN_NODE_BIN then system node — ABI matches by build.
    # launchd agents don't inherit the user's PATH, so probe known locations.
    NODE="${MERIDIAN_NODE_BIN:-}"
    if [[ -z "${NODE}" ]] || [[ ! -x "${NODE}" ]]; then
        for _n in /opt/homebrew/bin/node /usr/local/bin/node /usr/bin/node; do
            if [[ -x "${_n}" ]]; then NODE="${_n}"; break; fi
        done
    fi
    [[ -x "${NODE:-}" ]] || { log "node not found — cannot start UI server"; exit 1; }
fi

exec "${NODE}" "${APP_ROOT}/ui/server.js"
