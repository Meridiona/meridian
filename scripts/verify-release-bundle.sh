#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Pre-publish smoke test for the release bundle. Runs in semantic-release's
# PREPARE phase (appended to prepareCmd in .releaserc.json) — i.e. AFTER the
# `tauri build` has produced the .app/DMG, but BEFORE the git tag is created and
# BEFORE anything is published. A non-zero exit aborts the release with nothing
# published and no tag left behind.
#
# It independently re-checks the failure modes that have shipped broken releases:
#   1. signing + notarization — the .app AND the daemon nested inside it must both
#      carry a Developer ID signature under Meridiona's Team ID with the Hardened
#      Runtime, and the .app + DMG must be notarized AND stapled. Catches an
#      ad-hoc-signed release (Gatekeeper scare + TCC grants dropped on every
#      update, because the cdhash changes) and a missing notarization secret.
#   2. updater artifacts — payload + signature + latest.json must all exist, or
#      auto-update ships dead (tauri build logs the failure but exits 0).
#
#   scripts/verify-release-bundle.sh <version>
set -euo pipefail

VERSION="${1:?usage: verify-release-bundle.sh <version>}"
VERSION="${VERSION#v}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

pass() { echo "  ✓ $*"; }
fail() { echo "✗ SMOKE TEST FAILED: $*" >&2; exit 1; }

echo "→ Pre-publish smoke test (v${VERSION})"

# ── 1. macOS code signing + notarization — the Gatekeeper/TCC guard ──────────
# Every failure below has shipped (or would ship) a broken release:
#   • ad-hoc signature      → Gatekeeper "unidentified developer" scare on first
#     launch, AND a fresh cdhash every build, so macOS treats each update as a
#     brand-new app and silently drops the user's Screen Recording / Accessibility
#     grants (the "Permissions not granted after update" bug).
#   • unsigned nested Mach-O → Apple's notary rejects the whole submission. Note
#     `codesign --verify --deep --strict` PASSES on an ad-hoc nested binary, so it
#     canNOT be relied on here — the Team ID must be asserted explicitly.
#   • un-notarized/un-stapled → first launch is blocked or needs an online check.
# `--target universal-apple-darwin` bundles under target/universal-apple-darwin/,
# not plain target/release/.
BUNDLE_DIR="target/universal-apple-darwin/release/bundle"
APP="${BUNDLE_DIR}/macos/Meridian.app"
DMG="${BUNDLE_DIR}/dmg/Meridian.dmg"
DAEMON_IN_APP="${APP}/Contents/Resources/backend/meridian"
TEAM_ID="AQTYN9PZ83"   # Meridiona LLP

[[ -d "${APP}" ]] || fail "${APP} not found — the tauri build did not produce an .app"

# 3z. Both the tray binary and the bundled daemon must actually carry both
#     architectures — catches a silent fallback to a single-arch build (e.g.
#     the universal-apple-darwin target flag getting dropped) before it ships.
for _target in "${APP}/Contents/MacOS/meridian-tray:tray binary" "${DAEMON_IN_APP}:bundled daemon"; do
    _path="${_target%%:*}"; _what="${_target##*:}"
    _archs="$(lipo -archs "${_path}" 2>/dev/null || true)"
    [[ "${_archs}" == *arm64* && "${_archs}" == *x86_64* ]] \
        || fail "${_what} at ${_path} is not universal (lipo -archs: '${_archs}') — expected both arm64 and x86_64"
done
pass "tray binary + bundled daemon: universal (arm64 + x86_64)"

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

# ── 2. updater artifacts — the silent-no-auto-update guard ───────────────────
# `tauri build` logs "failed to decode secret key: incorrect updater private key
# password" but STILL EXITS 0; package-updater.sh then finds no .sig and skips
# latest.json with a friendly message. Net effect: a green release that ships
# with auto-update dead. Assert the artifacts exist rather than trusting exit
# codes. (A wrong/absent TAURI_SIGNING_PRIVATE_KEY_PASSWORD is the usual cause.)
for _art in \
    "${BUNDLE_DIR}/macos/Meridian.app.tar.gz:updater payload" \
    "${BUNDLE_DIR}/macos/Meridian.app.tar.gz.sig:updater signature" \
    "${BUNDLE_DIR}/macos/latest.json:updater manifest"; do
    _p="${_art%%:*}"; _w="${_art##*:}"
    [[ -s "${_p}" ]] || fail "${_w} missing/empty at ${_p} — auto-update would be dead for every installed user. Check TAURI_SIGNING_PRIVATE_KEY / _PASSWORD (tauri build logs the failure but exits 0)."
done
pass "updater artifacts present: payload + minisign signature + latest.json"

# latest.json must carry BOTH platform keys (pointing at the same universal
# payload) — package-updater.sh dropping one silently would leave that arch's
# installs never seeing an update.
for _plat in darwin-aarch64 darwin-x86_64; do
    python3 -c "import json,sys; sys.exit(0 if '${_plat}' in json.load(open('${BUNDLE_DIR}/macos/latest.json'))['platforms'] else 1)" \
        || fail "latest.json is missing the '${_plat}' platform key"
done
pass "latest.json covers both darwin-aarch64 and darwin-x86_64"

echo "✓ Smoke test passed — safe to publish v${VERSION}"
