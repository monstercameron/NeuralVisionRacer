# Splat Racer

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

The repository is one tree, but two kinds of data are kept out of git on purpose:

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
| W / ↑, S / ↓ | Throttle, brake (hold S at a standstill to reverse) |
| A / ←, D / → | Steer |
| Q, E | Paddle shift down, up |
| G | Automatic / manual gearbox |
| T | Flip the car upright where it is |
| F | Chase camera / free orbit camera |
| R | Reset the camera |
| Space | Pause and resume the simulation |
| P | Stop drawing frames (saves power while paused) |
| Left-drag | Rotate the orbit camera |
| Right-drag or Shift-drag | Pan |
| Middle-drag or Option-drag | Height |
| Mouse wheel | Zoom |
| B | Show the level's octree boxes, coloured by detail |
| C | Show the voxel collision around the car |
| H | Show or hide the dashboard |
| Esc (browser) | Pause menu with a level selector |
| L (browser) | Next level |

## Configuration

Everything tunable is in [`engine.conf`](engine.conf), in sections `[engine]`, `[window]`, `[render]`, `[camera]`,
`[splat]`, `[hdri]`, `[physics]`, `[level]`, `[live]`, `[vehicle]`, `[effects]`, `[hud]`, `[game]` and `[console]`.
Keys are commented in place. Most take effect on
restart or `reload`; the debug views and the HUD apply live. The console command `dump` prints the active settings.

Frame pacing (`render.frame_pacing`) chooses `auto`, `60`, `30` or `off`. In `auto`, the renderer draws each frame for
as many display refreshes as the GPU needs, so frame times stay even.

## Tests

Each crate is tested on its own:

```bash
for crate in crates/*; do (cd "$crate" && cargo test -q); done
```

GPU tests check the sorter and the pool bookkeeping and skip themselves when no GPU adapter is available.

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
