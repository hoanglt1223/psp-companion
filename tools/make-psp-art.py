"""Draws the XMB art for the EBOOT and the save: psp/ICON0.PNG (144x80) and psp/PIC1.PNG (480x272).

Original geometric art (a handheld and a monitor joined by Wi-Fi arcs), no fonts or third-party images,
so the output is reproducible: `python tools/make-psp-art.py` (needs Pillow).
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw

ACCENT = (64, 192, 240)
ACCENT_DARK = (24, 96, 150)
LIGHT = (235, 240, 245)
BG_TOP = (18, 30, 48)
BG_BOTTOM = (40, 60, 90)

SS = 4  # supersampling factor for smooth edges


def gradient(w, h, top, bottom, alpha=255):
    img = Image.new("RGBA", (w, h))
    px = img.load()
    for y in range(h):
        t = y / max(1, h - 1)
        c = tuple(round(a + (b - a) * t) for a, b in zip(top, bottom))
        for x in range(w):
            px[x, y] = (*c, alpha)
    return img


def handheld(d, cx, cy, w, color, screen):
    """A wide handheld: rounded body, screen, D-pad cross and four face dots."""
    h = w * 0.42
    x0, y0 = cx - w / 2, cy - h / 2
    d.rounded_rectangle((x0, y0, x0 + w, y0 + h), radius=h / 2, fill=color)
    sw, sh = w * 0.5, h * 0.62
    d.rectangle((cx - sw / 2, cy - sh / 2, cx + sw / 2, cy + sh / 2), fill=screen)
    # D-pad
    px, unit = x0 + w * 0.12, w * 0.028
    d.rectangle((px - unit, cy - unit * 3, px + unit, cy + unit * 3), fill=screen)
    d.rectangle((px - unit * 3, cy - unit, px + unit * 3, cy + unit), fill=screen)
    # Face buttons
    bx, r = x0 + w * 0.88, w * 0.022
    for dx, dy in ((0, -2.6), (0, 2.6), (-2.6, 0), (2.6, 0)):
        x, y = bx + dx * r, cy + dy * r
        d.ellipse((x - r, y - r, x + r, y + r), fill=screen)
    return screen, (cx - sw / 2, cy - sh / 2, cx + sw / 2, cy + sh / 2)


def monitor(d, cx, cy, w, color, screen):
    h = w * 0.62
    x0, y0 = cx - w / 2, cy - h / 2
    d.rounded_rectangle((x0, y0, x0 + w, y0 + h), radius=w * 0.05, fill=color)
    m = w * 0.06
    d.rectangle((x0 + m, y0 + m, x0 + w - m, y0 + h - m * 1.6), fill=screen)
    d.polygon(
        [(cx - w * 0.08, y0 + h), (cx + w * 0.08, y0 + h), (cx + w * 0.14, y0 + h + w * 0.16), (cx - w * 0.14, y0 + h + w * 0.16)],
        fill=color,
    )
    d.rounded_rectangle((cx - w * 0.26, y0 + h + w * 0.14, cx + w * 0.26, y0 + h + w * 0.2), radius=w * 0.02, fill=color)


def arcs(d, cx, cy, r0, step, n, width, color):
    for i in range(n):
        r = r0 + i * step
        d.arc((cx - r, cy - r, cx + r, cy + r), start=-140, end=-40, fill=color, width=round(width))
    d.ellipse((cx - width, cy - width, cx + width, cy + width), fill=color)


def scene(w, h, transparent):
    W, H = w * SS, h * SS
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0)) if transparent else gradient(W, H, BG_TOP, BG_BOTTOM)
    d = ImageDraw.Draw(img)
    unit = min(W / 144, H / 80)
    # Handheld bottom-left, monitor right, arcs between them.
    handheld(d, W * 0.27, H * 0.70, 64 * unit, LIGHT, ACCENT_DARK)
    monitor(d, W * 0.79, H * 0.38, 42 * unit, LIGHT, ACCENT_DARK)
    arcs(d, W * 0.535, H * 0.50, 8 * unit, 6.5 * unit, 3, 2.6 * unit, ACCENT)
    return img.resize((w, h), Image.LANCZOS)


def main():
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent / "psp"
    # ICON0 is drawn over the XMB wave, so it keeps a dark plate for contrast.
    icon = gradient(144, 80, BG_TOP, BG_BOTTOM)
    icon.alpha_composite(scene(144, 80, transparent=True))
    icon.save(out / "ICON0.PNG", optimize=True)
    pic = scene(480, 272, transparent=False).convert("RGB")
    pic.save(out / "PIC1.PNG", optimize=True)
    print(f"wrote {out / 'ICON0.PNG'} and {out / 'PIC1.PNG'}")


if __name__ == "__main__":
    main()
