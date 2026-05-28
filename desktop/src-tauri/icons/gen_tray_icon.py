#!/usr/bin/env python3
"""
Generate the macOS menu bar tray icon for AirNote.

Produces a *template image* (pure white silhouette on transparent) that
macOS will auto-tint to match dark/light menu bar appearance.

Glyph: two rounded "quotation mark" blobs above a soft voice-wave —
       same concept as the in-app brand mark.

Output:
    tray.png       (22 x 22, 1x)
    tray@2x.png    (44 x 44, 2x retina)
"""

from PIL import Image, ImageDraw
import math, os

OUT_DIR = os.path.dirname(os.path.abspath(__file__))


def draw_mark(size: int) -> Image.Image:
    """Render the AirNote mark at the given square pixel size."""
    img   = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw  = ImageDraw.Draw(img)
    s     = size / 22.0  # all coords below in 22-unit space

    # ── Two quotation-mark blobs (top half) ─────────────────────────────
    def quote(cx: float, cy: float):
        # outer soft blob
        r = 2.4
        draw.rounded_rectangle(
            [(cx - r) * s, (cy - r) * s, (cx + r) * s, (cy + r) * s],
            radius=int(2 * s), fill=(255, 255, 255, 255),
        )
        # inner cut-out (gives the curl shape)
        cut = 1.2
        draw.rounded_rectangle(
            [
                (cx - cut + 0.4) * s,
                (cy - cut - 0.6) * s,
                (cx + cut + 0.4) * s,
                (cy + cut - 0.6) * s,
            ],
            radius=int(1.0 * s), fill=(0, 0, 0, 0),
        )

    quote(cx=7.2,  cy=7.5)
    quote(cx=14.8, cy=7.5)

    # ── Voice wave underneath ───────────────────────────────────────────
    # Three bumps (sin-style) drawn as a thick rounded polyline.
    pts   = []
    y_mid = 14.5
    amp   = 1.6
    for i in range(0, 41):           # 41 samples → smooth curve
        x = 3.0 + (i / 40.0) * 16.0
        y = y_mid - math.sin((i / 40.0) * math.pi * 3) * amp
        pts.append((x * s, y * s))

    # Draw as overlapping circles for smooth thick stroke + rounded ends.
    stroke_w = max(1, int(round(1.6 * s)))
    for x, y in pts:
        r = stroke_w / 2.0
        draw.ellipse(
            [x - r, y - r, x + r, y + r],
            fill=(255, 255, 255, 255),
        )

    return img


def main():
    for label, size in [("tray.png", 22), ("tray@2x.png", 44)]:
        img = draw_mark(size)
        out = os.path.join(OUT_DIR, label)
        img.save(out, "PNG")
        print(f"wrote {out} ({size}x{size})")


if __name__ == "__main__":
    main()
