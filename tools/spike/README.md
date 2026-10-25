# Car splat spike — local viewer

Gate 0 of the car-splat spike: a local Gaussian splat viewer that loads PLY and SOG on this Mac, before any generation or reconstruction work.

## Layout

- `make_test_ply.py` — writes a synthetic car-shaped Gaussian PLY (15,200 splats, SH degree 0). Test data only, not a real car.
- `data/` — generated files: `test_car.ply` (source) and `test_car.sog` (SplatTransform 3.10.3, CPU mode).
- `vendor/` — pinned npm dependencies: `@sparkjsdev/spark@2.1.0`, `three@0.186.0`. Spark 2.1.0 requires three r179 or later.
- `viewer/index.html` — the viewer. Loads `../data/test_car.ply` by default; accepts `?src=` and a file picker.

## Run

Serve this folder over HTTP (opening the HTML from `file://` will not load modules):

```bash
cd tools/spike && python3 -m http.server 8765 --bind 127.0.0.1
```

Then open `http://127.0.0.1:8765/viewer/index.html`, or `?src=../data/test_car.sog` for the SOG.

## Results (October 24, 2026, this Mac, in-app Chromium browser pane)

| Check | Result |
| --- | --- |
| PLY loads and renders | Pass — 15,200 splats, ~420 ms load |
| SOG v2 loads and renders | Pass — same bounds as the PLY, ~80 ms load |
| Orbit by mouse drag | Pass |
| Reset view button | Not separately tested |
| Frame rate | Pane showed ~100 fps; not measured on a real GPU load |
| Browser support | Only the in-app Chromium pane was tested |

Not yet validated: colour and SH against a PlayCanvas reference frame, real captured data, file picker, Brush-trained output, and performance at real splat counts.

## Notes

- Spark needs its `SparkRenderer` added to the scene before any `SplatMesh`; without it nothing draws.
- `window.splatViewer` is a debug handle for this spike. Remove it before the viewer is reused.
