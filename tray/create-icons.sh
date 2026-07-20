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
# Every icon size EXCEPT the macOS .icns (see MAC_MASTER below) is derived
# from the flat brand mark (the gradient spirograph on a transparent
# background, no platform chrome), NOT a synthetic placeholder. Swap MASTER
# to rebrand. This is correct for Windows' icon.ico, Linux's window-manager
# PNGs, and the menu-bar template — none of them want macOS-specific chrome
# baked in.
MASTER="${ICONS_DIR}/meridiona-mark.png"
if [[ ! -f "${MASTER}" ]]; then
    echo "✗ brand mark not found: ${MASTER}" >&2
    exit 1
fi

# Normalise the master to a square 1024×1024 RGBA source (preserves alpha).
SOURCE="${ICONS_DIR}/source-1024.png"
sips -z 1024 1024 "${MASTER}" --out "${SOURCE}" >/dev/null 2>&1
echo "  · source-1024.png derived from brand mark"

# ── Mac-specific master: the pre-chromed Dock/Finder icon ───────────────────
# Unlike iOS, macOS does NOT auto-mask a plain square icon into the Big
# Sur+ rounded-square shape or add its drop shadow — HIG requires the app's
# own icon.icns to already contain that rounded-square canvas + shadow,
# painted by the designer. Feeding the flat, unpadded MASTER above into the
# iconset (as this script used to) produces the exact "tiny mark floating in
# an oversized square, no shadow, inconsistent with every other Dock icon"
# look — the .icns analogue of the menu-bar padding bug fixed above.
# mac-icon-1024.png is that pre-chromed render (white rounded square +
# shadow already baked into its pixels); it's used ONLY for icon.icns below.
MAC_MASTER="${ICONS_DIR}/mac-icon-1024.png"
if [[ ! -f "${MAC_MASTER}" ]]; then
    echo "✗ mac-chromed icon not found: ${MAC_MASTER}" >&2
    exit 1
fi
MAC_SOURCE="${ICONS_DIR}/mac-source-1024.png"
sips -z 1024 1024 "${MAC_MASTER}" --out "${MAC_SOURCE}" >/dev/null 2>&1
echo "  · mac-source-1024.png derived from the pre-chromed Dock icon"

# ── tray-template.png — the macOS menu-bar template icon ────────────────────
# The menu-bar tray icon must be a template image: a monochrome silhouette
# whose SHAPE comes purely from the alpha channel (macOS ignores RGB and tints
# it to match the light/dark menu bar) — a mechanism that structurally cannot
# preserve color or gradients, confirmed against Apple's own conventions
# (there's no official pixel spec for NSStatusItem icons, but `isTemplate`
# has always been black-and-clear-only). The full gradient brand mark
# (meridiona-mark.png) doesn't survive that: its strokes are thin hairlines
# that box-downsample to ~8% mean alpha, invisible against a real menu bar
# (confirmed against a live build). Every well-behaved app with a detailed
# app icon ships a SEPARATE, purpose-built glyph for the menu bar rather than
# reusing it — tray-icon-source.png is that dedicated glyph: the same
# spirograph motif redrawn with bold, fully-opaque strokes specifically for
# this context, not a crop of the app icon.
#
# tray-icon-source.png's own pixels (non-white, non-transparent) become the
# opaque part of the silhouette; white and background both become
# transparent. Its content also isn't padded to an app-icon safe area, so
# the bounding-box crop below is mostly a no-op margin trim, not load-bearing
# the way it was for the gradient mark.
#
# Even these bolder strokes are only ~10-15px wide at source resolution,
# which is sub-pixel once box-downsampled to 44×44 (mean alpha ~18%,
# confirmed against a live build: visibly dull/gray next to neighboring
# menu-bar icons, not the bright white they render at). The `** GAMMA` step
# contrast-boosts that coverage the same way it did for the gradient mark,
# lifting partially-covered pixels toward full opacity. Picked by eye against
# a simulated dark-menu-bar composite; retune if the glyph's stroke weight
# changes.
#
# Written in pure-stdlib python3 (zlib for the PNG codec) for the same reason
# icon.ico is below: no ImageMagick/Pillow dependency assumed on dev machines
# or CI runners.
TRAY_SOURCE="${ICONS_DIR}/tray-icon-source.png"
if [[ ! -f "${TRAY_SOURCE}" ]]; then
    echo "✗ menu-bar glyph not found: ${TRAY_SOURCE}" >&2
    exit 1
fi
echo "→ generating tray-template.png (menu-bar template icon)"
python3 - "${TRAY_SOURCE}" "${ICONS_DIR}/tray-template.png" 44 <<'PY'
import struct, sys, zlib

def decode_png(path):
    with open(path, 'rb') as f:
        buf = f.read()
    pos = 8
    width = height = color_type = None
    idat = bytearray()
    while pos < len(buf):
        length = struct.unpack('>I', buf[pos:pos + 4])[0]; pos += 4
        ctype = buf[pos:pos + 4].decode('ascii'); pos += 4
        data = buf[pos:pos + length]; pos += length
        pos += 4  # crc
        if ctype == 'IHDR':
            width, height = struct.unpack('>II', data[0:8])
            color_type = data[9]
        elif ctype == 'IDAT':
            idat += data
        elif ctype == 'IEND':
            break
    raw = zlib.decompress(bytes(idat))
    channels = 4 if color_type == 6 else 3
    stride = width * channels
    out = bytearray(height * stride)
    rpos = 0
    for y in range(height):
        filt = raw[rpos]; rpos += 1
        row_start = y * stride
        for x in range(stride):
            raw_byte = raw[rpos + x]
            a = out[row_start + x - channels] if x >= channels else 0
            b = out[row_start - stride + x] if y > 0 else 0
            c = out[row_start - stride + x - channels] if (x >= channels and y > 0) else 0
            if filt == 0:
                val = raw_byte
            elif filt == 1:
                val = raw_byte + a
            elif filt == 2:
                val = raw_byte + b
            elif filt == 3:
                val = raw_byte + (a + b) // 2
            elif filt == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                val = raw_byte + pr
            else:
                raise ValueError(f'unknown filter {filt}')
            out[row_start + x] = val & 0xff
        rpos += stride
    return width, height, channels, bytes(out)

def encode_png(width, height, rgba):
    stride = width * 4
    raw = bytearray((stride + 1) * height)
    for y in range(height):
        raw[y * (stride + 1)] = 0
        raw[y * (stride + 1) + 1: y * (stride + 1) + 1 + stride] = rgba[y * stride: y * stride + stride]
    idat = zlib.compress(bytes(raw), 9)

    def chunk(ctype, data):
        return (struct.pack('>I', len(data)) + ctype.encode('ascii') + data
                + struct.pack('>I', zlib.crc32(ctype.encode('ascii') + data) & 0xffffffff))

    sig = bytes([137, 80, 78, 71, 13, 10, 26, 10])
    ihdr = struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0)
    return sig + chunk('IHDR', ihdr) + chunk('IDAT', idat) + chunk('IEND', b'')

GAMMA = 0.35  # <1 lifts partial coverage toward opaque; 1.0 = raw coverage
CROP_MARGIN = 0.06  # fraction of the mark's bbox extent kept as breathing room

src_path, dest_path, out_size = sys.argv[1], sys.argv[2], int(sys.argv[3])
width, height, channels, data = decode_png(src_path)
if channels != 4:
    sys.exit('expected RGBA source')

mask = [0.0] * (width * height)
minx, miny, maxx, maxy = width, height, 0, 0
for y in range(height):
    for x in range(width):
        i = (y * width + x) * channels
        r, g, b, a = data[i], data[i + 1], data[i + 2], data[i + 3]
        is_white = r > 240 and g > 240 and b > 240
        v = (a / 255.0) if (a > 0 and not is_white) else 0.0
        mask[y * width + x] = v
        if v > 0:
            minx, maxx = min(minx, x), max(maxx, x)
            miny, maxy = min(miny, y), max(maxy, y)

# Crop to the mark's real content (see the comment above), squared around its
# center so the downsample below doesn't stretch it.
margin = round(max(maxx - minx, maxy - miny) * CROP_MARGIN)
cx0, cy0 = max(minx - margin, 0), max(miny - margin, 0)
cx1, cy1 = min(maxx + margin, width - 1), min(maxy + margin, height - 1)
side = max(cx1 - cx0, cy1 - cy0)
cx0 = max(min(cx0 - (side - (cx1 - cx0)) // 2, width - side), 0)
cy0 = max(min(cy0 - (side - (cy1 - cy0)) // 2, height - side), 0)

scale = side / out_size
out = bytearray(out_size * out_size * 4)
for oy in range(out_size):
    y0 = cy0 + int(oy * scale)
    y1 = max(cy0 + int((oy + 1) * scale), y0 + 1)
    for ox in range(out_size):
        x0 = cx0 + int(ox * scale)
        x1 = max(cx0 + int((ox + 1) * scale), x0 + 1)
        total = 0.0
        count = 0
        for y in range(y0, y1):
            base = y * width
            for x in range(x0, x1):
                total += mask[base + x]
                count += 1
        coverage = total / count
        boosted = coverage ** GAMMA if coverage > 0 else 0.0
        alpha = round(min(boosted, 1.0) * 255)
        i = (oy * out_size + ox) * 4
        out[i] = 0; out[i + 1] = 0; out[i + 2] = 0; out[i + 3] = alpha

with open(dest_path, 'wb') as f:
    f.write(encode_png(out_size, out_size, bytes(out)))
print(f'  · {dest_path} ({out_size}x{out_size})')
PY

# ── Resize using sips (macOS built-in) ──────────────────────────────────────

resize() {
    local size="$1" dest="$2"
    sips -z "${size}" "${size}" "${SOURCE}" --out "${dest}" >/dev/null 2>&1
}

# Only what's actually consumed: 32x32.png/128x128.png are in tauri.conf.json's
# bundle.icon; all four (plus 256x256.png) are inputs to icon.ico below. No
# other size is referenced anywhere, so none of icon.png/128x128@2x.png/
# 512x512.png/tray.png are generated — they were dead build output.
echo "→ generating icon sizes"
resize 32  "${ICONS_DIR}/32x32.png"
resize 64  "${ICONS_DIR}/32x32@2x.png"
resize 128 "${ICONS_DIR}/128x128.png"
resize 256 "${ICONS_DIR}/256x256.png"

# iconset for .icns — from MAC_SOURCE (pre-chromed), not SOURCE (flat mark).
resize_mac() {
    local size="$1" dest="$2"
    sips -z "${size}" "${size}" "${MAC_SOURCE}" --out "${dest}" >/dev/null 2>&1
}
resize_mac 16  "${ICONSET}/icon_16x16.png"
resize_mac 32  "${ICONSET}/icon_16x16@2x.png"
resize_mac 32  "${ICONSET}/icon_32x32.png"
resize_mac 64  "${ICONSET}/icon_32x32@2x.png"
resize_mac 128 "${ICONSET}/icon_128x128.png"
resize_mac 256 "${ICONSET}/icon_128x128@2x.png"
resize_mac 256 "${ICONSET}/icon_256x256.png"
resize_mac 512 "${ICONSET}/icon_256x256@2x.png"
resize_mac 512 "${ICONSET}/icon_512x512.png"
resize_mac 1024 "${ICONSET}/icon_512x512@2x.png"

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
