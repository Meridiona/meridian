#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Mirror the freshly-built staging updater manifest onto a FIXED, rolling
# `updater-staging` GitHub prerelease, so an installed staging .app — which
# bakes the endpoint …/releases/download/updater-staging/latest.json (see
# tray/src-tauri/tauri.staging.conf.json) — always finds the newest staging
# build at one stable URL. Mirrors the repo's existing `runtime-staging`
# fixed-tag channel convention.
#
# Called by semantic-release (@semantic-release/exec publishCmd) on the pre-main
# staging channel, AFTER @semantic-release/github has created the versioned
# v<version> prerelease — which carries the Meridian.app.tar.gz that
# latest.json's `url` points at. So we only mirror latest.json here; the signed
# tarball stays on the immutable versioned release.
#
#   scripts/mirror-staging-release.sh <version>
#   MERIDIAN_TARGET=aarch64-apple-darwin scripts/mirror-staging-release.sh <version>
#
# MERIDIAN_TARGET selects the target triple's bundle tree, defaulting to
# universal-apple-darwin so the current staging path is untouched.
#
# CAUTION for the per-arch world: this script CLOBBERS latest.json on the fixed
# updater-staging tag. That is correct while one job owns the whole manifest, as
# the universal build does. Two per-arch runners each mirroring their own
# fragment here would race and leave the staging channel covering whichever arch
# finished last. So under a per-arch release the MERGED manifest must be
# mirrored by the join job, once, and this script should be called (if at all)
# only for its DMG upload. The arch-suffixed DMG name below is safe to upload
# concurrently precisely because the two runners no longer share a filename.
set -euo pipefail

VERSION="${1:?usage: mirror-staging-release.sh <version>}"
VERSION="${VERSION#v}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

TAG="updater-staging"
# `tauri build --target <triple>` bundles under target/<triple>/, not plain
# target/release/.
TARGET="${MERIDIAN_TARGET:-universal-apple-darwin}"
MAC="target/${TARGET}/release/bundle/macos"
LATEST="${MAC}/latest.json"

# Must mirror package-updater.sh's stable-DMG naming exactly — it is the file
# this uploads. Plain Meridian.dmg for universal keeps the staging tester's
# download link (…/releases/download/updater-staging/Meridian.dmg) working.
case "${TARGET}" in
  universal-apple-darwin) STABLE_DMG="Meridian.dmg" ;;
  aarch64-apple-darwin)   STABLE_DMG="Meridian-aarch64.dmg" ;;
  x86_64-apple-darwin)    STABLE_DMG="Meridian-x64.dmg" ;;
  *) echo "✗ mirror-staging-release: unsupported MERIDIAN_TARGET '${TARGET}'" >&2; exit 1 ;;
esac
DMG="target/${TARGET}/release/bundle/dmg/${STABLE_DMG}"

# Deliberately hardcoded to latest.json, NOT the triple-keyed fragment name
# package-updater.sh emits per-arch (updater-<triple>.json). Under a per-arch
# build there is no finished manifest on this runner to mirror — only half of
# one — so this finds nothing and the whole script no-ops.
#
# Note that the early exit below is BEFORE the DMG upload, so a per-arch run
# publishes no staging DMG either. That is the safe default (nothing raced,
# nothing half-published) but it means the join job owns BOTH the merged
# latest.json and the staging DMG uploads. If per-arch staging DMGs are wanted
# from the build runners instead, this early exit has to move below the DMG
# block — a deliberate change, not something to fix by accident.
#
# The same no-op also covers the original case: package-updater.sh skips
# latest.json entirely when updater artifacts weren't signed (no
# TAURI_SIGNING_PRIVATE_KEY). Nothing to mirror then either — don't fail.
if [[ ! -f "${LATEST}" ]]; then
  echo "→ ${LATEST} absent — updater artifacts not built; skipping staging mirror"
  exit 0
fi

# Ensure the rolling prerelease exists (idempotent across runs). Marked
# --prerelease so GitHub's "latest" (the production endpoint) never resolves to
# it. Anchored to pre-main at first creation; the tag never moves afterwards —
# only its attached assets are clobbered, and the asset download URL is
# tag-stable regardless of which commit the tag points at.
if ! gh release view "${TAG}" >/dev/null 2>&1; then
  gh release create "${TAG}" \
    --prerelease \
    --target pre-main \
    --title "Updater staging channel (rolling)" \
    --notes "Rolling staging updater channel. The latest.json here always points at the newest pre-main staging build, so installed staging apps self-update. Auto-managed by scripts/mirror-staging-release.sh — do not edit by hand."
  echo "✓ created rolling ${TAG} prerelease"
fi

# Clobber the manifest so the fixed endpoint serves the newest build.
gh release upload "${TAG}" "${LATEST}" --clobber
echo "✓ ${TAG}/latest.json ← v${VERSION}"

# Also publish a stable-named DMG for a fixed staging-tester download link
# (…/releases/download/updater-staging/Meridian.dmg). Not required for
# auto-update (latest.json's url points at the versioned tarball) — convenience.
if [[ -f "${DMG}" ]]; then
  gh release upload "${TAG}" "${DMG}" --clobber
  echo "✓ ${TAG}/${STABLE_DMG} ← v${VERSION}"
fi
