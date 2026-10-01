#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Generate the DMG auto-update artifacts for the GitHub release:
#   - latest.json  — the manifest tauri-plugin-updater fetches from
#                    /releases/latest/download/latest.json (built from the REAL
#                    minisign signature `tauri build` just produced).
#   - Meridian.dmg — a stable-named copy of the Meridian_<version>_*.dmg so the
#                    public download link /releases/latest/download/Meridian.dmg
#                    is version-independent
#
# Called by semantic-release (@semantic-release/exec prepareCmd) AFTER the tray
# build, with the next version. @semantic-release/github then attaches the
# resulting files. Idempotent; safe to run locally to inspect the output.
#
#   scripts/package-updater.sh <version>
#   MERIDIAN_TARGET=aarch64-apple-darwin scripts/package-updater.sh <version>
#
# ── universal vs per-arch ────────────────────────────────────────────────────
# MERIDIAN_TARGET selects the target triple whose bundle tree we package, and
# defaults to universal-apple-darwin so every existing caller keeps working
# byte-for-byte unchanged. It decides three things:
#
#   1. which platform keys latest.json carries. tauri-plugin-updater resolves
#      the key from the RUNNING app's arch, not from the manifest, so:
#        universal-apple-darwin → BOTH darwin-aarch64 and darwin-x86_64, pointing
#          at the ONE universal payload/signature (today's behaviour, unchanged)
#        aarch64-apple-darwin   → darwin-aarch64 only
#        x86_64-apple-darwin    → darwin-x86_64 only
#      A per-arch run therefore emits a manifest FRAGMENT, not a finished
#      manifest, and names it after its own triple:
#        universal-apple-darwin → macos/latest.json  (unchanged)
#        aarch64-apple-darwin   → macos/updater-aarch64-apple-darwin.json
#        x86_64-apple-darwin    → macos/updater-x86_64-apple-darwin.json
#      Merging the two fragments into the published latest.json is a
#      later join job's business — deliberately NOT done here, because this
#      script only ever sees one runner's half and a merge that guessed at the
#      other half is exactly how a release ships a manifest that strands an
#      entire architecture. (scripts/compose-updater-manifest.py is that join
#      job: it refuses to write anything it cannot first prove is complete.)
#
#   2. the stable-named DMG. The universal path must keep producing plain
#      Meridian.dmg — that filename is the public download link contract
#      (/releases/latest/download/Meridian.dmg) and cannot move. Per-arch builds
#      cannot share it (two runners would clobber each other's asset in the one
#      release), so they get Meridian-aarch64.dmg / Meridian-x64.dmg.
#
#   3. the updater payload's asset name. Same collision: both runners produce a
#      file literally called Meridian.app.tar.gz, and two same-named assets
#      cannot coexist in one GitHub release. Per-arch runs therefore also write
#      an arch-suffixed COPY of the tarball + .sig beside the originals and point
#      the manifest url at that name. The universal path keeps the bare
#      Meridian.app.tar.gz name that every installed app's manifest already
#      references.
set -euo pipefail

VERSION="${1:?usage: package-updater.sh <version>}"
VERSION="${VERSION#v}"
RELEASE_TAG="${MERIDIAN_RELEASE_TAG:-v${VERSION}}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

REPO="Meridiona/meridian"
# `tauri build --target <triple>` bundles under target/<triple>/, not plain
# target/release/.
TARGET="${MERIDIAN_TARGET:-universal-apple-darwin}"
BUNDLE="target/${TARGET}/release/bundle"
MAC="${BUNDLE}/macos"
DMG_DIR="${BUNDLE}/dmg"
SIG="${MAC}/Meridian.app.tar.gz.sig"

# ARCH_LABEL is the suffix that disambiguates per-arch assets inside a single
# GitHub release. Empty for universal — that is what preserves the unsuffixed
# Meridian.dmg / Meridian.app.tar.gz names the existing download link and every
# already-installed app depend on. Note this is OUR naming, not tauri's: it is
# derived from the triple we were given, so it does not depend on how
# tauri-bundler happens to spell the arch in the versioned DMG filename (which
# we only ever reach by glob).
case "${TARGET}" in
  universal-apple-darwin) PLATFORM_KEYS=(darwin-aarch64 darwin-x86_64); ARCH_LABEL="" ;;
  aarch64-apple-darwin)   PLATFORM_KEYS=(darwin-aarch64);               ARCH_LABEL="aarch64" ;;
  x86_64-apple-darwin)    PLATFORM_KEYS=(darwin-x86_64);                ARCH_LABEL="x64" ;;
  *)
    # Fail loudly rather than emit a manifest with no platform keys: a manifest
    # that parses but covers nothing is an update path that dies silently.
    echo "✗ package-updater: unsupported MERIDIAN_TARGET '${TARGET}'" >&2
    exit 1
    ;;
esac

if [[ -n "${ARCH_LABEL}" ]]; then
  STABLE_DMG="Meridian-${ARCH_LABEL}.dmg"
  UPDATER_ASSET="Meridian-${ARCH_LABEL}.app.tar.gz"
else
  STABLE_DMG="Meridian.dmg"
  UPDATER_ASSET="Meridian.app.tar.gz"
fi

# The manifest filename is keyed off the FULL TRIPLE, not off ARCH_LABEL, and
# the difference is deliberate. Two vocabularies are in play here: tauri-bundler
# spells the Intel arch `x64` (which is why the DMG and tarball above use it —
# those are user-facing download names and must match what the bundler emits),
# while the updater platform key spells it `x86_64`. Naming the manifest after
# either one invents a third mapping someone has to keep in their head, and the
# failure mode is a fragment the publish job silently does not find — a green
# release missing an entire platform. The triple is the value MERIDIAN_TARGET
# already carries, so it needs no mapping table at all. Universal keeps the
# plain `latest.json` name for backward-compatible/local callers; the current
# release workflow composes per-platform fragments into the published manifest.
if [[ "${TARGET}" == "universal-apple-darwin" ]]; then
  MANIFEST_NAME="latest.json"
else
  MANIFEST_NAME="updater-${TARGET}.json"
fi

# The build fails earlier when createUpdaterArtifacts can't be signed (pubkey
# present, no TAURI_SIGNING_PRIVATE_KEY), so reaching here without a .sig means
# updater artifacts were intentionally off — skip rather than fail the release.
if [[ ! -f "${SIG}" ]]; then
  echo "→ ${SIG} absent — updater artifacts not built; skipping latest.json"
  exit 0
fi

# Stable-named DMG for a version-independent public download link. The arch
# suffix tauri-bundler puts in the versioned filename differs per target (and is
# absent-but-not-empty for universal), so glob rather than reconstruct it — the
# glob is deliberately `_*` and matches every spelling.
VERSIONED_DMG="$(ls "${DMG_DIR}"/Meridian_${VERSION}_*.dmg 2>/dev/null | head -1)"
if [[ -n "${VERSIONED_DMG}" && -f "${VERSIONED_DMG}" ]]; then
  cp "${VERSIONED_DMG}" "${DMG_DIR}/${STABLE_DMG}"
  echo "✓ ${DMG_DIR}/${STABLE_DMG} (copy of $(basename "${VERSIONED_DMG}"))"
else
  echo "⚠ no Meridian_${VERSION}_*.dmg found in ${DMG_DIR} — no stable-named DMG (tarball update still works)"
fi

# Per-arch only: an arch-suffixed copy of the updater payload + its signature.
# Both runners emit a file named Meridian.app.tar.gz; GitHub releases hold one
# asset per name, so the second upload would silently replace the first and half
# the fleet would auto-update onto the wrong architecture. Copies (not renames)
# so anything downstream still expecting the canonical name keeps finding it.
if [[ -n "${ARCH_LABEL}" ]]; then
  cp "${MAC}/Meridian.app.tar.gz" "${MAC}/${UPDATER_ASSET}"
  cp "${SIG}" "${MAC}/${UPDATER_ASSET}.sig"
  echo "✓ ${MAC}/${UPDATER_ASSET} (+ .sig) — arch-suffixed updater payload"
fi

# Minimum-version enforcement is release policy, not build input. RC and fresh
# stable manifests are produced without a floor. A maintainer may arm or clear
# the floor after stable publication via .github/workflows/minimum-version.yml.
# Stable promotion carries any already-armed floor forward from the previous
# latest stable manifest.
MINIMUM=""
# latest.json from the real signature. The tarball URL points at the v<version>
# release tag the @semantic-release/git commit + @semantic-release/github release
# will create; the app reaches it via the /latest/ redirect baked in tauri.conf.json.
SIG_CONTENT="$(cat "${SIG}")"
URL="https://github.com/${REPO}/releases/download/${RELEASE_TAG}/${UPDATER_ASSET}"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# Universal writes the finished manifest as ${MAC}/latest.json exactly as before.
# A per-arch build writes ${MAC}/updater-<triple>.json — a self-describing
# FRAGMENT covering one platform key, which the publish job collects from both
# runners and joins into the real latest.json. The name says which runner
# produced it, so a fragment that goes missing is a loud absence rather than a
# manifest that quietly covers half the fleet.
python3 - "${MAC}/${MANIFEST_NAME}" "${VERSION}" "${URL}" "${SIG_CONTENT}" "${PUB_DATE}" "${PLATFORM_KEYS[@]}" <<'PY'
import json, sys
out, ver, url, sig, pub = sys.argv[1:6]
keys = sys.argv[6:]
notes = f"Meridian v{ver}"
# tauri-plugin-updater resolves the platform key from the RUNNING app's arch,
# not from the manifest. A universal build passes BOTH keys here and they share
# one payload + signature (the one universal tarball serves both arches); a
# per-arch build passes its single key, and the join job supplies the other.
platform = {"signature": sig, "url": url}
json.dump(
    {
        "version": ver,
        "notes": notes,
        "pub_date": pub,
        "platforms": {k: platform for k in keys},
    },
    open(out, "w"),
    indent=2,
)
PY
_covers="${PLATFORM_KEYS[*]}"
echo "✓ ${MAC}/${MANIFEST_NAME} (v${VERSION} → ${URL}, covers ${_covers})"
