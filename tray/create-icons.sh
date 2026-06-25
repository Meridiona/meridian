#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Generate all Tauri icon sizes from the Meridian brand mark.
# Run from the tray/ directory before building: bash create-icons.sh
# Requires: sips (macOS built-in), iconutil (macOS built-in)
set -euo pipefail

ICONS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/src-tauri/icons"
mkdir -p "${ICONS_DIR}"
ICONSET="${ICONS_DIR}/icon.iconset"
mkdir -p "${ICONSET}"

# ── Master source: the real Meridian brand mark ─────────────────────────────
# Every icon size is derived from the brand mark (the gradient spirograph on a
# transparent background), NOT a synthetic placeholder. Swap MASTER to rebrand.
MASTER="${ICONS_DIR}/meridiona-mark.png"
if [[ ! -f "${MASTER}" ]]; then
    echo "✗ brand mark not found: ${MASTER}" >&2
    exit 1
fi

# Normalise the master to a square 1024×1024 RGBA source (preserves alpha).
SOURCE="${ICONS_DIR}/source-1024.png"
sips -z 1024 1024 "${MASTER}" --out "${SOURCE}" >/dev/null 2>&1
# Keep tray.png in sync with the mark (Tauri loads meridiona-mark.png directly,
# but other tooling may expect tray.png).
sips -z 32 32 "${MASTER}" --out "${ICONS_DIR}/tray.png" >/dev/null 2>&1
echo "  · source-1024.png and tray.png derived from brand mark"

# ── Resize using sips (macOS built-in) ──────────────────────────────────────

resize() {
    local size="$1" dest="$2"
    sips -z "${size}" "${size}" "${SOURCE}" --out "${dest}" >/dev/null 2>&1
}

echo "→ generating icon sizes"
resize 32  "${ICONS_DIR}/32x32.png"
resize 64  "${ICONS_DIR}/32x32@2x.png"
resize 128 "${ICONS_DIR}/128x128.png"
resize 256 "${ICONS_DIR}/128x128@2x.png"
resize 256 "${ICONS_DIR}/256x256.png"
resize 512 "${ICONS_DIR}/512x512.png"
resize 512 "${ICONS_DIR}/icon.png"

# iconset for .icns
resize 16  "${ICONSET}/icon_16x16.png"
resize 32  "${ICONSET}/icon_16x16@2x.png"
resize 32  "${ICONSET}/icon_32x32.png"
resize 64  "${ICONSET}/icon_32x32@2x.png"
resize 128 "${ICONSET}/icon_128x128.png"
resize 256 "${ICONSET}/icon_128x128@2x.png"
resize 256 "${ICONSET}/icon_256x256.png"
resize 512 "${ICONSET}/icon_256x256@2x.png"
resize 512 "${ICONSET}/icon_512x512.png"
resize 1024 "${ICONSET}/icon_512x512@2x.png"

echo "→ building icon.icns"
iconutil -c icns "${ICONSET}" -o "${ICONS_DIR}/icon.icns"
rm -rf "${ICONSET}"

# .ico — simple copy of 32x32 (proper multi-size .ico needs extra tooling)
cp "${ICONS_DIR}/32x32.png" "${ICONS_DIR}/icon.ico"

echo "✓ icons generated in src-tauri/icons/"
