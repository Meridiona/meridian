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
#   bash scripts/sign-daemon.sh                          # target/release/meridian
#   MERIDIAN_DAEMON_BIN=path/to/meridian bash scripts/sign-daemon.sh
#   bash scripts/sign-daemon.sh path/to/meridian
#
# The path is overridable but its DEFAULT is deliberately not derived from
# MERIDIAN_TARGET like the other release scripts. tauri.conf.json's
# `bundle.resources` names the literal path `target/release/meridian`, so
# whatever ends up there is what gets bundled — a per-arch build must still
# place its thin daemon at that exact path before `tauri build` runs (see
# build-daemon-universal.sh's note). Keying this script off the triple instead
# would sign target/<triple>/release/meridian and leave the binary that actually
# ships unsigned, which notarization only discovers at the very end of a release.
#
# NO-OP unless APPLE_SIGNING_IDENTITY names a real Developer ID cert. Local dev
# builds (ad-hoc "-" or the self-signed "Meridian Dev" identity from
# scripts/dev-signing.sh) are left untouched: they are never notarized, and the
# dev identity deliberately keeps its own stable cdhash for TCC (see
# dev-signing.sh). Called from tray/package.json's `build` / `build:staging`,
# between `build:daemon` and `tauri build`.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Positional arg wins over the env var wins over the default, so a caller can
# override without exporting. A relative override resolves against the repo root
# (not the caller's cwd) to match how every other path in these scripts behaves.
DAEMON_BIN="${1:-${MERIDIAN_DAEMON_BIN:-target/release/meridian}}"
case "${DAEMON_BIN}" in
    /*) BIN="${DAEMON_BIN}" ;;
    *)  BIN="${REPO_ROOT}/${DAEMON_BIN}" ;;
esac
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
echo "→ sign-daemon: signing ${DAEMON_BIN} as '${IDENTITY}'"
codesign --force --options runtime --timestamp --sign "${IDENTITY}" "${BIN}"
codesign --verify --strict --verbose=2 "${BIN}"

echo "✓ sign-daemon: daemon signed + verified (Developer ID, Hardened Runtime)"
