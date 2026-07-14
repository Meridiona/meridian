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
#   3. signing + notarization — the .app AND the daemon nested inside it must both
#      carry a Developer ID signature under Meridiona's Team ID with the Hardened
#      Runtime, and the .app + DMG must be notarized AND stapled. Catches an
#      ad-hoc-signed release (Gatekeeper scare + TCC grants dropped on every
#      update, because the cdhash changes) and a missing notarization secret.
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

# ── 3. macOS code signing + notarization — the Gatekeeper/TCC guard ──────────
# Every failure below has shipped (or would ship) a broken release:
#   • ad-hoc signature      → Gatekeeper "unidentified developer" scare on first
#     launch, AND a fresh cdhash every build, so macOS treats each update as a
#     brand-new app and silently drops the user's Screen Recording / Accessibility
#     grants (the "Permissions not granted after update" bug).
#   • unsigned nested Mach-O → Apple's notary rejects the whole submission. Note
#     `codesign --verify --deep --strict` PASSES on an ad-hoc nested binary, so it
#     canNOT be relied on here — the Team ID must be asserted explicitly.
#   • un-notarized/un-stapled → first launch is blocked or needs an online check.
APP="target/release/bundle/macos/Meridian.app"
DMG="target/release/bundle/dmg/Meridian.dmg"
DAEMON_IN_APP="${APP}/Contents/Resources/backend/meridian"
TEAM_ID="AQTYN9PZ83"   # Meridiona LLP

[[ -d "${APP}" ]] || fail "${APP} not found — the tauri build did not produce an .app"

# 3a. Both Mach-Os must carry a Developer ID signature under Meridiona's Team ID
#     with the Hardened Runtime flag set (notarization requires all three).
for _target in "${APP}:app bundle" "${DAEMON_IN_APP}:bundled daemon"; do
    _path="${_target%%:*}"; _what="${_target##*:}"
    [[ -e "${_path}" ]] || fail "${_what} missing at ${_path}"
    _info="$(codesign -dv --verbose=4 "${_path}" 2>&1)"
    grep -q "Authority=Developer ID Application: Meridiona LLP (${TEAM_ID})" <<<"${_info}" \
        || fail "${_what} is not signed with the Developer ID cert (found: $(grep -m1 '^Signature' <<<"${_info}" || echo 'no signature')). Are APPLE_CERTIFICATE / APPLE_SIGNING_IDENTITY set in CI?"
    grep -q "TeamIdentifier=${TEAM_ID}" <<<"${_info}" || fail "${_what} has no/wrong TeamIdentifier (want ${TEAM_ID})"
    grep -qE 'flags=.*runtime' <<<"${_info}" || fail "${_what} is not signed with the Hardened Runtime (--options runtime) — notarization will reject it"
done
pass "app + bundled daemon: Developer ID (${TEAM_ID}), Hardened Runtime"

# 3b. The bundle seal is intact and every nested piece verifies.
codesign --verify --deep --strict "${APP}" 2>/dev/null || fail "codesign --verify --deep --strict failed on ${APP} — the bundle seal is broken"
pass "bundle seal verifies (--deep --strict)"

# 3c. Notarized + stapled: Gatekeeper must accept the app OFFLINE. `spctl`
#     reports "Notarized Developer ID" only when a stapled ticket is present.
_spctl="$(spctl --assess --type exec -vv "${APP}" 2>&1 || true)"
grep -q "accepted" <<<"${_spctl}" || fail "Gatekeeper REJECTED ${APP}: ${_spctl}"
grep -q "Notarized Developer ID" <<<"${_spctl}" || fail "${APP} is signed but NOT notarized (${_spctl}) — users get a Gatekeeper warning. Check the APPLE_API_* notarization secrets."
xcrun stapler validate "${APP}" >/dev/null 2>&1 || fail "no stapled notarization ticket on ${APP} — first launch would need an online Gatekeeper check"
pass "notarized + stapled (Gatekeeper accepts offline)"

# 3d. The DMG users actually download must itself be stapled.
if [[ -f "${DMG}" ]]; then
    xcrun stapler validate "${DMG}" >/dev/null 2>&1 || fail "no stapled notarization ticket on ${DMG} — the downloaded DMG would trip Gatekeeper"
    pass "DMG stapled: $(basename "${DMG}")"
else
    fail "${DMG} not found — package-updater.sh should have made the stable-named copy"
fi

# ── 4. updater artifacts — the silent-no-auto-update guard ───────────────────
# `tauri build` logs "failed to decode secret key: incorrect updater private key
# password" but STILL EXITS 0; package-updater.sh then finds no .sig and skips
# latest.json with a friendly message. Net effect: a green release that ships
# with auto-update dead. Assert the artifacts exist rather than trusting exit
# codes. (A wrong/absent TAURI_SIGNING_PRIVATE_KEY_PASSWORD is the usual cause.)
for _art in \
    "target/release/bundle/macos/Meridian.app.tar.gz:updater payload" \
    "target/release/bundle/macos/Meridian.app.tar.gz.sig:updater signature" \
    "target/release/bundle/macos/latest.json:updater manifest"; do
    _p="${_art%%:*}"; _w="${_art##*:}"
    [[ -s "${_p}" ]] || fail "${_w} missing/empty at ${_p} — auto-update would be dead for every installed user. Check TAURI_SIGNING_PRIVATE_KEY / _PASSWORD (tauri build logs the failure but exits 0)."
done
pass "updater artifacts present: payload + minisign signature + latest.json"

echo "✓ Smoke test passed — safe to publish v${VERSION}"
