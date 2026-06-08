#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Generate all Tauri icon sizes from a programmatic monochrome source.
# Run from the tray/ directory before building: bash create-icons.sh
# Requires: Python 3 (stdlib only), sips (macOS built-in), iconutil (macOS built-in)
set -euo pipefail

ICONS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/src-tauri/icons"
mkdir -p "${ICONS_DIR}"
ICONSET="${ICONS_DIR}/icon.iconset"
mkdir -p "${ICONSET}"

# ── Generate a 1024×1024 RGBA PNG source icon using Python stdlib ────────────
# Design: warm amber filled circle with a white "M" letterform, on transparent bg.
python3 - <<'PYEOF'
import struct, zlib, math, os

def png_chunk(tag, data):
    crc_data = tag + data
    return struct.pack('>I', len(data)) + crc_data + struct.pack('>I', zlib.crc32(crc_data) & 0xffffffff)

def make_rgba_png(filename, size, draw_fn):
    rows = []
    for y in range(size):
        row = [0]  # filter type = None
        for x in range(size):
            r, g, b, a = draw_fn(x, y, size)
            row += [r, g, b, a]
        rows.append(bytes(row))

    raw = b''.join(rows)
    compressed = zlib.compress(raw, 9)

    ihdr = struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0)  # 8-bit RGBA
    png = (
        b'\x89PNG\r\n\x1a\n'
        + png_chunk(b'IHDR', ihdr)
        + png_chunk(b'IDAT', compressed)
        + png_chunk(b'IEND', b'')
    )
    with open(filename, 'wb') as f:
        f.write(png)

AMBER_R, AMBER_G, AMBER_B = 196, 130, 42   # Meridian accent (#C4822A)

def draw_app_icon(x, y, size):
    cx, cy = size / 2, size / 2
    radius = size * 0.45
    # Circle (anti-aliased via distance)
    dist = math.hypot(x - cx, y - cy)
    if dist > radius + 1:
        return 0, 0, 0, 0   # transparent
    alpha = int(255 * max(0, min(1, radius + 1 - dist)))
    # White "M" letterform — two outer posts + centre V
    rel_x = (x - cx) / (size * 0.3)
    rel_y = (y - cy) / (size * 0.3)
    in_left   = -0.9 <= rel_x <= -0.55 and -0.8 <= rel_y <= 0.8
    in_right  =  0.55 <= rel_x <=  0.9 and -0.8 <= rel_y <= 0.8
    # V: left arm y = rel_x + 0.55 (for rel_x in [-0.55..0]), right arm mirrored
    in_v_left  = -0.55 <= rel_x <= 0 and abs(rel_y - (rel_x + 0.55)) <= 0.18 and rel_y >= -0.3
    in_v_right =   0 <= rel_x <= 0.55 and abs(rel_y - (-rel_x + 0.55)) <= 0.18 and rel_y >= -0.3
    if in_left or in_right or in_v_left or in_v_right:
        return 255, 255, 255, alpha    # white mark
    return AMBER_R, AMBER_G, AMBER_B, alpha

# Tray icon: smaller, monochrome, dark symbol on transparent (template image)
def draw_tray_icon(x, y, size):
    cx, cy = size / 2, size / 2
    radius = size * 0.45
    dist = math.hypot(x - cx, y - cy)
    # No background fill — just the mark for template image rendering
    rel_x = (x - cx) / (size * 0.3)
    rel_y = (y - cy) / (size * 0.3)
    in_left   = -0.9 <= rel_x <= -0.55 and -0.8 <= rel_y <= 0.8
    in_right  =  0.55 <= rel_x <=  0.9 and -0.8 <= rel_y <= 0.8
    in_v_left  = -0.55 <= rel_x <= 0 and abs(rel_y - (rel_x + 0.55)) <= 0.18 and rel_y >= -0.3
    in_v_right =   0 <= rel_x <= 0.55 and abs(rel_y - (-rel_x + 0.55)) <= 0.18 and rel_y >= -0.3
    if in_left or in_right or in_v_left or in_v_right:
        if dist < radius:
            return 0, 0, 0, 220    # near-black mark
    return 0, 0, 0, 0

icons_dir = os.environ.get('ICONS_DIR', 'src-tauri/icons')
make_rgba_png(os.path.join(icons_dir, 'source-1024.png'), 1024, draw_app_icon)
make_rgba_png(os.path.join(icons_dir, 'tray.png'), 32, draw_tray_icon)
print('  · source-1024.png and tray.png generated')
PYEOF

# ── Resize using sips (macOS built-in) ──────────────────────────────────────
SOURCE="${ICONS_DIR}/source-1024.png"

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
