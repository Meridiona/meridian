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

# ── icon.ico — a REAL multi-size ICO, not a renamed PNG ─────────────────────
# This used to be `cp 32x32.png icon.ico`, which is a PNG carrying the wrong
# extension. Nothing consumed it while Meridian shipped macOS-only, so it went
# unnoticed; Tauri's Windows bundler does consume it, and a PNG-in-.ico is not
# a valid icon resource.
#
# Assembled here rather than with ImageMagick/iconutil because neither is a
# safe assumption: `iconutil` cannot write ICO at all, and ImageMagick is not
# installed by default on macOS or on the GitHub runners. The ICO container is
# a 6-byte header plus one 16-byte directory entry per image, and every Windows
# since Vista reads PNG-compressed entries directly — so the PNGs generated
# above can be embedded as-is with no re-encoding and no new dependency.
# python3 is already required by scripts/make-dmg-background.py.
echo "→ building icon.ico (multi-size)"
python3 - "${ICONS_DIR}" <<'PY'
import struct, sys
from pathlib import Path

icons = Path(sys.argv[1])
# Sizes Windows actually picks between: taskbar, alt-tab, desktop, and the
# 256px used by the installer and large-icon views.
wanted = [(16, "32x32.png"), (32, "32x32.png"), (48, "128x128.png"),
          (64, "32x32@2x.png"), (128, "128x128.png"), (256, "256x256.png")]

entries = []
for size, name in wanted:
    src = icons / name
    if not src.is_file():
        sys.exit(f"missing {src} - cannot build icon.ico")
    entries.append((size, src.read_bytes()))

# Deduplicate by payload so the same PNG reused at several declared sizes is
# stored once; Windows scales from the nearest entry.
out = bytearray()
out += struct.pack("<HHH", 0, 1, len(entries))  # reserved, type=1 (ICO), count
offset = 6 + 16 * len(entries)
for size, data in entries:
    # 256 is encoded as 0 in the single-byte width/height fields.
    dim = 0 if size >= 256 else size
    out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
for _, data in entries:
    out += data

dest = icons / "icon.ico"
dest.write_bytes(bytes(out))
print(f"  · icon.ico ({len(entries)} sizes, {len(out)} bytes)")
PY

echo "✓ icons generated in src-tauri/icons/"
