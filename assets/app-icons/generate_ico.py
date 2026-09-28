"""Build the Windows icon from optically drawn small frames and the Lexift PNG.

Small entries use 32-bit DIBs with an explicit AND mask for Win32 consumers
that do not handle PNG-compressed ICO entries consistently. The 256px entry
stays PNG-compressed. Requires Pillow; this runs only when authoring the asset.
"""

from io import BytesIO
from pathlib import Path
from struct import pack

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parent
SIZES = (16, 24, 32, 48, 64, 128, 256)
SMALL_SIZES = (16, 24, 32)
SUPERSAMPLE = 4
CYAN = "#1bb9e9"


def small_frame(size: int) -> Image.Image:
    """Draw the brand mark with details placed on the small icon's pixel grid."""
    if size not in SMALL_SIZES:
        raise ValueError(f"unsupported small icon size: {size}")
    k = SUPERSAMPLE
    image = Image.new("RGBA", (size * k, size * k), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    # Each size has its own margins; scaling the detailed 256px artwork makes
    # the two rings and speech-bubble tail merge at 16px.
    geometry = {
        16: ((1, 1, 14, 14), 3, (2, 4, 13, 11), 2, [(4, 10), (4, 13), (7, 11)], (6, 10), 2, 1),
        24: ((1, 1, 22, 22), 5, (3, 5, 20, 17), 3, [(6, 16), (6, 20), (10, 17)], (9, 15), 3, 1),
        32: ((1, 1, 30, 30), 7, (4, 7, 27, 23), 4, [(8, 22), (8, 27), (13, 23)], (11, 20), 4, 2),
    }
    background, bg_radius, bubble, bubble_radius, tail, centers, outer, inner = geometry[size]

    def box(rect: tuple[int, int, int, int]) -> tuple[int, int, int, int]:
        return tuple(value * k for value in rect)

    draw.rounded_rectangle(box(background), radius=bg_radius * k, fill=CYAN)
    draw.polygon([(x * k, y * k) for x, y in tail], fill="white")
    draw.rounded_rectangle(box(bubble), radius=bubble_radius * k, fill="white")
    for center_x in centers:
        center_y = size // 2
        draw.ellipse(
            ((center_x - outer) * k, (center_y - outer) * k,
             (center_x + outer) * k, (center_y + outer) * k),
            fill=CYAN,
        )
        draw.ellipse(
            ((center_x - inner) * k, (center_y - inner) * k,
             (center_x + inner) * k, (center_y + inner) * k),
            fill="white",
        )
    return image.resize((size, size), Image.Resampling.LANCZOS)


def dib_entry(image: Image.Image) -> bytes:
    """Encode a bottom-up 32-bit ICO bitmap and its bottom-up 1-bit mask."""
    width, height = image.size
    rgba = image.convert("RGBA")
    bgra = bytearray()
    mask = bytearray()
    mask_stride = ((width + 31) // 32) * 4

    for y in range(height - 1, -1, -1):
        row_mask = bytearray(mask_stride)
        for x in range(width):
            red, green, blue, alpha = rgba.getpixel((x, y))
            # Some classic Win32 icon consumers use only the 1-bit mask.
            # Exclude faint resampling pixels so they cannot turn into dark specks.
            if alpha < 128:
                bgra.extend((0, 0, 0, 0))
                row_mask[x // 8] |= 0x80 >> (x % 8)
            else:
                bgra.extend((blue, green, red, alpha))
        mask.extend(row_mask)

    header = pack("<IiiHHIIiiII", 40, width, height * 2, 1, 32, 0, len(bgra), 0, 0, 0, 0)
    return header + bgra + mask


def build_icon(source: Image.Image) -> bytes:
    entries = []
    for size in SIZES:
        if size == 256:
            data = BytesIO()
            source.save(data, format="PNG")
            payload = data.getvalue()
        else:
            frame = (small_frame(size) if size in SMALL_SIZES
                     else source.resize((size, size), Image.Resampling.LANCZOS))
            payload = dib_entry(frame)
        entries.append((size, payload))

    directory = bytearray(pack("<HHH", 0, 1, len(entries)))
    offset = 6 + 16 * len(entries)
    for size, payload in entries:
        directory.extend(pack("<BBBBHHII", size if size < 256 else 0,
                              size if size < 256 else 0, 0, 0, 1, 32,
                              len(payload), offset))
        offset += len(payload)
    return bytes(directory) + b"".join(payload for _, payload in entries)


if __name__ == "__main__":
    with Image.open(ROOT / "lexift.png") as source_image:
        source = source_image.convert("RGBA")
    if source.size != (256, 256):
        raise ValueError("Lexift icon source must be 256×256")
    (ROOT / "lexift.ico").write_bytes(build_icon(source))
