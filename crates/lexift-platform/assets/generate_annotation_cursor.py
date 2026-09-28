"""Regenerate cursor RGBA rasters from the SVG's 32-unit geometry.

Requires Pillow. The output is raw top-down RGBA for CreateDIBSection; this script is
an asset-authoring tool and is not run during a user build.
"""

from pathlib import Path

from PIL import Image, ImageDraw


def raster(size: int) -> bytes:
    supersample = 8
    unit = size * supersample / 32
    xy = lambda point: tuple(round(value * unit) for value in point)
    image = Image.new("RGBA", (size * supersample, size * supersample))
    draw = ImageDraw.Draw(image)
    pointer = [(2, 2), (19.6, 10.9), (11, 13.4), (7.6, 20.8)]
    draw.polygon([xy(p) for p in pointer], fill="#000")
    draw.line([xy(p) for p in pointer + [pointer[0]]], fill="#fff", width=round(1.5 * unit), joint="curve")

    # Match the SVG's detached, round-capped cubic arc without an arrowhead.
    controls = [(15.6, 25.6), (16.8, 20.2), (20.6, 16.5), (24.3, 17.8)]
    curve = [
        tuple(
            (1 - t) ** 3 * controls[0][axis]
            + 3 * (1 - t) ** 2 * t * controls[1][axis]
            + 3 * (1 - t) * t**2 * controls[2][axis]
            + t**3 * controls[3][axis]
            for axis in (0, 1)
        )
        for t in (step / 24 for step in range(25))
    ]
    for color, width in (("#fff", 4.7), ("#000", 2.8)):
        draw.line([xy(p) for p in curve], fill=color, width=round(width * unit), joint="curve")
        radius = width * unit / 2
        for endpoint in (curve[0], curve[-1]):
            cx, cy = xy(endpoint)
            draw.ellipse((cx - radius, cy - radius, cx + radius, cy + radius), fill=color)
    image = image.resize((size, size), Image.Resampling.LANCZOS)
    return image.tobytes()


for dimension in (32, 48, 64):
    Path(__file__).with_name(f"annotation-corner-radius-{dimension}.rgba").write_bytes(raster(dimension))
