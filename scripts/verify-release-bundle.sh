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
#   MERIDIAN_TARGET=aarch64-apple-darwin scripts/verify-release-bundle.sh <version>
#
# MERIDIAN_TARGET points the smoke test at a specific target triple's bundle
# tree, and defaults to universal-apple-darwin so existing callers are unaffected.
# It also selects WHAT is expected of the binaries and the manifest: a universal
# build must be fat and cover both platform keys, a per-arch build must be thin
# for exactly its own arch and cover exactly its own key. Asserting "thin, and
# thin for the RIGHT arch" matters as much as the fat assertion did — a job that
# built the wrong slice would otherwise sail through and ship an Intel-only
# release to Apple Silicon users.
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
# `tauri build --target <triple>` bundles under target/<triple>/, not plain
# target/release/.
TARGET="${MERIDIAN_TARGET:-universal-apple-darwin}"
BUNDLE_DIR="target/${TARGET}/release/bundle"
APP="${BUNDLE_DIR}/macos/Meridian.app"
DAEMON_IN_APP="${APP}/Contents/Resources/backend/meridian"
TEAM_ID="AQTYN9PZ83"   # Meridiona LLP

# Per-target expectations. These MUST stay in lockstep with package-updater.sh's
# own case statement — it decides the stable DMG name and the platform keys, and
# this script's whole job is to prove it produced what the release needs. The
# EXPECTED_ARCHS list is what `lipo -archs` must report, exactly (no more, no
# less), and EXPECTED_PLATFORMS is what latest.json must cover.
case "${TARGET}" in
  universal-apple-darwin)
    EXPECTED_ARCHS=(arm64 x86_64); EXPECTED_PLATFORMS=(darwin-aarch64 darwin-x86_64)
    STABLE_DMG="Meridian.dmg"; UPDATER_ASSET="Meridian.app.tar.gz" ;;
  aarch64-apple-darwin)
    EXPECTED_ARCHS=(arm64);        EXPECTED_PLATFORMS=(darwin-aarch64)
    STABLE_DMG="Meridian-aarch64.dmg"; UPDATER_ASSET="Meridian-aarch64.app.tar.gz" ;;
  x86_64-apple-darwin)
    EXPECTED_ARCHS=(x86_64);       EXPECTED_PLATFORMS=(darwin-x86_64)
    STABLE_DMG="Meridian-x64.dmg"; UPDATER_ASSET="Meridian-x64.app.tar.gz" ;;
  *)
    fail "unsupported MERIDIAN_TARGET '${TARGET}'" ;;
esac
DMG="${BUNDLE_DIR}/dmg/${STABLE_DMG}"

# Same triple-keyed manifest name package-updater.sh writes — universal keeps
# latest.json, per-arch gets updater-<triple>.json. Note the DMG/tarball names
# above use tauri-bundler's `x64` spelling while the manifest uses the triple:
# that asymmetry is intentional (see package-updater.sh), and the whole point of
# duplicating the rule here rather than inferring it is that this script's job is
# to prove the file the publish step will look for actually exists.
if [[ "${TARGET}" == "universal-apple-darwin" ]]; then
  MANIFEST_NAME="latest.json"
else
  MANIFEST_NAME="updater-${TARGET}.json"
fi

[[ -d "${APP}" ]] || fail "${APP} not found — the tauri build did not produce an .app"

# 3z. Both the tray binary and the bundled daemon must carry EXACTLY the
#     architectures this target promises — no more, no less. Under the universal
#     target that catches a silent fallback to a single-arch build (the
#     --target flag getting dropped). Under a per-arch target it also catches the
#     inverse mistake: an unexpectedly fat binary means the daemon slice came
#     from a stale target/release/meridian left by a universal build rather than
#     from this run, so the app would ship a daemon nobody in this job compiled.
#     Sorted comparison because lipo's output order is not contractual.
_want_archs="$(printf '%s\n' "${EXPECTED_ARCHS[@]}" | sort | tr '\n' ' ')"
for _target in "${APP}/Contents/MacOS/meridian-tray:tray binary" "${DAEMON_IN_APP}:bundled daemon"; do
    _path="${_target%%:*}"; _what="${_target##*:}"
    _archs="$(lipo -archs "${_path}" 2>/dev/null || true)"
    _got="$(printf '%s\n' ${_archs} | sort | tr '\n' ' ')"
    [[ "${_got}" == "${_want_archs}" ]] \
        || fail "${_what} at ${_path} has architectures '${_archs}' — expected exactly '${EXPECTED_ARCHS[*]}' for target ${TARGET}"
done
pass "tray binary + bundled daemon: architectures are exactly ${EXPECTED_ARCHS[*]} (${TARGET})"

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

# 3c/3d. Notarization — STABLE ONLY.
#
# Gatekeeper enforces notarization only on a QUARANTINED app, and the quarantine
# flag is set by whatever downloaded it — a browser. The two channels differ in
# exactly that:
#
#   stable  — ships a DMG that new users download in a browser. Quarantined, so
#             an un-notarized build gives every new user "Apple cannot check it
#             for malicious software". Notarization is mandatory.
#   staging — delivered ONLY by tauri-plugin-updater, which unpacks the tarball
#             itself and sets no quarantine flag. The relaunched app is verified
#             on its Developer ID signature (checked above, unconditionally) and
#             the updater's minisign key. No notarization ticket is ever
#             consulted, so submitting one costs build time and buys nothing.
#
# The SIGNING assertions above run on every channel and are the ones that matter
# for staging: a broken signature breaks the updater's relaunch.
if [[ "${MERIDIAN_CHANNEL:-stable}" == "stable" ]]; then
    # Notarized + stapled: Gatekeeper must accept the app OFFLINE. `spctl`
    # reports "Notarized Developer ID" only when a stapled ticket is present.
    _spctl="$(spctl --assess --type exec -vv "${APP}" 2>&1 || true)"
    grep -q "accepted" <<<"${_spctl}" || fail "Gatekeeper REJECTED ${APP}: ${_spctl}"
    grep -q "Notarized Developer ID" <<<"${_spctl}" || fail "${APP} is signed but NOT notarized (${_spctl}) — users get a Gatekeeper warning. Check the APPLE_API_* notarization secrets."
    xcrun stapler validate "${APP}" >/dev/null 2>&1 || fail "no stapled notarization ticket on ${APP} — first launch would need an online Gatekeeper check"
    pass "notarized + stapled (Gatekeeper accepts offline)"

    # The DMG users actually download must itself be stapled.
    if [[ -f "${DMG}" ]]; then
        xcrun stapler validate "${DMG}" >/dev/null 2>&1 || fail "no stapled notarization ticket on ${DMG} — the downloaded DMG would trip Gatekeeper"
        pass "DMG stapled: $(basename "${DMG}")"
    else
        fail "${DMG} not found — package-updater.sh should have made the stable-named copy"
    fi
else
    echo "  ~ skipping notarization checks (channel=${MERIDIAN_CHANNEL}) — updater-only delivery sets no quarantine flag"
    # The DMG must still EXIST even unstapled: package-updater.sh produces the
    # stable-named copy the manifest and the download link both point at.
    [[ -f "${DMG}" ]] || fail "${DMG} not found — package-updater.sh should have made the stable-named copy"
    pass "DMG present (unstapled, staging): $(basename "${DMG}")"
fi

# ── 2. updater artifacts — the silent-no-auto-update guard ───────────────────
# `tauri build` logs "failed to decode secret key: incorrect updater private key
# password" but STILL EXITS 0; package-updater.sh then finds no .sig and skips
# latest.json with a friendly message. Net effect: a green release that ships
# with auto-update dead. Assert the artifacts exist rather than trusting exit
# codes. (A wrong/absent TAURI_SIGNING_PRIVATE_KEY_PASSWORD is the usual cause.)
# UPDATER_ASSET is the name the manifest's url actually points at — the bare
# Meridian.app.tar.gz for universal, the arch-suffixed copy package-updater.sh
# made for per-arch. Verify the file the manifest references, not just the one
# tauri emitted, or a broken suffixed copy would pass unnoticed.
for _art in \
    "${BUNDLE_DIR}/macos/${UPDATER_ASSET}:updater payload" \
    "${BUNDLE_DIR}/macos/${UPDATER_ASSET}.sig:updater signature" \
    "${BUNDLE_DIR}/macos/${MANIFEST_NAME}:updater manifest"; do
    _p="${_art%%:*}"; _w="${_art##*:}"
    [[ -s "${_p}" ]] || fail "${_w} missing/empty at ${_p} — auto-update would be dead for every installed user. Check TAURI_SIGNING_PRIVATE_KEY / _PASSWORD (tauri build logs the failure but exits 0)."
done
pass "updater artifacts present: payload + minisign signature + latest.json"

# latest.json must carry the platform keys this target is responsible for —
# package-updater.sh dropping one silently would leave that arch's installs
# never seeing an update. Under the universal target that is both keys (pointing
# at the same universal payload); under a per-arch target it is that arch's key
# alone, because this manifest is a fragment and the join job supplies the rest.
# We assert presence only, not exclusivity: a fragment that happened to carry an
# extra key is the join job's problem to reject, not a reason to fail a build
# whose own artifacts are sound.
for _plat in "${EXPECTED_PLATFORMS[@]}"; do
    python3 -c "import json,sys; sys.exit(0 if '${_plat}' in json.load(open('${BUNDLE_DIR}/macos/${MANIFEST_NAME}'))['platforms'] else 1)" \
        || fail "${MANIFEST_NAME} is missing the '${_plat}' platform key"
done
pass "${MANIFEST_NAME} covers ${EXPECTED_PLATFORMS[*]}"

echo "✓ Smoke test passed — safe to publish v${VERSION} (${TARGET})"
