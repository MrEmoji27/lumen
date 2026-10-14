"""Draws the Lumen icon: a warm disc of light, eclipsed, on a graphite tile.

Run: python assets/make_icon.py  ->  assets/lumen.ico and assets/lumen.png
Drawn at 1024 px, downsampled for each icon size, so edges stay clean.
"""
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

S = 1024
OUT = Path(__file__).parent


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def vertical(size, top, bottom):
    img = Image.new("RGB", (size, size))
    d = ImageDraw.Draw(img)
    for y in range(size):
        d.line([(0, y), (size, y)], fill=lerp(top, bottom, y / (size - 1)))
    return img


def radial(size, center, radius, inner, outer):
    """Radial gradient, inner color at center fading to outer at radius."""
    img = Image.new("RGB", (size, size), outer)
    d = ImageDraw.Draw(img)
    steps = 160
    for i in range(steps, 0, -1):
        t = i / steps
        r = radius * t
        cx, cy = center
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=lerp(inner, outer, t ** 1.4))
    return img


def tile_mask(size, inset, radius):
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).rounded_rectangle([inset, inset, size - inset, size - inset], radius, fill=255)
    return m


def circle(size, cx, cy, r):
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).ellipse([cx - r, cy - r, cx + r, cy + r], fill=255)
    return m


def draw():
    inset, rad = 64, 224
    tile = vertical(S, (44, 44, 50), (22, 22, 26))
    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    canvas.paste(tile, (0, 0), tile_mask(S, inset, rad))

    # Hairline top highlight on the tile edge, so it reads on dark taskbars.
    edge = Image.new("L", (S, S), 0)
    ImageDraw.Draw(edge).rounded_rectangle([inset, inset, S - inset, S - inset], rad, outline=255, width=6)
    fade = vertical(S, (90, 90, 90), (0, 0, 0)).convert("L")
    edge = ImageChops.multiply(edge, fade)
    canvas.paste((255, 255, 255, 255), (0, 0), edge.point(lambda v: v * 0.35))

    # The light: a warm disc, slightly above center.
    cx, cy, r = S * 0.5, S * 0.48, S * 0.255
    disc = radial(S, (cx - r * 0.35, cy - r * 0.4), r * 1.6, (255, 236, 200), (236, 128, 44))

    # The eclipse: a tile-colored circle bites the lower left. Thick enough to survive 16 px.
    bite = circle(S, cx - r * 0.62, cy + r * 0.58, r * 0.78)

    # Soft glow around the disc. Kept out of the bite, so the cut stays clean.
    glow = circle(S, cx, cy, r * 1.12).filter(ImageFilter.GaussianBlur(48))
    glow = ImageChops.subtract(glow, bite.filter(ImageFilter.GaussianBlur(10)))
    canvas.paste((240, 140, 60, 255), (0, 0), glow.point(lambda v: v * 0.42))
    light = ImageChops.subtract(circle(S, cx, cy, r), bite)
    canvas.paste(disc, (0, 0), light)
    return canvas


def main():
    big = draw()
    big.save(OUT / "lumen.png")
    sizes = [256, 128, 64, 48, 32, 24, 16]
    big.save(OUT / "lumen.ico", sizes=[(s, s) for s in sizes])
    print("wrote", OUT / "lumen.ico", OUT / "lumen.png")


if __name__ == "__main__":
    main()
