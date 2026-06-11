#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Pre-publish smoke test for the release bundle. Runs in semantic-release's
# PREPARE phase (appended to prepareCmd in .releaserc.json) — i.e. AFTER
# package-release.sh has populated npm/meridian-darwin-arm64 + release-assets/,
# but BEFORE the git tag is created and BEFORE anything is published. A non-zero
# exit aborts the release with nothing published and no tag left behind.
#
# It independently re-checks the exact failure modes that have shipped broken
# releases to production:
#   1. npm package size  — catches the 413 Payload Too Large (a re-bundled Node
#      runtime balloons the package past the registry limit).
#   2. better-sqlite3 ABI — extracts ui.tar.gz, downloads the pinned Node from
#      nodejs.org, and require()s the actual .node binary by absolute path. This
#      is INDEPENDENT of the check inside package-release.sh, so a bug in that
#      check (e.g. require(package_dir) never loading the addon) is still caught.
#   3. payload hygiene — the npm package must NOT contain the 113 MB node binary;
#      it MUST carry the tiny bin/node-runtime.meta the installer needs.
#
#   scripts/verify-release-bundle.sh <version>
set -euo pipefail

VERSION="${1:?usage: verify-release-bundle.sh <version>}"
VERSION="${VERSION#v}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

DEST="npm/meridian-darwin-arm64"
# Packed-tarball ceiling. The healthy package is ~25 MB packed (daemon + ui.tar.gz);
# the registry rejects ~200 MB. 75 MB sits far above normal yet trips instantly if
# the Node runtime (113 MB) ever leaks back into the package.
MAX_PACKED_MB=75

pass() { echo "  ✓ $*"; }
fail() { echo "✗ SMOKE TEST FAILED: $*" >&2; exit 1; }

echo "→ Pre-publish smoke test (v${VERSION})"

# ── 1. npm package size — the 413 guard ──────────────────────────────────────
_pack_json="$(cd "${DEST}" && npm pack --dry-run --json 2>/dev/null)"
_packed_bytes="$(printf '%s' "${_pack_json}" | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["size"])')"
_packed_mb=$(( _packed_bytes / 1048576 ))
if (( _packed_mb > MAX_PACKED_MB )); then
    fail "npm package is ${_packed_mb} MB packed (> ${MAX_PACKED_MB} MB ceiling) — it would 413 on publish. A large binary likely leaked into the package; large blobs belong on the GitHub Release."
fi
pass "npm package ${_packed_mb} MB packed (≤ ${MAX_PACKED_MB} MB) — under the registry limit"

# ── 2. payload hygiene — large blobs must be OFF the package ──────────────────
_files="$(printf '%s' "${_pack_json}" | python3 -c 'import json,sys; [print(f["path"]) for f in json.load(sys.stdin)[0]["files"]]')"
grep -qx 'bin/node-runtime' <<<"${_files}" && fail "bin/node-runtime (113 MB binary) is in the npm package — it must be downloaded at install, not bundled"
grep -qx 'bin/node-runtime.meta' <<<"${_files}" || fail "bin/node-runtime.meta is missing from the npm package — the installer needs it to resolve the Node runtime"
pass "payload hygiene: node-runtime.meta present; no bundled Node binary"

# ── 3. better-sqlite3 ABI — the independent dlopen check ──────────────────────
# Only when ui.tar.gz shipped this release (UI unchanged → no new addon to test;
# the installed dashboard keeps running on its existing ABI-matched runtime).
if [[ -f "${DEST}/ui.tar.gz" ]]; then
    _meta="${DEST}/bin/node-runtime.meta"
    [[ -f "${_meta}" ]] || fail "bin/node-runtime.meta missing — cannot resolve the Node runtime to ABI-test the addon"
    _nver="$(grep '^NODE_RUNTIME_VERSION=' "${_meta}" | cut -d= -f2 | tr -d '[:space:]')"
    _nsha="$(grep '^NODE_RUNTIME_SHA=' "${_meta}" | cut -d= -f2 | tr -d '[:space:]')"
    _tmp="$(mktemp -d)"
    # Download the exact pinned Node the installer will use, verify its SHA.
    curl -fsSL --retry 3 "https://nodejs.org/dist/v${_nver}/node-v${_nver}-darwin-arm64.tar.gz" -o "${_tmp}/node.tgz" \
        || fail "could not download Node ${_nver} from nodejs.org to run the ABI check"
    _ngot="$(shasum -a 256 "${_tmp}/node.tgz" | cut -d' ' -f1)"
    [[ "${_ngot}" == "${_nsha}" ]] || fail "Node ${_nver} SHA-256 in node-runtime.meta does not match nodejs.org (meta ${_nsha} vs ${_ngot})"
    tar -xzf "${_tmp}/node.tgz" -C "${_tmp}"
    _node="${_tmp}/node-v${_nver}-darwin-arm64/bin/node"
    # Extract the shipped dashboard and locate its better-sqlite3 addon.
    mkdir -p "${_tmp}/ui"
    tar -xzf "${DEST}/ui.tar.gz" -C "${_tmp}/ui"
    _addon="$(find "${_tmp}/ui" -name 'better_sqlite3.node' -path '*/Release/*' 2>/dev/null | head -1)"
    [[ -n "${_addon}" ]] || fail "no better_sqlite3.node found in ui.tar.gz"
    # The real test: the pinned Node must dlopen the shipped addon. require() of
    # the .node file by ABSOLUTE path forces dlopen (NODE_MODULE_VERSION check);
    # require(package_dir) would NOT — better-sqlite3 lazy-loads inside Database().
    if "${_node}" -e "require('${_addon}')" 2>/dev/null; then
        pass "better-sqlite3 ABI: Node ${_nver} loads the shipped addon (dashboard will start)"
    else
        rm -rf "${_tmp}"
        fail "Node ${_nver} cannot load the shipped better_sqlite3.node — ABI mismatch; the dashboard would crash-loop on every user's machine"
    fi
    rm -rf "${_tmp}"
else
    pass "UI unchanged this release (no ui.tar.gz) — better-sqlite3 ABI check not applicable"
fi

echo "✓ Smoke test passed — safe to publish v${VERSION}"
