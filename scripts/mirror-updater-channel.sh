#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Put the freshly-composed updater manifest where installed apps of a given
# channel actually look for it.
#
#   scripts/mirror-updater-channel.sh <version> <channel>     # channel: stable | staging
#
# Run ONCE, by the publish job, after the merged latest.json exists and the
# versioned release has been made public. Never per-architecture — see WHY below.
#
# ── THE TWO CHANNELS RESOLVE THEIR ENDPOINT DIFFERENTLY ──────────────────────
#
# stable   tray/src-tauri/tauri.conf.json bakes
#            …/releases/latest/download/latest.json
#          `releases/latest` is resolved BY GITHUB to the most recent
#          NON-PRERELEASE release. So publishing the versioned release is what
#          makes its latest.json live — there is nothing to mirror, and copying
#          it anywhere else would create a second source of truth that could
#          drift. This script is therefore a deliberate NO-OP for stable.
#
#          (This is also why staging releases MUST stay flagged as prereleases:
#          a staging build that is not marked prerelease would become
#          `releases/latest` and every stable user would be offered it.)
#
# staging  tray/src-tauri/tauri.staging.conf.json bakes a FIXED tag
#            …/releases/download/updater-staging/latest.json
#          because staging releases are prereleases and can never be
#          `releases/latest`. So the manifest has to be mirrored onto that
#          rolling tag explicitly.
#
# ── WHY THIS IS A JOIN-JOB SCRIPT, NOT A PER-RUNNER ONE ──────────────────────
#
# It CLOBBERS latest.json on a fixed tag. That is correct exactly once per
# release, when one caller owns the whole merged manifest. Two per-arch runners
# each mirroring their own fragment would race, and the staging channel would
# end up advertising only whichever architecture finished last — with the other
# architecture's users silently stranded on their current version. The failure
# is invisible from the release page, which is what makes it dangerous.
#
# The DMG copies below are safe to push from anywhere only because per-arch
# builds no longer share a filename; they are done here for the same reason
# anyway — one place, after everything is known good.
set -euo pipefail

VERSION="${1:?usage: mirror-updater-channel.sh <version> <channel>}"
CHANNEL="${2:?usage: mirror-updater-channel.sh <version> <channel>}"
VERSION="${VERSION#v}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

case "${CHANNEL}" in
    stable)
        echo "→ mirror-updater-channel: stable resolves via releases/latest — nothing to mirror"
        echo "  (the published v${VERSION} release IS the endpoint)"
        exit 0
        ;;
    staging) ;;
    *)
        echo "✗ mirror-updater-channel: unknown channel '${CHANNEL}' (expected stable|staging)" >&2
        exit 1
        ;;
esac

TAG="updater-staging"
REPO="Meridiona/meridian"

[[ -f latest.json ]] || {
    echo "✗ mirror-updater-channel: no latest.json in $(pwd) — compose it first" >&2
    exit 1
}

# Refuse to mirror a manifest for a different version than we were asked to
# publish. Catches a stale latest.json left in the working directory, which
# would otherwise point the whole staging channel at an older build.
MANIFEST_VERSION="$(python3 -c 'import json,sys; print(json.load(open("latest.json"))["version"])')"
if [[ "${MANIFEST_VERSION}" != "${VERSION}" ]]; then
    echo "✗ mirror-updater-channel: latest.json is version ${MANIFEST_VERSION}, expected ${VERSION}" >&2
    exit 1
fi

# The rolling tag is a prerelease that is reused forever; create it once.
if ! gh release view "${TAG}" --repo "${REPO}" >/dev/null 2>&1; then
    gh release create "${TAG}" \
        --repo "${REPO}" \
        --prerelease \
        --title "Staging updater channel" \
        --notes "Rolling pointer to the newest staging build. Not a release - see the versioned prereleases."
    echo "✓ created rolling ${TAG} release"
fi

# latest.json LAST is not an option here (it is the only manifest), but the DMGs
# go first so the fixed download links never point at a build the manifest has
# not yet caught up with.
shopt -s nullglob
dmgs=(target/*-apple-darwin/release/bundle/dmg/Meridian-*.dmg)
if [[ ${#dmgs[@]} -gt 0 ]]; then
    gh release upload "${TAG}" --repo "${REPO}" --clobber "${dmgs[@]}"
    for d in "${dmgs[@]}"; do echo "  → $(basename "${d}")"; done
else
    echo "  (no arch-suffixed DMGs present in this working tree — manifest only)"
fi

gh release upload "${TAG}" --repo "${REPO}" --clobber latest.json

echo "✓ staging channel now points at v${VERSION}"
echo "  https://github.com/${REPO}/releases/download/${TAG}/latest.json"
