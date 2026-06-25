#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Pre-publish smoke test for the release bundle. Runs in semantic-release's
# PREPARE phase (appended to prepareCmd in .releaserc.json) — i.e. AFTER
# package-release.sh has populated npm/meridian-darwin-arm64 + release-assets/,
# but BEFORE the git tag is created and BEFORE anything is published. A non-zero
# exit aborts the release with nothing published and no tag left behind.
#
# It independently re-checks the failure modes that have shipped broken releases:
#   1. npm package size  — catches the 413 Payload Too Large (a large binary
#      leaking into the package balloons it past the registry limit).
#   2. binaries present  — the daemon + tray binaries must be in the package. The
#      dashboard ships embedded in the tray binary (static export), so there's no
#      separate ui.tar.gz / Node runtime / better-sqlite3 addon to ABI-check.
#
#   scripts/verify-release-bundle.sh <version>
set -euo pipefail

VERSION="${1:?usage: verify-release-bundle.sh <version>}"
VERSION="${VERSION#v}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

DEST="npm/meridian-darwin-arm64"
# Packed-tarball ceiling. The healthy package is ~15 MB packed (daemon + tray);
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

# ── 2. binaries present — daemon + tray (the dashboard is embedded in the tray) ─
_files="$(printf '%s' "${_pack_json}" | python3 -c 'import json,sys; [print(f["path"]) for f in json.load(sys.stdin)[0]["files"]]')"
for _bin in bin/meridian bin/meridian-tray; do
    grep -qx "${_bin}" <<<"${_files}" || fail "${_bin} missing from the npm package"
done
pass "binaries present: daemon + tray (dashboard embedded in the tray binary)"

echo "✓ Smoke test passed — safe to publish v${VERSION}"
