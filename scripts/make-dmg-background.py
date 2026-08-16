#!/usr/bin/env python3
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""Generate the DMG installer background (tray/src-tauri/dmg/background.png).

Matches the setup wizard's lilac theme (ui/app/globals.css :root /
html[data-theme="lilac"]: --t-panel, --desk, --color-state-proposal) and
Tauri's default DMG icon layout (tray/src-tauri/tauri.conf.json's
bundle.macOS.dmg: windowSize 660x400, appPosition (180,170),
applicationFolderPosition (480,170), icon size 128pt) — Finder draws the
real .app and Applications-alias icons on top of this at build time, so the
background only needs the decoration between them.

Renders at 2x (1320x800 px) with 144 dpi metadata so Finder displays it
crisply at the 660x400-point window size.

Requires Pillow (`pip install Pillow`); not part of the app's runtime deps.

Usage: python3 scripts/make-dmg-background.py
"""

# Digest of the committed tray/src-tauri/dmg/background.png this script produces.
# Pinned by ui/__tests__/dmg-install-steps.test.ts so a copy edit here that is
# never re-rendered cannot ship the old picture with a diff that looks complete.
# Re-run this script and update the line below in the SAME commit.
# rendered-digest: 99d73a6d8ebc92285095ba79d25001c75c262536f52026ff16ecfd84580d8248
import math
import os
from PIL import Image, ImageDraw, ImageFont, ImageOps, ImageFilter

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MARK_PATH = os.path.join(REPO_ROOT, "tray/src-tauri/icons/meridiona-mark.png")
OUT_PATH = os.path.join(REPO_ROOT, "tray/src-tauri/dmg/background.png")

SCALE = 2
W_PT, H_PT = 660, 400
W, H = W_PT * SCALE, H_PT * SCALE

# ── lilac theme tokens (ui/app/globals.css) ──────────────────────────────────
T_MUTED = (110, 106, 136)        # --t-muted
ACCENT = (139, 92, 246)          # --color-state-proposal
ARROW = (196, 181, 253)          # --chip gradient stop (lighter violet tint)
DESK_1 = (236, 234, 255)         # --desk stop 1
DESK_2 = (231, 236, 252)         # --desk stop 2
DESK_3 = (233, 234, 245)         # --desk stop 3

# macOS-only system fonts — this script only runs on macOS (it's a local dev
# tool, not part of CI or release tooling; see the module docstring).
SFNS = "/System/Library/Fonts/SFNS.ttf"
SFNS_MONO = "/System/Library/Fonts/SFNSMono.ttf"
for _font_path in (SFNS, SFNS_MONO):
    if not os.path.exists(_font_path):
        raise SystemExit(
            f"make-dmg-background.py requires the macOS system font {_font_path!r} "
            "— this script only runs on macOS."
        )


def pt(x, y):
    return (x * SCALE, y * SCALE)


def font(path, size_pt):
    return ImageFont.truetype(path, size_pt * SCALE)


def radial_l(size):
    """255 (white) at the center fading to 0 (black) at the edge — the
    opposite of Pillow's built-in `Image.radial_gradient`, which is 0 at the
    center and 255 at the corners."""
    return ImageOps.invert(Image.radial_gradient("L")).resize(size, Image.LANCZOS)


def make_gradient():
    """Soft radial "desk" gradient, off-center toward top — mirrors the
    app's own --desk token (`radial-gradient(1100px 760px at 18% -12%, ...)`)."""
    g = radial_l((W * 2, H * 2))
    cx, cy = int(W * 2 * 0.30), int(H * 2 * 0.18)
    left, top = cx - W // 2, cy - H // 2
    g = g.crop((left, top, left + W, top + H))
    return ImageOps.colorize(g, black=DESK_3, mid=DESK_2, white=DESK_1).convert("RGBA")


def bezier(t, p0, c1, c2, p3):
    x = (1 - t) ** 3 * p0[0] + 3 * (1 - t) ** 2 * t * c1[0] + 3 * (1 - t) * t ** 2 * c2[0] + t ** 3 * p3[0]
    y = (1 - t) ** 3 * p0[1] + 3 * (1 - t) ** 2 * t * c1[1] + 3 * (1 - t) * t ** 2 * c2[1] + t ** 3 * p3[1]
    return (x, y)


def main():
    bg = make_gradient()
    draw = ImageDraw.Draw(bg, "RGBA")

    # ── Brand mark + wordmark, top center ────────────────────────────────────
    # The source PNG has a lot of asymmetric transparent padding around the
    # glyph (~17% top / ~23% bottom margin), so pasting it as-is both shrinks
    # the visible mark and shifts it off-center within its own box. Crop to
    # the actual alpha bounding box first, with a little even breathing room,
    # so the mark is properly sized and truly centered above the wordmark.
    mark_src = Image.open(MARK_PATH).convert("RGBA")
    content_bbox = mark_src.getchannel("A").getbbox()
    cw, ch = content_bbox[2] - content_bbox[0], content_bbox[3] - content_bbox[1]
    side = max(cw, ch)
    ccx, ccy = (content_bbox[0] + content_bbox[2]) // 2, (content_bbox[1] + content_bbox[3]) // 2
    pad = int(side * 0.06)
    half = side // 2 + pad
    mark = mark_src.crop((ccx - half, ccy - half, ccx + half, ccy + half))

    mark_size = 46 * SCALE
    mark = mark.resize((mark_size, mark_size), Image.LANCZOS)
    mark_x = W // 2 - mark_size // 2
    mark_y = 26 * SCALE
    bg.paste(mark, (mark_x, mark_y), mark)

    kicker_font = font(SFNS_MONO, 11)
    kicker_text = "M E R I D I A N"  # manual tracking — matches the wizard's Kicker style
    bbox = draw.textbbox((0, 0), kicker_text, font=kicker_font)
    tw = bbox[2] - bbox[0]
    draw.text((W // 2 - tw // 2, mark_y + mark_size + 10 * SCALE), kicker_text, font=kicker_font, fill=T_MUTED)

    # ── Curved arrow: app icon (180,170) → Applications alias (480,170) ─────
    # Icon size 128pt → radius 64pt; start/end just outside each icon's edge
    # so the arrow floats in the gap rather than overlapping Finder's icons.
    p0 = pt(180 + 70, 170 - 6)
    p3 = pt(480 - 70, 170 - 6)
    c1 = pt(280, 96)
    c2 = pt(380, 96)
    N = 120
    pts = [bezier(i / N, p0, c1, c2, p3) for i in range(N + 1)]

    # Soft glow stroke under the arrow (depth), then the crisp stroke on top.
    arrow_layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    arrow_draw = ImageDraw.Draw(arrow_layer)
    arrow_draw.line(pts, fill=ARROW + (90,), width=10 * SCALE, joint="curve")
    arrow_layer = arrow_layer.filter(ImageFilter.GaussianBlur(6 * SCALE / 2))
    bg = Image.alpha_composite(bg, arrow_layer)
    draw = ImageDraw.Draw(bg, "RGBA")

    draw.line(pts, fill=ARROW + (235,), width=int(3.4 * SCALE), joint="curve")
    # Round the line caps (ImageDraw.line leaves flat caps at the ends).
    r = int(3.4 * SCALE / 2)
    for cap in (pts[0], pts[-1]):
        draw.ellipse([cap[0] - r, cap[1] - r, cap[0] + r, cap[1] + r], fill=ARROW + (235,))

    # Arrowhead at the end, oriented along the local tangent.
    tx, ty = pts[-1][0] - pts[-6][0], pts[-1][1] - pts[-6][1]
    ang = math.atan2(ty, tx)
    head_len = 15 * SCALE
    head_w = 10 * SCALE
    tip = pts[-1]
    back = (tip[0] - head_len * math.cos(ang), tip[1] - head_len * math.sin(ang))
    left = (back[0] - head_w * math.sin(ang), back[1] + head_w * math.cos(ang))
    right = (back[0] + head_w * math.sin(ang), back[1] - head_w * math.cos(ang))
    draw.polygon([tip, left, right], fill=ARROW + (235,))

    # ── Instructional caption under the icon row ─────────────────────────────
    #
    # TWO STEPS, NUMBERED, because the second one is the one people miss.
    #
    # This used to read "Drag to Applications to install" and stop there — which
    # describes the drag accurately and then leaves the user on a Finder window
    # with nothing telling them the app has not started. A DMG cannot launch
    # anything itself: the drag is a Finder copy, no code of ours runs, and macOS
    # has no post-install hook in a disk image by design. So the app sitting there
    # unopened is not a bug to fix in code — it is a step the installer has to
    # ASK for, and the background is the only surface that can.
    #
    # Numbered rather than one sentence: "and then open it" tacked onto the drag
    # instruction reads as a description of what the drag does, which is exactly
    # the misreading that leaves the app unopened. Two numbers make it two
    # actions, and the second one names where to find it, because Applications
    # is not where the user is looking at that moment.
    step_font = font(SFNS, 12)
    steps = [
        "1.  Drag Meridian to Applications",
        "2.  Open it from Applications to set up",
    ]
    for n, line in enumerate(steps):
        bbox = draw.textbbox((0, 0), line, font=step_font)
        tw = bbox[2] - bbox[0]
        # 252pt clears the 128pt icon row (centred on 170pt, so its bottom edge
        # is 234pt) with room for Finder's own icon labels underneath them.
        draw.text((W // 2 - tw // 2, (252 + n * 17) * SCALE), line, font=step_font, fill=T_MUTED)

    os.makedirs(os.path.dirname(OUT_PATH), exist_ok=True)
    bg.convert("RGB").save(OUT_PATH, dpi=(144, 144))
    print(f"saved {OUT_PATH} {bg.size}")


if __name__ == "__main__":
    main()
