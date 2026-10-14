"""Reproduce the shared nine-slice alpha mask; requires only Python's stdlib.

The image overlays the inner 17px square. Sixteen-pixel slice corners retain
the 8px outer falloff plus the blur's inner corner transition when stretched.
"""

import math
from pathlib import Path
import struct
import zlib


def generate_mask() -> bytes:
    """Return a 33px RGBA Gaussian mask with transparent outermost pixels."""
    size, sigma = 33, 2.5

    def integral(position: float) -> float:
        denominator = sigma * math.sqrt(2)
        return (
            math.erf((position - 8) / denominator)
            - math.erf((position - 25) / denominator)
        ) / 2

    rows = bytearray()
    for y in range(size):
        rows.append(0)  # PNG scanline: no filtering.
        for x in range(size):
            alpha = round(255 * min(1, 1.5 * integral(x + 0.5) * integral(y + 0.5)))
            if x in (0, size - 1) or y in (0, size - 1):
                alpha = 0
            rows.extend((255, 255, 255, alpha))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent
    (root / "crates/lexift-ui/ui/assets/pin-glow-mask.png").write_bytes(generate_mask())
