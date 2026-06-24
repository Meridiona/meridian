#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Generate the DMG auto-update artifacts for the GitHub release:
#   - latest.json  — the manifest tauri-plugin-updater fetches from
#                    /releases/latest/download/latest.json (built from the REAL
#                    minisign signature `tauri build` just produced)
#   - Meridian.dmg — a stable-named copy of Meridian_<version>_aarch64.dmg so the
#                    public download link /releases/latest/download/Meridian.dmg
#                    is version-independent
#
# Called by semantic-release (@semantic-release/exec prepareCmd) AFTER the tray
# build, with the next version. @semantic-release/github then attaches the
# resulting files. Idempotent; safe to run locally to inspect the output.
#
#   scripts/package-updater.sh <version>
set -euo pipefail

VERSION="${1:?usage: package-updater.sh <version>}"
VERSION="${VERSION#v}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

REPO="Meridiona/meridian"
MAC="target/release/bundle/macos"
DMG_DIR="target/release/bundle/dmg"
SIG="${MAC}/Meridian.app.tar.gz.sig"

# The build fails earlier when createUpdaterArtifacts can't be signed (pubkey
# present, no TAURI_SIGNING_PRIVATE_KEY), so reaching here without a .sig means
# updater artifacts were intentionally off — skip rather than fail the release.
if [[ ! -f "${SIG}" ]]; then
  echo "→ ${SIG} absent — updater artifacts not built; skipping latest.json"
  exit 0
fi

# Stable-named DMG for a version-independent public download link.
VERSIONED_DMG="${DMG_DIR}/Meridian_${VERSION}_aarch64.dmg"
if [[ -f "${VERSIONED_DMG}" ]]; then
  cp "${VERSIONED_DMG}" "${DMG_DIR}/Meridian.dmg"
  echo "✓ ${DMG_DIR}/Meridian.dmg (copy of $(basename "${VERSIONED_DMG}"))"
else
  echo "⚠ ${VERSIONED_DMG} not found — no stable-named DMG (tarball update still works)"
fi

# latest.json from the real signature. The tarball URL points at the v<version>
# release tag the @semantic-release/git commit + @semantic-release/github release
# will create; the app reaches it via the /latest/ redirect baked in tauri.conf.json.
SIG_CONTENT="$(cat "${SIG}")"
URL="https://github.com/${REPO}/releases/download/v${VERSION}/Meridian.app.tar.gz"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

python3 - "${MAC}/latest.json" "${VERSION}" "${URL}" "${SIG_CONTENT}" "${PUB_DATE}" <<'PY'
import json, sys
out, ver, url, sig, pub = sys.argv[1:6]
json.dump(
    {
        "version": ver,
        "notes": f"Meridian v{ver}",
        "pub_date": pub,
        "platforms": {"darwin-aarch64": {"signature": sig, "url": url}},
    },
    open(out, "w"),
    indent=2,
)
PY
echo "✓ ${MAC}/latest.json (v${VERSION} → ${URL})"
