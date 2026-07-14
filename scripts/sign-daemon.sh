#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Developer-ID-sign the `meridian` daemon binary BEFORE `tauri build` copies it
# into Meridian.app as a bundle resource (bundle.resources → backend/meridian).
#
# Why this exists: Tauri signs the .app and its own tray binary, but NOT the
# extra Mach-O we inject via `bundle.resources`. Left alone, the daemon ships
# inside the bundle ad-hoc/linker-signed with no Team ID — and Apple's notary
# service rejects a submission where ANY nested Mach-O lacks a Developer ID
# signature + Hardened Runtime. Note that `codesign --verify --deep --strict`
# still PASSES in that state (an ad-hoc signature is a valid signature), so this
# only surfaces at notarization; hence the explicit step rather than trusting a
# deep-verify to catch it.
#
# Signing must happen INSIDE-OUT: the daemon is sealed into the app's resource
# hashes, so it has to carry its final signature before Tauri seals the outer
# bundle. Re-signing it afterwards would invalidate the app's seal.
#
#   bash scripts/sign-daemon.sh            # signs target/release/meridian
#
# NO-OP unless APPLE_SIGNING_IDENTITY names a real Developer ID cert. Local dev
# builds (ad-hoc "-" or the self-signed "Meridian Dev" identity from
# scripts/dev-signing.sh) are left untouched: they are never notarized, and the
# dev identity deliberately keeps its own stable cdhash for TCC (see
# dev-signing.sh). Called from tray/package.json's `build` / `build:staging`,
# between `build:daemon` and `tauri build`.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${REPO_ROOT}/target/release/meridian"
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"

# Only a real Developer ID cert is worth signing with here — see the header.
case "${IDENTITY}" in
    "Developer ID Application:"*) ;;
    *)
        echo "→ sign-daemon: skipping (APPLE_SIGNING_IDENTITY is not a Developer ID cert)"
        exit 0
        ;;
esac

[[ -f "${BIN}" ]] || { echo "✗ sign-daemon: ${BIN} not found — run the daemon build first" >&2; exit 1; }

# --options runtime  : Hardened Runtime, mandatory for notarization.
# --timestamp        : secure timestamp from Apple's TSA, also mandatory (needs
#                      network; a signature without one is rejected by notary).
# --force            : replace the ad-hoc signature cargo/ld left behind.
echo "→ sign-daemon: signing target/release/meridian as '${IDENTITY}'"
codesign --force --options runtime --timestamp --sign "${IDENTITY}" "${BIN}"
codesign --verify --strict --verbose=2 "${BIN}"

echo "✓ sign-daemon: daemon signed + verified (Developer ID, Hardened Runtime)"
