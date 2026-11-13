"""Draw the app icons: a pi sign of rounded cells, after Ludolph van Ceulen,
who gave years to its digits.

Run: uv run --with pillow python crates/relay/assets/make_icons.py crates/relay/assets assets
"""

import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw

# The sign's cells by (column, row). Each is CELL units with GAP between.
CELLS = [
    *((x, 0) for x in range(6)),
    *((1, y) for y in range(1, 6)),
    *((4, y) for y in range(1, 6)), (5, 5),
]
CELL, GAP, RADIUS = 100, 20, 16
SPAN = 6 * CELL + 5 * GAP  # the sign is as tall as it's wide


def cells(size: float, scale: float):
    """Yield each cell's box on a `size` canvas, the sign spanning `scale` of it."""
    unit = size * scale / SPAN
    origin = (size - SPAN * unit) / 2
    for x, y in CELLS:
        left, top = origin + x * (CELL + GAP) * unit, origin + y * (CELL + GAP) * unit
        yield left, top, left + CELL * unit, top + CELL * unit, RADIUS * unit


def draw(size: int, plate: float, plate_radius: float, scale: float, bg: tuple, fg: tuple) -> Image.Image:
    """Render the sign at `size` px: a rounded plate covering `plate` of the canvas, cells on it."""
    n = size * 4  # supersample, then shrink for smooth edges
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    inset = n * (1 - plate) / 2
    if bg[3]:
        d.rounded_rectangle([inset, inset, n - inset, n - inset], radius=n * plate_radius, fill=bg)
    for left, top, right, bottom, radius in cells(n, scale):
        d.rounded_rectangle([left, top, right, bottom], radius=radius, fill=fg)
    return img.resize((size, size), Image.LANCZOS)


def svg(size: int, plate: float, plate_radius: float, scale: float) -> str:
    """The home screen icon as SVG, for browsers that take one."""
    inset, side = size * (1 - plate) / 2, size * plate
    rects = [f'<rect x="{inset:g}" y="{inset:g}" width="{side:g}" height="{side:g}" rx="{size * plate_radius:g}" fill="#000000"/>']
    for left, top, right, bottom, radius in cells(size, scale):
        rects.append(
            f'<rect x="{left:.1f}" y="{top:.1f}" width="{right - left:.1f}" height="{bottom - top:.1f}" rx="{radius:.1f}" fill="white"/>'
        )
    body = "\n".join(rects)
    return f'<svg width="{size}" height="{size}" viewBox="0 0 {size} {size}" fill="none" xmlns="http://www.w3.org/2000/svg">\n{body}\n</svg>\n'


def icns(path: Path, plate: float, plate_radius: float, scale: float, bg: tuple, fg: tuple) -> None:
    """Write the Mac app's icon to `path`, each size drawn on its own, as `iconutil` takes them."""
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "Ludolph.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            draw(size, plate, plate_radius, scale, bg, fg).save(iconset / f"icon_{size}x{size}.png")
            draw(size * 2, plate, plate_radius, scale, bg, fg).save(iconset / f"icon_{size}x{size}@2x.png")
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(path)], check=True)


def main() -> None:
    """Write every icon the phone app references into the first folder, and the Mac app's into the second."""
    out, mac = Path(sys.argv[1]), Path(sys.argv[2])
    black, white = (0, 0, 0, 255), (255, 255, 255, 255)
    # Home screen icons: a plate of 80.5 % of the canvas, as macOS app icons have.
    home = (0.805, 0.181, 0.5)
    for size in (192, 512):
        draw(size, *home, black, white).save(out / f"icon-{size}.png")
    (out / "icon.svg").write_text(svg(1024, *home))
    # Maskable: the launcher crops to a shape, so fill the canvas, keep the sign in the safe zone.
    draw(512, 1.0, 0.0, 0.48, black, white).save(out / "icon-maskable.png")
    # Notification badge: Android uses only the alpha channel.
    draw(96, 1.0, 0.0, 0.8, (0, 0, 0, 0), white).save(out / "badge.png")
    # The Mac app's icon: the home screen one, on the plate macOS icons share.
    icns(mac / "Ludolph.icns", *home, black, white)


if __name__ == "__main__":
    main()
