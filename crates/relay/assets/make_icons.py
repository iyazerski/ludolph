"""Draw the app icons from Zeron's logo (dist/zeron.svg in Zeron, MIT).

Run: uv run --with pillow python relay/assets/make_icons.py relay/assets
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw

# The logo's cells on its 100-unit grid (x, y), from dist/zeron.svg.
CELLS = [
    (0, 600), (0, 720), (240, 840), (240, 720), (120, 840), (120, 600), (240, 600), (0, 480), (0, 360),
    (480, 840), (480, 720), (120, 360), (120, 240), (240, 360), (600, 720), (480, 600), (360, 360),
    (240, 240), (600, 600), (720, 600), (720, 480), (240, 120), (600, 380), (720, 240), (720, 0),
    (480, 240), (480, 0), (120, 480), (240, 480), (360, 840), (360, 720), (360, 600), (360, 480), (120, 720),
]


def draw(size: int, plate: float, plate_radius: float, glyph_scale: float, bg: tuple, fg: tuple) -> Image.Image:
    """Render the logo at `size` px: a rounded plate covering `plate` of the canvas, cells inside."""
    n = size * 4  # supersample, then shrink for smooth edges
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    inset = n * (1 - plate) / 2
    if bg[3]:
        d.rounded_rectangle([inset, inset, n - inset, n - inset], radius=n * plate_radius, fill=bg)
    # The glyph spans 820 x 940 units; center it.
    unit = n * glyph_scale / 940
    ox, oy = (n - 820 * unit) / 2, (n - 940 * unit) / 2
    for x, y in CELLS:
        d.rounded_rectangle(
            [ox + x * unit, oy + y * unit, ox + (x + 100) * unit, oy + (y + 100) * unit],
            radius=16 * unit,
            fill=fg,
        )
    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    """Write every icon the app references into the given folder."""
    out = Path(sys.argv[1])
    black, white = (0, 0, 0, 255), (255, 255, 255, 255)
    # Home screen icons: the logo as Zeron ships it (plate 80.5 % of the canvas).
    for size in (192, 512):
        draw(size, 0.805, 0.181, 0.55, black, white).save(out / f"icon-{size}.png")
    # Maskable: the launcher crops to a shape, so fill the canvas, keep the glyph in the safe zone.
    draw(512, 1.0, 0.0, 0.5, black, white).save(out / "icon-maskable.png")
    # Notification badge: Android uses only the alpha channel.
    draw(96, 1.0, 0.0, 0.8, (0, 0, 0, 0), white).save(out / "badge.png")


if __name__ == "__main__":
    main()
