"""Write a synthetic car-like Gaussian PLY for viewer smoke tests.

Standard 3DGS PLY layout (binary little-endian, SH degree 0):
x y z nx ny nz f_dc_0..2 opacity scale_0..2 rot_0..3
opacity is stored as a logit, scale as log(metres), rot as w-first quaternion.
"""

import sys
from pathlib import Path

import numpy as np

C0 = 0.28209479177387814
rng = np.random.default_rng(0)


def ellipsoid_samples(n, centre, radii, colour):
    # Uniform-ish points on an ellipsoid shell.
    v = rng.normal(size=(n, 3))
    v /= np.linalg.norm(v, axis=1, keepdims=True)
    pts = v * np.array(radii) + np.array(centre)
    cols = np.tile(colour, (n, 1)) + rng.normal(scale=0.03, size=(n, 3))
    return pts, np.clip(cols, 0, 1)


def wheel_samples(n, centre):
    # Disc in the y-z plane, thin along x.
    theta = rng.uniform(0, 2 * np.pi, n)
    r = np.sqrt(rng.uniform(0, 1, n)) * 0.35
    pts = np.stack(
        [rng.normal(scale=0.04, size=n), r * np.cos(theta), r * np.sin(theta)], axis=1
    )
    pts += np.array(centre)
    cols = np.tile([0.08, 0.08, 0.08], (n, 1))
    return pts, cols


def build():
    parts = [
        ellipsoid_samples(9000, [0, 0.55, 0], [2.0, 0.45, 0.9], [0.8, 0.1, 0.1]),
        ellipsoid_samples(3000, [0, 0.95, 0], [1.0, 0.35, 0.8], [0.15, 0.15, 0.2]),
    ]
    for x, z in [(1.2, 0.8), (1.2, -0.8), (-1.2, 0.8), (-1.2, -0.8)]:
        parts.append(wheel_samples(800, [x, 0.3, z]))
    pts = np.concatenate([p[0] for p in parts])
    cols = np.concatenate([p[1] for p in parts])
    n = len(pts)

    scale = np.log(np.full((n, 3), 0.06) + rng.uniform(0, 0.03, (n, 3)))
    rot = np.tile([1.0, 0.0, 0.0, 0.0], (n, 1))
    opacity = np.full((n, 1), np.log(0.8 / (1 - 0.8)))
    f_dc = (cols - 0.5) / C0
    normals = np.zeros((n, 3))
    return pts, normals, f_dc, opacity, scale, rot


def write_ply(path, data):
    pts, normals, f_dc, opacity, scale, rot = data
    n = len(pts)
    props = (
        ["x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity"]
        + [f"scale_{i}" for i in range(3)]
        + [f"rot_{i}" for i in range(4)]
    )
    table = np.concatenate([pts, normals, f_dc, opacity, scale, rot], axis=1).astype("<f4")
    header = "\n".join(
        ["ply", "format binary_little_endian 1.0", f"element vertex {n}"]
        + [f"property float {p}" for p in props]
        + ["end_header", ""]
    )
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as f:
        f.write(header.encode("ascii"))
        f.write(table.tobytes())
    print(f"wrote {n} gaussians to {path}")


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "data/test_car.ply"
    write_ply(out, build())
