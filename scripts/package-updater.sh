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

# Optional mandatory-update floor. When tray/minimum-version contains a semver,
# it ships as a `Minimum-Version: <v>` line inside the manifest notes — installed
# apps running BELOW that version install this release automatically instead of
# waiting for the banner click (tray/src-tauri/src/update.rs
# enforce_minimum_version; the notes line is the transport because
# tauri-plugin-updater drops unknown manifest fields). File absent or empty =
# every update stays consent-based. A malformed value fails the release loudly:
# a typo silently dropping the floor would defeat the point of setting it.
MIN_FILE="tray/minimum-version"
MINIMUM=""
if [[ -f "${MIN_FILE}" ]]; then
  MINIMUM="$(tr -d '[:space:]' < "${MIN_FILE}")"
  if [[ -n "${MINIMUM}" && ! "${MINIMUM}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "✗ ${MIN_FILE} contains '${MINIMUM}' — not a plain semver (X.Y.Z)" >&2
    exit 1
  fi
  # The floor must not exceed the release that carries it — a floor above every
  # published version would make old apps force-install a build that is itself
  # still below the minimum (self-terminating, but confusing). Always an
  # operator error, so fail loudly. Cores only (X.Y.Z), so a prerelease
  # release version (1.70.0-staging.1) may carry a 1.70.0 floor.
  if [[ -n "${MINIMUM}" ]]; then
    python3 - "${MINIMUM}" "${VERSION}" <<'PY' || exit 1
import sys
minimum, ver = sys.argv[1], sys.argv[2]
core = lambda v: tuple(int(x) for x in v.split("-")[0].split("."))
if core(minimum) > core(ver):
    sys.exit(f"✗ tray/minimum-version {minimum} exceeds the release version {ver}")
PY
  fi
fi

# latest.json from the real signature. The tarball URL points at the v<version>
# release tag the @semantic-release/git commit + @semantic-release/github release
# will create; the app reaches it via the /latest/ redirect baked in tauri.conf.json.
SIG_CONTENT="$(cat "${SIG}")"
URL="https://github.com/${REPO}/releases/download/v${VERSION}/Meridian.app.tar.gz"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

python3 - "${MAC}/latest.json" "${VERSION}" "${URL}" "${SIG_CONTENT}" "${PUB_DATE}" "${MINIMUM}" <<'PY'
import json, sys
out, ver, url, sig, pub, minimum = sys.argv[1:7]
notes = f"Meridian v{ver}"
if minimum:
    notes += f"\nMinimum-Version: {minimum}"
json.dump(
    {
        "version": ver,
        "notes": notes,
        "pub_date": pub,
        "platforms": {"darwin-aarch64": {"signature": sig, "url": url}},
    },
    open(out, "w"),
    indent=2,
)
PY
if [[ -n "${MINIMUM}" ]]; then
  echo "✓ ${MAC}/latest.json (v${VERSION} → ${URL}, minimum supported v${MINIMUM})"
else
  echo "✓ ${MAC}/latest.json (v${VERSION} → ${URL})"
fi
