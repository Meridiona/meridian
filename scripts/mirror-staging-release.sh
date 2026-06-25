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
set -euo pipefail

VERSION="${1:?usage: mirror-staging-release.sh <version>}"
VERSION="${VERSION#v}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

TAG="updater-staging"
MAC="target/release/bundle/macos"
DMG="target/release/bundle/dmg/Meridian.dmg"
LATEST="${MAC}/latest.json"

# package-updater.sh skips latest.json when updater artifacts weren't signed
# (no TAURI_SIGNING_PRIVATE_KEY). Nothing to mirror then — no-op, don't fail.
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
  echo "✓ ${TAG}/Meridian.dmg ← v${VERSION}"
fi
