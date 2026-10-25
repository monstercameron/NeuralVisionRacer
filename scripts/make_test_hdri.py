"""Writes a 2:1 equirectangular Radiance (.hdr) environment for testing the HDRI path.

The panorama is a sky gradient, a dim ground, and one bright sun disc, so tone mapping and the
cubemap orientation can be checked by eye. Values are linear HDR (the sun is far above 1.0).

usage: python3 scripts/make_test_hdri.py assets/hdri/studio_sun.hdr [width]
"""
import math
import struct
import sys


def radiance_encode(r, g, b):
    """Radiance RGBE: shared 8-bit exponent, so values up to about 1e38 survive."""
    m = max(r, g, b)
    if m < 1e-32:
        return bytes((0, 0, 0, 0))
    mantissa, exponent = math.frexp(m)
    scale = mantissa * 256.0 / m
    return bytes((min(255, int(r * scale)), min(255, int(g * scale)), min(255, int(b * scale)), exponent + 128))


def main():
    out = sys.argv[1]
    width = int(sys.argv[2]) if len(sys.argv) > 2 else 512
    height = width // 2
    sun_dir = (0.4, 0.7, 0.5)
    length = math.sqrt(sum(c * c for c in sun_dir))
    sun = tuple(c / length for c in sun_dir)

    rows = []
    for y in range(height):
        polar = math.pi * (y + 0.5) / height  # 0 at the top (zenith)
        elevation = math.pi / 2 - polar
        for x in range(width):
            azimuth = 2 * math.pi * (x + 0.5) / width - math.pi
            d = (math.cos(elevation) * math.sin(azimuth), math.sin(elevation), -math.cos(elevation) * math.cos(azimuth))
            if d[1] >= 0:
                t = math.sqrt(d[1])
                colour = [0.62 + (0.2 - 0.62) * t, 0.74 + (0.36 - 0.74) * t, 0.95 + (0.8 - 0.95) * t]
            else:
                colour = [0.09, 0.085, 0.08]
            cos_sun = max(0.0, sum(a * b for a, b in zip(d, sun)))
            colour = [c + 2.0 * cos_sun ** 128 for c in colour]
            if cos_sun > 0.9993:
                colour = [c + 400.0 for c in colour]  # the disc: many stops over the display range
            rows.append(radiance_encode(*colour))

    header = f"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nEXPOSURE=1.0\n\n-Y {height} +X {width}\n".encode()
    with open(out, "wb") as f:
        f.write(header)
        f.write(b"".join(rows))
    print(f"wrote {out}: {width}x{height}")


if __name__ == "__main__":
    main()
