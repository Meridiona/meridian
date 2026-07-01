#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Force-update a runtime channel's rolling GitHub release with the freshly built
# tarball + manifest. Shared by the publish-staging / publish-production jobs in
# build-mlx-runtime.yml so the release mechanics live in exactly one place.
#
# The app pins the channel's runtime-manifest.json (RUNTIME_MANIFEST_URL, or
# MERIDIAN_RUNTIME_MANIFEST_URL for staging). The rolling release is created once
# — always a prerelease, so it never competes with the app's semantic-release for
# the "Latest" slot — then its assets are clobbered on every publish. The version
# record lives inside the manifest (and in the git tag, when a tag triggered it).
#
# Inputs (env): CHANNEL (runtime-staging | runtime-latest), GH_TOKEN, and the
# default GITHUB_REPOSITORY. Expects dist/ to hold the downloaded artifact.
set -euo pipefail

: "${CHANNEL:?CHANNEL required (runtime-staging | runtime-latest)}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY required}"

gh release view "${CHANNEL}" --repo "${GITHUB_REPOSITORY}" >/dev/null 2>&1 \
  || gh release create "${CHANNEL}" \
       --repo "${GITHUB_REPOSITORY}" \
       --title "MLX runtime (${CHANNEL#runtime-})" \
       --prerelease \
       --notes "Rolling ${CHANNEL#runtime-} self-contained MLX runtime. Auto-updated by build-mlx-runtime.yml; the app pins this release's runtime-manifest.json."

gh release upload "${CHANNEL}" \
  dist/meridian-mlx-runtime-*-aarch64.tar.gz \
  dist/runtime-manifest.json \
  --clobber --repo "${GITHUB_REPOSITORY}"

echo "✓ published ${CHANNEL} from $(ls dist/meridian-mlx-runtime-*-aarch64.tar.gz)"
