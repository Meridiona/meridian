#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Generate the Windows auto-update artifacts for the GitHub release:
#   - Meridian-x86_64-setup.exe — a stable-named copy of the versioned NSIS
#                                 installer, so the public download link
#                                 /releases/latest/download/Meridian-x86_64-setup.exe
#                                 is version-independent.
#   - updater-windows.json      — a manifest FRAGMENT covering only the
#                                 windows-x86_64 platform key, joined with the
#                                 macOS fragments by
#                                 scripts/compose-updater-manifest.py in the
#                                 publish job. Mirrors scripts/package-updater.sh's
#                                 per-arch fragment shape — see that script's
#                                 header for why composing once, after every
#                                 runner finishes, is what avoids the
#                                 tauri-action read-modify-write race.
#
#   scripts/package-updater-windows.sh <version>
#
# Called from the `windows` job in .github/workflows/release-build.yml, right
# after `npm run build:windows[:staging]` has produced the NSIS bundle.
# Idempotent; safe to run locally (on Windows, via Git Bash) to inspect output.
set -euo pipefail

VERSION="${1:?usage: package-updater-windows.sh <version>}"
VERSION="${VERSION#v}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

REPO="Meridiona/meridian"
NSIS_DIR="target/release/bundle/nsis"
STABLE_SETUP="Meridian-x86_64-setup.exe"

SETUP="$(ls "${NSIS_DIR}"/*-setup.exe 2>/dev/null | head -1)"
[[ -n "${SETUP}" ]] || { echo "✗ package-updater-windows: no NSIS installer in ${NSIS_DIR}" >&2; exit 1; }

# createUpdaterArtifacts (tauri.conf.json) signs the installer at bundle time
# using TAURI_SIGNING_PRIVATE_KEY; a missing .sig means that secret was absent
# rather than the build having failed outright — skip rather than fail the
# release, mirroring package-updater.sh's macOS behaviour.
SIG="${SETUP}.sig"
if [[ ! -f "${SIG}" ]]; then
  echo "→ ${SIG} absent — updater artifacts not built; skipping updater-windows.json"
  exit 0
fi

cp "${SETUP}" "${NSIS_DIR}/${STABLE_SETUP}"
cp "${SIG}" "${NSIS_DIR}/${STABLE_SETUP}.sig"
echo "✓ ${NSIS_DIR}/${STABLE_SETUP} (copy of $(basename "${SETUP}"))"

# Optional mandatory-update floor — same file, same rule, same validation as
# package-updater.sh's macOS copy. Both jobs run in parallel on separate
# runners and each independently refuses a malformed value; that is
# deliberate redundancy; a floor is exactly as necessary on Windows as on
# macOS, and there is no single place both jobs could share the check from
# without a job dependency neither needs otherwise.
MIN_FILE="tray/minimum-version"
MINIMUM=""
if [[ -f "${MIN_FILE}" ]]; then
  MINIMUM="$(tr -d '[:space:]' < "${MIN_FILE}")"
  if [[ -n "${MINIMUM}" && ! "${MINIMUM}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "✗ ${MIN_FILE} contains '${MINIMUM}' — not a plain semver (X.Y.Z)" >&2
    exit 1
  fi
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

SIG_CONTENT="$(cat "${SIG}")"
URL="https://github.com/${REPO}/releases/download/v${VERSION}/${STABLE_SETUP}"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# Two platform keys, not one, both pointing at the SAME installer + signature.
# tauri-plugin-updater >= 2.10 also looks up an `{os}-{arch}-{installer}` key
# (updaterJsonPreferNsis defaults to false "for legacy reasons", which only
# matters when both an MSI and an NSIS entry exist — we ship NSIS only, so
# there is nothing for the preference to pick wrong, but a client on a newer
# updater plugin version may still look specifically for `windows-x86_64-nsis`
# rather than falling back to the bare `windows-x86_64` key). Publishing both
# means either lookup finds a valid entry regardless of updater-plugin version.
python3 - "${NSIS_DIR}/updater-windows.json" "${VERSION}" "${URL}" "${SIG_CONTENT}" "${PUB_DATE}" "${MINIMUM}" <<'PY'
import json, sys
out, ver, url, sig, pub, minimum = sys.argv[1:7]
notes = f"Meridian v{ver}"
if minimum:
    notes += f"\nMinimum-Version: {minimum}"
platform = {"signature": sig, "url": url}
json.dump(
    {
        "version": ver,
        "notes": notes,
        "pub_date": pub,
        "platforms": {"windows-x86_64": platform, "windows-x86_64-nsis": platform},
    },
    open(out, "w"),
    indent=2,
)
PY

if [[ -n "${MINIMUM}" ]]; then
  echo "✓ ${NSIS_DIR}/updater-windows.json (v${VERSION} → ${URL}, covers windows-x86_64 + windows-x86_64-nsis, minimum supported v${MINIMUM})"
else
  echo "✓ ${NSIS_DIR}/updater-windows.json (v${VERSION} → ${URL}, covers windows-x86_64 + windows-x86_64-nsis)"
fi
