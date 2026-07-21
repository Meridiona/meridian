#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Notarize + staple the DMG. Runs after `tauri build`, before package-updater.sh
# makes its stable-named `Meridian.dmg` copy (stapling rewrites the file, so the
# copy inherits the ticket).
#
# Why this exists: Tauri notarizes and staples the .app (tauri-bundler's
# macos/app.rs), but it only *signs* the DMG — there is no notarize/staple call
# for the disk image anywhere in the bundler. Apple's guidance is explicit that
# the container you actually distribute must itself be notarized: a quarantined,
# un-notarized DMG makes macOS tell the user it "can't check it for malicious
# software" when they open it — which is precisely the scare a paid product
# cannot ship, and precisely what the Developer ID was bought to avoid. The app
# inside being stapled does not cover the DMG: a notarization ticket is keyed to
# the cdhash of the thing submitted, and the DMG's own cdhash was never seen by
# Apple.
#
#   bash scripts/notarize-dmg.sh <version>
#   MERIDIAN_TARGET=aarch64-apple-darwin bash scripts/notarize-dmg.sh <version>
#
# MERIDIAN_TARGET selects which `target/<triple>/release/bundle` tree to look in.
# It defaults to universal-apple-darwin so every existing caller (semantic-release
# prepareCmd, release-staging.yml) keeps working with no change at all. The
# per-arch release path sets it to aarch64-apple-darwin / x86_64-apple-darwin,
# one per runner; each runner notarizes only its own DMG.
#
# NO-OP unless APPLE_SIGNING_IDENTITY names a real Developer ID cert, so local
# dev builds don't try (and fail) to reach Apple's notary service. When it IS a
# Developer ID build, missing notarization credentials are a HARD failure — a
# silently-unnotarized release is the exact bug this script exists to prevent.
set -euo pipefail

VERSION="${1:?usage: notarize-dmg.sh <version>}"
VERSION="${VERSION#v}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

# `tauri build --target <triple>` bundles under target/<triple>/, not plain
# target/release/. The DMG filename's arch suffix differs per target (universal
# builds don't carry the fixed "_aarch64" a single-arch build used, and a
# per-arch build's suffix is tauri-bundler's own arch spelling, not the triple)
# — so glob for it rather than trying to reconstruct the name.
TARGET="${MERIDIAN_TARGET:-universal-apple-darwin}"
DMG_DIR="target/${TARGET}/release/bundle/dmg"
DMG="$(ls "${DMG_DIR}"/Meridian_${VERSION}_*.dmg 2>/dev/null | head -1)"
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"

case "${IDENTITY}" in
    "Developer ID Application:"*) ;;
    *)
        echo "→ notarize-dmg: skipping (APPLE_SIGNING_IDENTITY is not a Developer ID cert)"
        exit 0
        ;;
esac

[[ -n "${DMG}" && -f "${DMG}" ]] || { echo "✗ notarize-dmg: no Meridian_${VERSION}_*.dmg found in ${DMG_DIR}" >&2; exit 1; }

: "${APPLE_API_KEY:?notarize-dmg: APPLE_API_KEY (the 10-char Key ID) is unset}"
: "${APPLE_API_ISSUER:?notarize-dmg: APPLE_API_ISSUER (the issuer UUID) is unset}"
: "${APPLE_API_KEY_PATH:?notarize-dmg: APPLE_API_KEY_PATH (path to the .p8) is unset}"
[[ -f "${APPLE_API_KEY_PATH}" ]] || { echo "✗ notarize-dmg: no .p8 at ${APPLE_API_KEY_PATH}" >&2; exit 1; }

# --wait blocks until Apple returns Accepted/Invalid (typically 1-5 min). Without
# it the submission is fire-and-forget and the staple below would race it.
echo "→ notarize-dmg: submitting $(basename "${DMG}") to Apple's notary service…"
xcrun notarytool submit "${DMG}" \
    --key "${APPLE_API_KEY_PATH}" \
    --key-id "${APPLE_API_KEY}" \
    --issuer "${APPLE_API_ISSUER}" \
    --wait

# Staple the ticket INTO the dmg so Gatekeeper accepts it with no network.
xcrun stapler staple "${DMG}"
xcrun stapler validate "${DMG}"

echo "✓ notarize-dmg: $(basename "${DMG}") notarized + stapled"
