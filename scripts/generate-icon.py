#!/usr/bin/env python3
"""Generate AirNote app icons — 4-bar waveform on dark rounded-rect background."""
import os, subprocess, struct
from PIL import Image, ImageDraw

ICON_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "desktop", "src-tauri", "icons")

def draw_icon(size: int) -> Image.Image:
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    # Rounded rect background
    r = int(size * 0.223)  # macOS icon radius ratio
    draw.rounded_rectangle([0, 0, size-1, size-1], radius=r, fill=(16, 17, 23, 255))
    # Subtle border
    draw.rounded_rectangle([1, 1, size-2, size-2], radius=r-1, fill=None, outline=(255,255,255,20), width=max(1, size//256))

    # 4 bars: heights relative to 24-unit viewBox, centered
    # Bar definitions: (x, y, w, h) in 24x24 space
    bars = [
        (3, 8.5, 3, 7),
        (8, 4.5, 3, 15),
        (13, 2.5, 3, 19),
        (18, 6.5, 3, 11),
    ]
    # Scale and center
    pad = size * 0.24  # padding around bars
    inner = size - 2 * pad
    scale = inner / 24

    for (bx, by, bw, bh) in bars:
        x1 = pad + bx * scale
        y1 = pad + by * scale
        x2 = x1 + bw * scale
        y2 = y1 + bh * scale
        br = bw * scale * 0.5  # pill radius
        # Gradient: top white → bottom gray
        for row in range(int(y1), int(y2)):
            t = (row - y1) / max(1, y2 - y1)
            c = int(226 - t * 60)
            draw.rounded_rectangle([x1, row, x2, min(row+1.5, y2)], radius=br, fill=(c, c, c+8, 255))

    return img

def make_ico(pngs: dict, path: str):
    """Create a minimal .ico from PNG images."""
    sizes_to_use = [s for s in [16, 32, 48, 256] if s in pngs]
    entries = []
    data_blocks = []
    offset = 6 + 16 * len(sizes_to_use)  # header + entries
    for s in sizes_to_use:
        png_data = open(pngs[s], "rb").read()
        w = s if s < 256 else 0
        h = w
        entries.append(struct.pack("<BBBBHHII", w, h, 0, 0, 1, 32, len(png_data), offset))
        data_blocks.append(png_data)
        offset += len(png_data)
    with open(path, "wb") as f:
        f.write(struct.pack("<HHH", 0, 1, len(sizes_to_use)))
        for e in entries:
            f.write(e)
        for d in data_blocks:
            f.write(d)

def main():
    os.makedirs(ICON_DIR, exist_ok=True)
    pngs = {}

    for size in [16, 32, 64, 128, 256, 512, 1024]:
        img = draw_icon(size)
        path = os.path.join(ICON_DIR, f"icon_{size}x{size}.png" if size not in (512,) else "icon_512x512.png")
        if size == 512:
            path = os.path.join(ICON_DIR, "icon_512x512.png")
        elif size == 1024:
            path = os.path.join(ICON_DIR, "icon_1024x1024.png")
        else:
            path = os.path.join(ICON_DIR, f"icon_{size}x{size}.png")
        img.save(path, "PNG")
        pngs[size] = path
        print(f"  {size}x{size} → {os.path.basename(path)}")

    # icon.png = 512
    img512 = draw_icon(512)
    img512.save(os.path.join(ICON_DIR, "icon.png"), "PNG")

    # .icns via iconutil
    iconset = os.path.join(ICON_DIR, "icon.iconset")
    os.makedirs(iconset, exist_ok=True)
    for base, scale in [(16,1),(16,2),(32,1),(32,2),(128,1),(128,2),(256,1),(256,2),(512,1),(512,2)]:
        actual = base * scale
        img = draw_icon(actual)
        name = f"icon_{base}x{base}" + ("@2x" if scale == 2 else "") + ".png"
        img.save(os.path.join(iconset, name), "PNG")
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(ICON_DIR, "icon.icns")], check=True)
    subprocess.run(["rm", "-rf", iconset])
    print(f"  icns → icon.icns")

    # .ico
    make_ico(pngs, os.path.join(ICON_DIR, "icon.ico"))
    print(f"  ico → icon.ico")

    print("\nDone!")

if __name__ == "__main__":
    main()
