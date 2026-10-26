# NeuralVisionRacer

**[Play the browser demo](https://monstercameron.github.io/NeuralVisionRacer/web/)** ·
[Asset credits and licences](THIRD_PARTY.md)

The Splat Racer prototype: Rust, WebGPU and real-world Gaussian splat scans. The public demo includes Saburi alley
in Yazd and the Porsche car model. Use recent Chrome or Edge with WebGPU; the first load downloads scene assets.

A racing game that drives through real-world scans. The levels are Gaussian splat scans (millions of coloured,
semi-transparent blobs rendered as a scene), the car is a splat model of a real car, and the road under it is built
from the scan's voxels so the wheels meet real ground.

It runs as a native window (wgpu) and in the browser (WebGPU, via WebAssembly). Both use the same Rust engine.

> **Status: work in progress.** Driving, physics, streaming level-of-detail and the browser build work. Tuning is
> ongoing; the open items are listed in [`documents/continuation.md`](documents/continuation.md).

## What it does

- **Rendering:** a port of PlayCanvas's Gaussian-splat pipeline to Rust and wgpu (`crates/splat-wgsl`,
  `crates/splat-render`). GPU depth sorting, view culling, spherical-harmonic colour, a car shadow on the level, a
  cutout that opens a hole through walls between the camera and the car, and screen effects (speed blur, colour
  fringing, grade, depth of field).
- **Level streaming:** each level is a tree of splat leaves stored at several levels of detail. Detail is chosen by
  screen-space error, centred on the car and weighted towards what is in view, within a GPU pool sized to the
  level. Changes crossfade.
- **Physics:** rapier3d. The car is a ray-cast vehicle with four tyres, a spring-damper suspension and a convex hull
  body. The level's floor and walls are built from its voxels in streamed tiles.
- **Car mechanics:** `crates/splat-car` simulates a 911 GT3 RS from its published specification: engine torque and
  power curves, a seven-speed PDK gearbox with paddles and launch control, traction limits, brakes, aero and
  cooling.
- **Dashboard:** `crates/splat-hud` draws a speedometer, rev counter, boost, temperatures and shift lights from a
  design file, and can be swapped per car.
- **Car clarity:** depth-masked sharpening and local contrast, plus faint splat-tail cleanup. All three are live
  settings; see [`documents/car-clarity.md`](documents/car-clarity.md).
- **Level sunlight:** each local scan has a visually estimated sun in [`specs/level-lighting.json`](specs/level-lighting.json).
  The car gets a view-dependent clearcoat highlight from smoothed Gaussian normals, and its cast shadow uses the
  same sun. Both native and WASM select the profile for the active level, including the browser's `?level=` override.
- **Splat wheels:** continuous wheel rotation from physics, with steering and suspension, unchanged Gaussian scales
  and axle thickness, and updated depth sorting. The current Porsche uses
  fitted wheel selections; see [`documents/wheel-animation.md`](documents/wheel-animation.md) for inspection and limits.
- **Configuration without recompiling:** every tunable is in [`engine.conf`](engine.conf) and most can be changed
  while it runs.
- **Agent console:** the engine can run headless and be driven one command per line over stdin, with JSON replies.
  See [`documents/agents.md`](documents/agents.md).

## Repository layout

| Path | What it is |
| --- | --- |
| `crates/splat-app` | The native window and the `splat-racer` binary (`--headless` runs without a window). |
| `crates/splat-web` | The browser build: a WebAssembly module the page drives. |
| `crates/splat-engine` | Engine services: `engine.conf`, the console and logs, voxel colliders, the physics world, wheels. |
| `crates/splat-render` | The wgpu renderer: pools, sorting, streaming, the frame and its passes. |
| `crates/splat-wgsl` | Gaussian-splat shaders (ported from PlayCanvas) and the WGSL preprocessor. |
| `crates/splat-game` | Pure state machines (practice, race, menus). |
| `crates/splat-car` | Car mechanics and the car specification format. |
| `crates/splat-camera` | The chase camera. |
| `crates/splat-hud` | Dashboard gauges and the `.hud` design format. |
| `crates/splat-ecs` | A small entity-component-system the engine uses for scene entities. |
| `web/` | The browser page: `index.html`, `main.js`, and the generated `pkg/` (not committed). |
| `scripts/` | `serve_web.py` (serves the browser build), `fetch_upstream.py` (fetches the pinned reference sources), `download_supersplat.py`, `make_test_hdri.py`. |
| `tools/spike/` | The early car-splat feasibility viewer (JavaScript). Separate from the game. |
| `third_party/` | Reference source subsets from three PlayCanvas projects, with their MIT licences, and `upstream-lock.json`, which pins them. See below. |
| `docs/` | The GitHub Pages site: the browser build, built by `scripts/build_pages.py`. |
| `documents/` | `agents.md` (console and logs), `state-machines.md`, `continuation.md` (current status and next steps), `research/`. |
| `tools/` | Standalone tools that are not part of the game. |
| `LICENSE`, `THIRD_PARTY.md` | Licence of the project, and notices for everything else it contains. |

Most crates under `crates/` are their own Cargo workspace (an empty `[workspace]` table keeps each one separate), and
they reach each other by relative path. They share one build folder through [`.cargo/config.toml`](.cargo/config.toml).

## Requirements

- Rust, stable (developed with rustc 1.99; the repository does not pin a toolchain).
- For the browser build: the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` **0.2.129**, matching the
  `wasm-bindgen` crate in the lockfile:

  ```bash
  rustup target add wasm32-unknown-unknown
  cargo install wasm-bindgen-cli --version 0.2.129
  ```

- Python 3 (for the web server script).
- A GPU with Vulkan, Metal or DirectX 12 for the native app; a browser with WebGPU (recent Chrome or Edge) for the
  web build.

## Assets and levels (not in git)

The full local asset library stays out of git. The browser demo under `docs/` includes only the CC BY 4.0 car and
Saburi alley scene, with attribution. The other local scenes are not published.

Outside the demo, two kinds of data are kept out of git on purpose:

- **Assets** (the car model and the environment map) live in `assets/`. Git ignores them, and
  [`assets/README.md`](assets/README.md) says where they come from.
- **Levels** (the scene data the game streams) live in `levels/`. Git ignores everything in that folder except its
  README, because the scenes are large and each has its own licence. See [`levels/README.md`](levels/README.md) for the
  list, the licence of each, and which are playable.

Other projects that are not part of the game sit in `other-projects/`, which git also ignores.

```
NeuralVisionRacer/          <- the repository
  crates/                   the game (ten crates)
  web/                      the browser page
  assets/                   car and HDRI (git-ignored)
  levels/                   scene folders (git-ignored; README committed)
  docs/  documents/  scripts/  tools/  third_party/
  engine.conf  README.md  LICENSE  THIRD_PARTY.md
  other-projects/           not part of the game (git-ignored)
```

The default level in `engine.conf` is `levels/nmbu-park-03`. The browser's level menu offers the four playable levels.

## Running the native app

From the repository root, so that `engine.conf` is found:

```bash
cargo run --release --manifest-path crates/splat-app/Cargo.toml
```

Headless, for an agent or a test, with the console on stdin and stdout:

```bash
cargo run --release --manifest-path crates/splat-app/Cargo.toml -- --headless --config engine.conf
```

## Running in the browser

Build the WebAssembly module and generate its JavaScript bindings (from the repository root):

```bash
cd crates/splat-web
cargo build -q --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir ../../web/pkg ../../target/wasm32-unknown-unknown/release/splat_web.wasm
cd ../..
```

Serve it from the repository root:

```bash
python3 scripts/serve_web.py --port 8800
```

Then open <http://localhost:8800/web/index.html>, or add `?level=basilica-assumption` (or another level
name) to the address. The server sends `Cache-Control: no-store`, so a reload always picks up a new build.

## Controls

| Key or mouse | Action |
| --- | --- |
| W / ↑, S / ↓ | Brake opposite motion to a full stop, then drive forward / reverse |
| Left / right Shift (hold) | Rear-wheel handbrake; release to restore grip |
| A / ←, D / → | Steer |
| Q, E | Paddle shift down, up |
| G | Automatic / manual gearbox |
| T | Flip the car upright where it is |
| F | Chase camera / free orbit camera |
| R | Reset the camera |
| Backspace | Reset the car to its spawn (edits text normally in the browser console) |
| Tilde / backtick (browser) | Switch keyboard focus between the console and driving |
| Space | Pause and resume the simulation |
| P | Stop drawing frames (saves power while paused) |
| Left-drag | Rotate the orbit camera |
| Right-drag or Shift-drag | Pan |
| Middle-drag or Option-drag | Height |
| Mouse wheel | Zoom |
| B | Show the level's octree boxes, coloured by detail |
| C | Show the voxel collision around the car |
| H | Show or hide the dashboard |
| Esc (browser) | Pause menu: level selector and live shader/effect toggles |
| L (browser) | Next level |

Collisions recover in place and do not automatically reset the car. Use Backspace to reset or T to right it.
Saved per-level position and heading overrides live in `specs/level-spawns.json`; these apply on startup and reset.

## Configuration

Everything tunable is in [`engine.conf`](engine.conf), in sections `[engine]`, `[window]`, `[render]`, `[camera]`,
`[splat]`, `[hdri]`, `[physics]`, `[level]`, `[live]`, `[vehicle]`, `[effects]`, `[hud]`, `[game]` and `[console]`.
Keys are commented in place. Most take effect on
restart or `reload`; the debug views and the HUD apply live. The console command `dump` prints the active settings.

Car sunlight controls are live: `set effects.car_specular 0` disables the added highlight; `1` restores it.
`effects.car_roughness` adjusts its width (default `0.32`, modified by the level profile). Set
`effects.level_sun false` to use the explicit world direction in `effects.sun` instead. `status.lighting` reports
the resolved direction, source, confidence and strength. Unknown level folders use the explicit fallback.

The five profiles were inspected from source-camera and overhead renders, including height slices to expose baked
ground shadows. The catalog records the visual evidence and inspection cameras. NMBU's lamp/tree shadows and
Madras's cone/barrier shadows give the clearest azimuth cues. Saburi uses a building-corner shadow; Basilica and
Delft have incomplete shadow correspondences, so their provisional profiles use weaker, broader highlights.
Elevations and colors are approximate fits. Catalog edits require rebuilding Rust/WASM because it is embedded.

This is additive clearcoat over the car's captured RGB/SH, with surface normals estimated from each Gaussian's
thinnest axis and smoothed in a small spatial neighbourhood. Round, large and faint splats are excluded. It does
not remove the car's original baked reflections, recover material properties or test local tree/building occlusion
of the sun. Source assets stay unchanged. The color hook preserves Gaussian opacity and depth coverage.

The [shared shadow strategy](documents/car-shadows.md) fits seven occlusion ellipsoids from those reliable car
surfaces. They project a soft shadow along the same sun and suppress occluded added highlights. Ground attenuation
uses linear light and receiver incidence; provisional lighting profiles use weaker, softer shadows. No extra GPU
pass or binding is added. The profiler export includes the fitted proxy and resolved shadow settings.

For a repeatable shadow inspection, use the `render_level` example with `LEVEL_ROT=z`, `SLICE_Y=<ceiling>`,
optionally `SLICE_MIN_Y=<floor>` and `SLICE_RADIUS=0.25` to suppress large reconstruction floaters. These diagnostic
filters only affect the in-memory preview. The lighting catalog uses the default 180-degree Z rotation; the engine
rotates the light by the same change if `level.rotate_z_degrees` is adjusted.

Frame pacing (`render.frame_pacing`) chooses `auto`, `60`, `30` or `off`. In `auto`, the renderer draws each frame for
as many display refreshes as the GPU needs, so frame times stay even.

## Performance

The renderer shares decoded centres, draws indexed Gaussian quads, batches radix-sort dispatches and caches
unchanged uploads, wheel deformation and HUD overlays. Empty collision tiles are cached; disabled post effects
use fast paths. GPU timestamp queries and a two-frame submission limit improve pacing diagnostics and latency.
See [performance notes and the repeatable benchmark](documents/performance.md). The browser console command
`perf` reports CPU phases, GPU timing source, queue latency, pending frames and canvas size. Open **Render profiler**
in the browser for WGSL pass timings and JSON export; see the [profiler guide](documents/render-profiler.md).

## Tests

Each native crate is tested on its own; the browser crate is checked against its WebAssembly target:

```bash
for crate in crates/*; do
  [ "$crate" = crates/splat-web ] || cargo test --locked -q --manifest-path "$crate/Cargo.toml"
done
cargo check --locked --target wasm32-unknown-unknown --manifest-path crates/splat-web/Cargo.toml
```

GPU tests check the sorter and the pool bookkeeping and skip themselves when no GPU adapter is available.

## GitHub Pages demo

`docs/` contains the deployable browser build. GitHub Pages serves `main` → `/docs`. To rebuild it from the local
asset library, run `python3 scripts/build_pages.py`, then commit and push the updated `docs/` files. The public
demo offers only levels present in that build; local development offers the other installed levels too.

## Third-party code

Full notices are in [THIRD_PARTY.md](THIRD_PARTY.md).

- `crates/splat-wgsl` contains shaders ported from PlayCanvas's Gaussian-splat pipeline, MIT licensed.
- `crates/splat-render/assets/blue_noise_32_rgba8.bin` is a blue-noise texture extracted from PlayCanvas's engine,
  MIT licensed (see the note in that folder).
- `third_party/` holds reference source subsets from `playcanvas/engine`, `splat-transform` and `supersplat-viewer`,
  each with its own MIT licence file. `third_party/upstream-lock.json` pins every file to a commit and blob hash;
  `scripts/fetch_upstream.py` downloads or checks them.

## Licence

Splat Racer is licensed under the **Apache License 2.0**; see [LICENSE](LICENSE).

Third-party code and data, and the licences of the assets that are not included, are listed in
[THIRD_PARTY.md](THIRD_PARTY.md). The scene data that the game streams has its own, more restrictive licences; check
[`levels/README.md`](levels/README.md) before publishing anything derived from it.
