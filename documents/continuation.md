# Continuation notes: LOD, flicker, frame pacing (paused 2026-10-24)

Where the work stopped, what is verified, what is not, and what to do next. Everything is uncommitted in
the repository root (no commits this session).

## Getting going

- Web server (must be running for the browser): from `NeuralVisionRacer/`,
  `python3 -I scripts/serve_web.py --port 8800` (from the repository root), then open
  `http://localhost:8800/web/index.html` (`?level=basilica-assumption` etc. picks a level; L cycles).
- Web build: `cd crates/splat-web && cargo build -q --target wasm32-unknown-unknown --release && wasm-bindgen
  --target web --out-dir ../../web/pkg ../../target/wasm32-unknown-unknown/release/splat_web.wasm`
- Native: `cd crates/splat-app && cargo build --release`.
- Tests: `cargo test -q` in each crate. At the end of the session all passed: wgsl 14, render 26, car 10,
  camera 10, hud 11, engine 51, game 49.
- Browser fps is only meaningful with the pane visible (a hidden pane throttles).

**First thing tomorrow:** the web and native builds are current, and the web server was restarted detached
(`nohup python3 -I scripts/serve_web.py --port 8800` from the repository root; log in `/tmp/splat_serve.log`). The game
now starts with the debug views off and the HUD on (B, C and H toggle them). Next: the browser review in "Not
verified / open" item 1, then the remaining near differences (nmbu and Delft at 15–30 m).

## What changed this session (all in engine.conf where it is a setting)

**Floor and car**
- Floor collision is one smoothed heightfield per 16 m tile (`floor_smooth_m`, `floor_step_keep_m`,
  `floor_mesh_m`), not stepped boxes.
- Wheels are a copy of rapier's ray-cast vehicle (`crates/splat-engine/src/wheels.rs`) with a 5-ray footprint and
  skyhook damping. Roll down 30–70 %, pitch 10–30 %; vertical bounce mixed.
- T (console `flip`) rights a flipped car where it is.

**Flicker (the big ones)**
- *Ghost splats.* Released pool slots get NaN centres, and every cull comparison with NaN is false, so stale
  splats kept drawing in front of everything. Now caught by testing the float's exponent bits in
  `depth_keys.wgsl`; GPU test `empty_slots_are_not_drawn` in `crates/splat-render/tests/sort.rs`.
- *Judder.* Physics runs at 60 Hz while rendering ran at whatever rate; the camera moved 0–1.2 m per frame
  unevenly. Rendering is now interpolated between ticks (`Engine::set_render_alpha`, `render_car_pose`,
  `render_camera`).
- LOD switches crossfade by per-splat opacity (`splatFade`, written by the depth-key pass): the new range fades
  in over the old, then the old fades out (`lod_fade_s`). A switch now changes the picture by about 0.1 levels.

**LOD selection** (`crates/splat-render/src/stream.rs`)
- Screen-space error: each leaf takes the coarsest LOD whose splats land at most `lod_error_px` (1.5) apart at
  1080p. `lod_error_px = 0` falls back to distance bands (`lod_distance_m`).
- Centred on the car (`lod_centre = car`); leaves in view first; out-of-view leaves +`behind_lod_bias`, except
  within `lod_any_view_m` (8 m) of the car; coarsening waits `lod_keep_s`; a sticky budget; view hysteresis
  (enter 1.5 half-screens, the same as the splat cull, leave 1.8).
- Leaf distance is to the nearest point of its box (long facades used to read as far away).
- Prefetch of the next finer chunk for leaves ahead; 8 chunks cached.
- Adaptive detail scale (shrinks while the pool is short or crossfades get cut; grows with 15 % spare).
- **Pool size `pool_splats = 0` = automatic:** 60 % of the level's LOD 0 count, between 3M and 8M. Delft gets
  about 6M, the others 3M–4M.
- Spherical-harmonic colour on LOD 0 (`sh_lods = 1`); the device asks for 16384-row textures, so 15 palettes fit;
  leaves that missed one get it when a slot frees.

**Rendering and pacing**
- Depth of field (`[effects] dof*`), focus auto at the nearest in-view LOD 2+ leaf.
- Frame pacing (`render.frame_pacing = auto|60|30|off`, `crates/splat-engine/src/pacing.rs`): frames are shown
  for a steady number of display refreshes picked from the measured GPU time. Browser while driving: before 43 % of
  frames off the median by >6 ms, after 0 %.
- HUD off for now (`hud.enabled = false`); H toggles it live.
- Debug views on by default: B LOD octree boxes, C colliders. Device limits raised (textures 16384, storage
  buffers up to 1 GB).

## Verified, with numbers

Reference check (probe, see below): every frame of a 60-frame drive at 15 m/s compared with the same frame at full
detail; share of pixels differing by more than 24 levels, split by distance (worst frame).

| Level | Pool | Under 15 m | 15–30 m | Whole picture | Notes |
|---|---|---|---|---|---|
| basilica | auto (3.0M) | 0.00 % | 0.03 % | 1.38 % | far differences are under the depth of field |
| nmbu | auto (≈4M) | 0.01 % | 0.41 % | 1.49 % | |
| Delft | auto (≈6M) | 0.10 % | 0.52 % | 1.02 % | was 16.6 % under 15 m with a fixed 3M pool |
| saburi | auto with the 3M floor | 0.00 % | 0.00 % | 0.03 % | with a 2M pool (before the floor) it saturated: 2.17 % at 15–30 m |

The 3M floor changes only saburi's pool: basilica (5.0M at LOD 0, pool 3.0M), nmbu (6.84M, 4.11M) and Delft
(9.99M, 6.0M) already sat above it, so their rows are current; a re-run of basilica and nmbu reproduced them
exactly. What remains within 30 m: nmbu 0.41 % at 15–30 m, Delft 0.10 % under 15 m and 0.52 % at 15–30 m.

Also verified:
- Streaming adds no flicker over a frozen LOD on the same drive (basilica 7.81 % vs 7.84 %; the remainder scales
  with camera speed: zero standing still).
- Pool bookkeeping is consistent every step (`LevelStream::check_pool`).

**Bigger pools close most of what is left** (measured after the session paused):
- nmbu at 5.5M: 0.01 % under 15 m, 0.12 % at 15–30 m (from 0.41 % at its automatic 4.1M); 7M gains nothing more.
- Delft at 8M: 0.00 % under 15 m, 0.08 % at 15–30 m (from 0.10 % and 0.52 % at 6M).

`AUTO_POOL_SHARE` in `stream.rs` is now **0.8** (raised on request), which gives about those sizes. Browser
frame rate in the pane (paced), before and after:

| Level | Share | Resident | GPU per frame | Frame rate |
|---|---|---|---|---|
| nmbu | 0.6 | 3.48M | 44 ms | 25 fps, even |
| nmbu | 0.8 | 4.48M | 94 ms | 20–23 fps, 13–29 % of frames uneven |
| Delft | 0.6 | 5.04M | 127–182 ms | 15–16 fps, uneven |
| Delft | 0.8 | 6.75M | 240–250 ms | 12 fps |

So 0.8 roughly halves the frame rate for a small quality gain, and Delft is unplayable in the browser at either
share. Decision (2026-10-25): keep 0.8 for now. The way to get the frame rate back without losing detail is to make
splats cheaper to draw: sort only those in view, and cap the canvas pixel ratio.

**Wall slide:** `vehicle.hull_friction` is now 0.02 (from 0.1). New `vehicle.wall_slide` (0.6): while the body
touches a wall, the car is turned towards the wall's line and its tyres' sideways grip is eased, so it glances off.
A car rolling into a wall at 30° and 15 m/s now keeps 6.1 m/s along it, against 3.1 before (7.8 without a wall).
Test: `a_car_glancing_off_a_wall_slides_along_it`; `wall_glance_breakdown` (ignored) prints other angles. Head-on
hits and corners (nmbu's spawn line ends in a hedge corner) still stop the car.

**Pause menu:** Esc opens a pause menu in the browser game with the four levels (the one playing is marked) and
Resume. It pauses the simulation only if it was running, and resumes on close; arrow keys move between the
entries. The native app has no menu yet. The level list is `LEVELS` in `web/main.js`.

## Not verified / open

1. **Browser after the latest changes:** rebuild, then drive every level with the LOD boxes on and watch for
   coarse tiles or popping; measure fps. The auto pool raises Delft to about 6M splats, so expect a lower frame
   rate there. Pacing keeps it even, but check it is acceptable.
2. **GPU cost:** about 26 ms per frame at 2.3M resident in the browser pane (25 fps paced on a 120 Hz display).
   Debug views and depth of field cost nothing measurable; it is the splats. Ideas:
   - sort only the splats in view (compact before the radix sort; culled ones are still sorted now);
   - cap the canvas device-pixel ratio (the pane renders at about 1388×2204).
3. **Delft spawn** faces a bollard and the car is blocked at the start; nmbu's spawn line runs into walls within
   about 10 m. Trees and hedges count as walls up to 3 m (visible with C).
4. The probe's fixed camera lines fly through foliage near their ends (the 48 % "flicker" spikes were the camera
   inside a bush, identical with LOD frozen). Use the first 60 frames, or pick new lines.
5. `documents/agents.md` describes LOD by distance bands; update it for screen-space error, the auto pool, pacing and
   the H key.
6. The per-splat debug colouring (`Frame::set_debug_colour`: 1 by LOD, 2 by distance) is only used by the probe;
   it could get a key in the game.

## Tools

**`flicker_probe`** (`crates/splat-render/examples/flicker_probe.rs`)
`cargo run --release --example flicker_probe -- <level dir> <eye xyz> <target xyz> [out dir]`, world coordinates.
Starting camera lines used (from each level's chase camera at spawn):

| Level | eye | target |
|---|---|---|
| nmbu-park-03 | 38.79 -5.88 -88.39 | 38.02 -6.88 -82.43 |
| trompetstraat-delft | -1.26 0.33 -6.03 | 0 -0.67 -0.17 |
| saburi-alley-yazd | 2.53 2.48 -10.93 | -0.66 1.48 -5.85 |
| basilica-assumption | 52.96 2.11 -13.83 | 47.82 1.11 -10.73 |

Env options:
- `REF=1`: compare with full detail.
- `COVER=1`: LOD and distance tint, coarse-in-front share.
- `FREEZE=1`: LOD fixed at the start, a motion-only baseline.
- `FADECHECK=1`: jump, then hold still through the switches.
- `SAVE=a-b`: keep those frames.
- `BUDGET=1`: pool use by distance at frame 30.
- Settings: `ERR_PX`, `CAPACITY`, `LOD_M`, `FADE`, `SPEED`, `FRAMES`, `ANY_VIEW`, `BEHIND`, `FOCUS_AHEAD` (6 m,
  the car), `DOF`.

In zsh, pass a settings string with `env ${=cfg}`; plain `$cfg` does not split it into words.

**`ride_quality`** (`crates/splat-engine/examples`): ride metrics. `RIG=12` holds a fixed speed on a fixed line;
`ANGLE=` turns that line.

**In the browser console** (`window.splat`):
- `stream_stats()`, `stream_near(n)` (nearest leaves: distance, wanted and resident LOD, in view, chunk loaded),
  `stream_switches()`;
- `frame_timings()` (CPU per phase), `pacing()` (refresh, span, GPU ms), `render_eye()`;
- `command('set <key> <value>')` for the live settings (`render.debug_*`, `hud.enabled`, `render.frame_pacing`).

## Lessons that cost time

- The splat shader decodes colours from gamma 2.2 before blending, so debug values come back as their 2.2th
  power: undo it when reading (`value(v)` in the probe).
- WebGPU defaults are small: 8192-row textures and 128 MB storage bindings. Frame::new now requests more when the
  adapter has it.
- A stream-side "in view" test is not proof of what is on screen. The rendered checks (tint, distance, reference)
  are what settled every question here.
