# Agent console and logs

Splat Racer can be driven by a program. Start the engine headless, write one command per line to its standard input, and read one JSON reply per line from its standard output. Read the logs from a file, or ask for them over the console.

## Run it

```bash
cd NeuralVisionRacer
cargo run --release --manifest-path crates/splat-app/Cargo.toml -- --headless --config engine.conf
```

Headless runs the same fixed-step simulation and state machines as the window, without opening one. `screenshot` still works headless when a GPU is present.

For an interactive session, drop `--headless` and the window opens. Console commands work in both modes.

## Replies

Every command gets exactly one line on standard output:

```json
{"ok":true,"cmd":"pause","data":{"mode":"Paused(Practice)","paused":true,"transition":"Practice --UiPause--> Paused(Practice) [applied]"}}
{"ok":false,"cmd":"reset","error":"rejected: Paused; Paused(Practice) --Reset--> Paused(Practice) [rejected: Paused]"}
```

- `ok` is `true` or `false`. Check it before reading `data`.
- `cmd` is the verb that ran. A line that could not be parsed has `cmd` set to `parse`.
- Game-state commands report the transition they caused in `transition`, in the form `from --event--> to [outcome]`. Outcomes are `applied`, `unchanged`, `ignored`, or `rejected: <reason>`.
- Nothing else goes to standard output. Logs go to the log file and to `logs`.

## Commands

| Command | What it does |
| --- | --- |
| `help` | List the commands. |
| `status` | Tick, pause state, game mode, asset stages, vehicle state, pose and speed. |
| `get <section.key>` | Read one setting. |
| `set <section.key> <value>` | Change one setting. Most settings apply on `reload`; `engine.log_level` applies at once. |
| `dump` | The active configuration as `engine.conf` text. |
| `reload` | Re-read `engine.conf`, rebuild the world, reload the environment. Refused in a race or while paused. |
| `logs [count] [level]` | The newest records at or above `level` (default: all levels). |
| `pause` / `resume` | Stop or start the simulation. |
| `step [n]` | Advance `n` ticks. Only while paused. |
| `reset` | Put the car back at `vehicle.spawn`. Refused while counting down or paused. |
| `wreck` | Development only: wreck the car. It respawns after 2 seconds. |
| `mode practice` / `mode race` / `mode menu` | Request a game mode. `race` needs the menu first and has no track yet. |
| `camera` | Show the active camera: `mode` (`follow` or `orbit`), eye, target, distance, and the chase camera's pan/height `offset`. |
| `camera reset` | Chase camera: back behind the car. Free camera: frame the level again. |
| `camera follow`, `camera free` | Switch to the chase camera, or to the free orbit camera (it starts where the chase camera was). |
| `camera set ex ey ez tx ty tz` | Place the eye and the target (switches to the free camera). |
| `camera orbit dx dy`, `camera pan dx dy`, `camera zoom steps` | Rotate, pan along the ground, zoom; deltas are mouse pixels. |
| `camera raise dy` | Height: negative raises the camera, like dragging up with the middle button. |

| `drive <throttle> <brake> <steer>` | Hold the pedals (0..1) and steering (-1 left .. 1 right) until changed. Brake held at a standstill selects reverse. |
| `shift up\|down`, `gearbox auto\|manual` | Paddle shift; PDK auto or manual. |
| `car` | The car's spec (layout, engine, gearbox, mass) and live telemetry: rpm, gear, speed, torque, power, drive force, downforce, launch, wheelspin. `status` carries the telemetry too, under `car`. |

Driving keys in the window and the page: **W/↑** throttle, **S/↓** brake (reverse when held at a standstill), **A/D** or
**←/→** steer, **Q/E** paddles, **G** auto/manual, **T** rights a flipped car where it is (console `flip`).

The car's mechanics live in `crates/splat-car` (spec file `specs/porsche-992-gt3rs.car`, sources in its header).
`cargo run --example accel` in that crate prints 0-100, 0-200, the quarter mile, top speed and every shift.

The window and the page also take **P** to stop rendering (the page has a ⏸ button too): no frames, no ticks, the
simulation paused, so the machine goes idle. P again resumes. While stopped the window still reads console lines.

Levels stream by level of detail (`[level] stream = true`): the level's tree of leaves is re-evaluated each frame;
every leaf starts at its coarsest LOD and the nearest are refined first (LOD 0 within `lod_distance_m`, each coarser
LOD doubling the distance) while they fit `pool_splats` on the GPU. Chunk folders are fetched only when a leaf needs
them (a loader thread natively, `fetch` in the page) and dropped once uploaded, beyond a small cache. `status` shows
`collision_tiles`; the page exposes `splatStream()` (resident splats, leaves per LOD, chunks in memory).
Detail is measured from the car (`lod_centre = car`), so it holds still as the chase camera swings round; leaves in
the camera's view get the budget first, far leaves out of view drop `behind_lod_bias` levels, and a leaf keeps its
detail `lod_keep_s` before going coarser. When the pool cannot hold every target, the LOD distance shrinks until it
can (`detail_scale` in `splat.stream_stats()`), so leaves stop trading detail. A change of LOD crossfades over
`lod_fade_s`: the new range fades in (its opacity, per splat, written by the depth-key pass) over the old one, which
then fades out. Chunks for the next finer LOD of leaves ahead are prefetched and kept, so tiles are sharp before the
car reaches them. `stream_stats()` also counts `switches` and `near_switches` (LOD changes so far); in the page,
`splat.stream_near(n)` lists the nearest leaves (distance, wanted and resident LOD, in view, chunk loaded) and
`splat.stream_switches()` the latest LOD changes.

Debug views (on by default, for tuning): **B** draws the level's octree leaves coloured by the LOD they show (0
green, 1 yellow, 2 orange, 3 red; bright while loading, dim out of view; `render.debug_lod_boxes`), **C** the voxel
collision around the car (floor heightfield grid cyan, wall boxes magenta; `render.debug_colliders`). Both are live
settings, so `set render.debug_colliders false` works from the console.

Depth of field (`[effects] dof*`) keeps the car and the road ahead sharp and softens the distance; with
`dof_focus_m = 0` it starts half way through the LOD 1 band, so coarser tiles are always soft. Rendering is
interpolated between physics ticks (`Engine::set_render_alpha`), so motion is smooth at any frame rate. To measure
flicker, `cargo run --release --example flicker_probe -- <level> <eye xyz> <target xyz>` (in `crates/splat-render`)
drives a camera forward and reports the share of pixels that pop and pop back each frame. `FREEZE=1` holds the
level of detail chosen at the start (a motion-only baseline: streaming should add nothing on top of it),
`FADECHECK=1` jumps the camera and holds it still while the switches play out (each shot's change from the last
should be tiny), and `SAVE=a-b` with an out dir keeps those frames. `LevelStream::check_pool` verifies the pool's
bookkeeping. View-dependent colour (SH) goes to LOD 0 (`sh_lods = 1`): the pool's centroid texture holds 15 chunk
palettes on GPUs with 16384-texel textures (7 at WebGPU's default 8192), and a leaf that missed one gets it when a
slot frees.

Collision streams too: the voxel octree's floor heights (10 cm cells) and wall columns stay in memory, and colliders
are built only in 16 m tiles within 32 m of the car (`crates/splat-engine/src/collision.rs`). Each tile's floor is
smoothed with an edge-preserving filter (`floor_smooth_m`, keeping steps over `floor_step_keep_m`) and becomes one
heightfield with a point every `floor_mesh_m`. The wheels (`crates/splat-engine/src/wheels.rs`, rapier's ray-cast
vehicle adapted) feel the road through a five-ray footprint and damp the chassis's own motion, so the faceted surface
does not kick them; `cargo run --release --example ride_quality -- <car.sog> <level>` measures the ride (`RIG=12` holds
a fixed speed along a fixed line, `ANGLE=` turns that line).

The dashboard comes from `crates/splat-hud`: `[hud] design` names a built-in design (`porsche-992-gt3rs`,
`turbo-sport`) or a `.hud` file (desktop). Design files live in `crates/splat-hud/designs/`; their header comments
describe the format (dials, bars, digital readouts, gear, shift lights, warning lamps, panels). In the page,
`splat.set_hud(name or file text)` switches design live.

Levels: `[level] path` names a streamed level folder. Its `lod-meta.json` chunks are drawn at the level of detail from
`lod` (`auto` = finest within `max_splats`), turned upright by `rotate_z_degrees`, and its voxels give the floor and
the wall boxes (`walls`, `wall_height_m`). The car is scaled to `[vehicle] length_m`.
| `hdri procedural` or `hdri <path>` | Choose the environment. Takes effect at once. |
| `screenshot <path.png>` | Render the current frame to an sRGB PNG. |
| `quit` | Stop the engine. |

## Logs

The log file is set by `engine.log_file` (default `logs/engine.log`). Each line is one JSON object:

```json
{"seq":14,"tick":0,"level":"info","module":"vehicle","msg":"Parked --Release--> Airborne [applied]"}
```

- `seq` increases by one per record, so you can tell whether you missed any.
- `tick` is the simulation tick the record was written at.
- `level` is `trace`, `debug`, `info`, `warn` or `error`. `engine.log_level` sets the minimum.
- `module` is `app`, `splat`, `game`, `vehicle`, `asset`, `physics`, `console`, `renderer` or `config`.
- `msg` is text. A state-machine record reads `from --event--> to [outcome]`.

Machine records follow these rules:

- A transition that changes the state is `info`.
- A refused command is `warn`, so every refusal shows up.
- Ignored events and no-op steps are `debug`. Per-tick vehicle samples are not logged unless they change the state.

Read the newest records with `logs 20 info`, or read the file with any tool. The file is created fresh at each start.

## Example session

```
status
pause
step 240
status
resume
wreck
pause
step 200
status
resume
logs 10 info
quit
```

`step` needs the game paused, so pause first. A `step` while running is refused with `rejected: NotPaused`, and the reply says so.

## Vehicle and game states

Game modes: `Boot`, `Loading`, `Menu`, `Practice`, `Race(Countdown|Running|Finished)`, `Paused(<where it came from>)`, `Results`, `Error(...)`.

Vehicle states: `Unloaded`, `Loading`, `Parked`, `Driving(n)` (n counts samples without contact), `Airborne`, `Crashed(n)`, `Wrecked`, `Respawning(n)`. The full tables are in `docs/state-machines.md`.
