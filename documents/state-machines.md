# Splat Racer — state-machine design

Status: proposal, October 24, 2026. The engine core, console, logger and physics exist (`crates/splat-engine`, `crates/splat-app`). There is no game-mode, vehicle-controller or asset-state code yet. This document fixes that design before Milestone 3 (driving proof).

Labels used throughout:

- **[Fact]** checked against the repo or crates.io today.
- **[Rec]** a recommendation or design decision. Changeable.
- **[Verify]** an API or behaviour that must be checked against the pinned crate before coding.

Tunable constants are placeholders and are not measured.

## Implementation status (October 24, 2026)

Built in `crates/splat-game` (pure, no dependencies) and driven by `crates/splat-engine/src/engine.rs`.

- **Game machine:** every row G1–G34 is implemented and tested, with two additions for stepping: wreck timers and the countdown fire while the step gate is open (rows added beside G30).
- **Vehicle machine:** every row V1–V28 is implemented. Physics samples come from rapier's contact pairs (`Physics::sample`). The impact is the contact impulse over mass, in m/s.
- **Asset machine:** rows A1–A15 are implemented except the GPU upload stage. The renderer uploads the splat, outside the machine, so headless runs can reach `Ready`. Deviation from section 1.5.2.
- **Race:** the machine supports the race rows, but no track asset exists, so `mode race` runs a countdown on the placeholder track `main`.

Decisions taken for the open questions in section 6, so the build has a default. Change them by editing the tables:

1. Wreck policy: a wreck costs no time. The car respawns at the checkpoint (the spawn) after 2 seconds.
2. Gating: `reset` is refused during the countdown and while paused. `reload` is refused in a race and while paused.
3. Start: Practice releases the car at once, so it drops from `vehicle.spawn` as before. `game.start_mode = menu` starts parked.
4. Asset failure: fatal. The game enters `Error(AssetFailed(car.body))`, the console stays up, and `quit` exits.
5. WASM threading: not decided. The asset path is synchronous today.

Test coverage: `cargo test -p splat-game` runs the table rows, the seeded invariant sequences (10 000 of them) and the vehicle counter bounds. `cargo test -p splat-engine` runs the driver end to end on synthetic data.

---

## 0. Starting point (facts from the code)

- **Splat load is synchronous at startup.** `splat-app/src/main.rs` calls `sog::load`, then `sog::centres`, then `Engine::new`, all before the window opens. If the load fails, it logs an error and runs with no vehicle. [Fact]
- **Engine core** (`splat-engine/src/engine.rs`) has no window or GPU. `Engine::tick()` returns early when `paused`, otherwise steps physics and increments `tick`. [Fact]
- **Console** (`console.rs`, `Engine::execute`) returns `(Reply, Effect)`. `Effect` is `{None, Quit, ReloadEnvironment}`, which is already the pattern for work the core cannot do itself. [Fact]
- **Pause is an engine flag.** `pause` sets `Engine.paused` directly. `step n` temporarily clears it, ticks n times, then restores it. [Fact]
- **Modes are a string.** `mode practice|race` sets `game_mode` and logs it. No transition, no guard, no countdown. [Fact]
- **`reset`** calls `Physics::reset_vehicle(vehicle.spawn)` and is not gated by any state. [Fact]
- **`reload`** re-reads `engine.conf` (if one was given) and rebuilds `Physics` from the stored splat centres. [Fact]
- **Physics** (`physics.rs`, rapier3d 0.36.1, f32, dim3): a fixed ground cuboid and one dynamic body whose collider is a voxel set built at runtime from the splat centres (`voxel.rs`). There are no wheels, no raycast vehicle controller, no brakes, no engine force and no contact queries. [Fact]
- **Logger** (`log.rs`): JSON lines with `seq`, `tick`, `level`, `module` and a free-text `msg`, plus a bounded in-memory tail that the console `logs` command reads. [Fact]
- **Config** already has `[game] start_mode = practice|race` and `countdown_seconds = 3`. [Fact]
- The console's `mode` help text points at "the state machine docs", which is this document. [Fact]
- Collider voxelisation for **tracks** is offline (SplatTransform, `.collision.glb`, `research/research.md` section 2). [Fact, from research docs]

---

## 1. Machines and their boundary

### 1.1 The three machines

| Machine | Scope | Instances | Where it lives (proposed) |
| --- | --- | --- | --- |
| Game-mode | Whole session | 1 | Pure step in a new crate `crates/splat-game`; state held by `Engine` |
| Asset | One loadable part (splat, collider) | 0..n | Pure step in `splat-game`; registry held by `Engine` |
| Vehicle | One car | 1 today, keyed by `CarId` | Pure step in `splat-game`; one state per car inside `Engine` |

**Placement [Rec].** `splat-game` has **no dependencies**: no `wgpu`, `rapier3d`, `splat-engine`, `serde_json` or I/O. The purity of the step functions is then enforced by the dependency graph, not by convention. `splat-engine` (which already owns physics, console and logging) runs the driver loop that maps console commands and physics samples to events and applies effects.

### 1.2 Boundary rules [Rec]

1. Machines never call each other. The driver is the only place that does I/O and routes messages:
   - Asset → Game: `AssetsChanged { all_ready }`, `AssetFailed(id)`.
   - Asset → Vehicle: `CarReady`, `CarFailed`.
   - Physics → Vehicle: one `Sample` per physics tick.
   - Console → Game gate → Vehicle: commands (section 1.4.4).
   - Game and vehicle machines → driver: effects. The driver applies them.
2. The game machine's guards read only `GameState`. Asset readiness arrives as an event and is stored in `assets_ready: bool`. After every step the driver asserts that this equals the registry's `all_ready()`.
3. The vehicle machine never reads game mode. The game machine decides whether a command reaches the vehicle.
4. Payloads (bytes, decoded splat, GPU buffers, Rapier colliders) live in driver-owned stores. Events and states carry IDs only, which keeps `step` pure and cheap to clone.
5. Rapier types never enter a machine. The physics adapter converts Rapier state to `PhysicsSample`.
6. **One source for pause.** `Engine.paused` becomes a function of game mode (`mode == Paused(_)`). The console `pause` and `resume` verbs send events and no longer write the flag directly.

```
          load / decode / upload / collider             Sample (per physics tick)
 Asset ─────────────────────────────► Driver ◄──────────────────── Physics adapter (rapier3d)
   │  AssetsChanged / AssetFailed        │
   ▼                                     │ routes events, applies effects
 Game ───── Release/Park/Respawn ──────► Vehicle (per car)
   ▲                                     │
   └──────── console verbs (gated) ──────┘
```

### 1.3 Game-mode machine (session)

#### 1.3.1 States

| State | Meaning |
| --- | --- |
| `Boot` | Platform init: window and GPU device. |
| `Loading` | Required asset parts are in flight or being rebuilt. |
| `Menu` | Assets ready; choosing a mode. |
| `Practice` | Free drive. Car is `Driving` or `Parked`. |
| `Race(Countdown)` | Car on the grid; countdown running; input locked. |
| `Race(Running)` | Race clock running; input enabled. |
| `Race(Finished)` | Finish recorded; short fixed delay before results. |
| `Paused(ResumePoint)` | Engine ticks stopped. `ResumePoint` is `Practice`, `RaceCountdown` or `RaceRunning`. |
| `Results` | Race times and actions. |
| `Error(ErrorKind)` | `AssetFailed(id)` or `Fault(code)`. |

#### 1.3.2 Transition table

| # | From | Event | Guard | To | Side effects |
| --- | --- | --- | --- | --- | --- |
| G1 | Boot | BootDone | — | Loading | Request all asset parts |
| G2 | Boot | Fault(c) | — | Error(Fault(c)) | Show error |
| G3 | Loading | AssetsChanged{all_ready: true} | — | Menu | Show menu |
| G4 | Loading | AssetFailed(id) | — | Error(AssetFailed(id)) | Release other assets; show `id` |
| G5 | Menu | UiStart(Practice) | `assets_ready` | Practice | Vehicle `Release` |
| G6 | Menu | UiStart(Race{track}) | `assets_ready` | Race(Countdown) | Vehicle to grid checkpoint 0; start timer (`game.countdown_seconds`) |
| G7 | Practice | UiPause | — | Paused(Practice) | Engine stops ticking |
| G8 | Practice | UiQuit | — | Menu | Vehicle `Park` |
| G9 | Practice | Command(Reset) | — | Practice (unchanged) | Vehicle `Command(Reset)` |
| G10 | Race(Countdown) | CountdownElapsed | — | Race(Running) | Vehicle `Release`; start race clock |
| G11 | Race(Countdown) | UiPause | — | Paused(RaceCountdown) | Engine stops ticking; countdown paused |
| G12 | Race(Countdown) | UiQuit | — | Menu | Vehicle `Park`; cancel timer; discard run |
| G13 | Race(Countdown) | Command(Reset) | — | (rejected: WrongPhase) | — |
| G14 | Race(Running) | UiPause | — | Paused(RaceRunning) | Engine stops ticking; race clock paused |
| G15 | Race(Running) | UiQuit | — | Menu | Vehicle `Park`; discard run |
| G16 | Race(Running) | RaceFinished{t} | — | Race(Finished) | Vehicle `Park`; record `t`; start results delay |
| G17 | Race(Running) | Command(Reset) | — | Race(Running) (unchanged) | Vehicle `Command(Reset)`. Penalty policy: open question 1 |
| G18 | Race(Running) | VehicleWrecked(id) | — | Race(Running) (unchanged) | Start 2 s timer; on expiry, vehicle `Respawn` |
| G19 | Race(Finished) | ResultsDue | — | Results | Show results |
| G20 | Paused(r) | UiResume | — | Mode named by `r` | Engine ticks again; accumulator reset to zero; timers resume |
| G21 | Paused(r) | UiQuit | — | Menu | Vehicle `Park`; cancel timers |
| G22 | Paused(r) | Command(Reset) | — | (rejected: Paused) | — |
| G23 | Results | UiRestart | `assets_ready` | Race(Countdown) | As G6, same track |
| G24 | Results | UiQuit | — | Menu | — |
| G25 | Menu, Practice, Race(*), Paused(*), Results | AssetsChanged{all_ready: false} | — | Loading | Engine stops; vehicle `Unload` |
| G26 | Any except Boot, Error | Fault(c) | — | Error(Fault(c)) | Engine stops; release assets |
| G27 | Error(AssetFailed(id)) | UiRetry(id) | — | Loading | Asset `Request(id)` |
| G28 | Error(_) | UiQuit | — | Exit | Engine exits (native). Web: stays on error screen |
| G29 | Any | (no row matches) | — | (unchanged) | Ignored; logged at debug |
| G30 | Paused(_) | StepRequested(n) | — | Paused(_) (unchanged) | Run exactly `n` ticks with the engine gate open; log `stepped n` |
| G31 | Menu, Practice, Results | Reload | — | (unchanged) | Vehicle `Rebuild` (body recreated at spawn); config re-read |
| G32 | Race(*), Paused(*) | Reload | — | (rejected: InRace or Paused) | — |
| G33 | Any except Paused | StepRequested(n) | — | (rejected: NotPaused) | Logged at warn |
| G34 | Any except Menu | UiStart(_) | — | (rejected: WrongPhase) | Logged at warn |

Rules [Rec]:
- **G-R1.** Game rows never mutate vehicle state directly. They emit vehicle commands.
- **G-R2.** The wreck respawn timer is owned by the game machine (G18), so penalty and timing policy live in one place.
- **G-R3.** `Race(Finished)` lasts a fixed delay and accepts no input.
- **G-R4.** Console `quit` is a process-level `Effect::Quit`, as it is today. It bypasses this table.

### 1.4 Vehicle machine (per car)

#### 1.4.1 States

| State | Meaning |
| --- | --- |
| `Unloaded` | No car body. |
| `Loading` | Waiting for the car asset (splat plus collider). |
| `Parked` | Body exists and is frozen at its spawn or grid pose. Used on the grid and in menus. |
| `Driving { air_ticks }` | At least one ground contact. `air_ticks` counts consecutive samples with none. |
| `Airborne` | No ground contact for at least `K_AIR` consecutive samples. |
| `Crashed { hold }` | Impact between the crash and wreck thresholds. Short recovery window for FX and camera. |
| `Wrecked` | Impact at or above the wreck threshold. No automatic exit. |
| `Respawning { settle }` | Teleported to a checkpoint; waiting for speed to stay low for `SETTLE` samples. |

#### 1.4.2 Physics signals consumed

| Field | Source | Used for | Status |
| --- | --- | --- | --- |
| `ground_contacts: u8` | Number of contact pairs between the car collider and the ground or track colliders. Becomes the wheel count when the raycast controller arrives. | Driving vs Airborne; Parked release; settle | Contact pairs: [Verify] rapier 0.36.1 narrow-phase API |
| `chassis_impact_mps: Option<f32>` | Largest closing speed along the contact normal during the tick, from contact events | Crashed and Wrecked | [Verify]. Likely needs contact-force events or a velocity-delta fallback |
| `speed_mps: f32` | `Physics::vehicle_speed()` | Settle | [Fact] exists |
| `in_bounds: bool` | `\|x\| <= physics.ground_half_extent`, `\|z\| <= ground_half_extent` and `y >= ground_y - 5 m` | Out-of-bounds | [Fact] inputs exist; the 5 m margin is [Rec] |

The physics adapter builds one `PhysicsSample` per tick from these. Today it can supply `speed_mps` and `in_bounds`. `ground_contacts` and `chassis_impact_mps` need new rapier queries.

Placeholder tuning constants (none measured):

| Constant | Placeholder | Meaning |
| --- | --- | --- |
| `K_AIR` | 6 samples (100 ms at 60 Hz) | Debounce before Airborne |
| `CRASH_MPS` | 8 m/s | Impact that enters Crashed |
| `WRECK_MPS` | 15 m/s | Impact that enters Wrecked |
| `CRASH_HOLD` | 30 samples | Time in Crashed |
| `SETTLE` | 30 samples with speed below 0.5 m/s | Respawn complete |

**Current body vs. the design.** Today the car is a dynamic body with no wheels. "Parked" and "Freeze" are therefore new behaviour: a body is frozen with rapier body-type or gravity-scale changes [Verify]. Until the raycast controller exists, the engine and brake effects have no meaning and are not emitted.

#### 1.4.3 Transition table

| # | From | Event | Guard | To | Side effects |
| --- | --- | --- | --- | --- | --- |
| V1 | Unloaded | Load | — | Loading | — |
| V2 | Loading | CarReady | — | Parked | Spawn body; freeze it |
| V3 | Loading | CarFailed | — | Unloaded | Notify game |
| V4 | Parked | Release | `ground_contacts >= 1` | Driving{0} | Unfreeze |
| V5 | Parked | Release | `ground_contacts == 0` | Airborne | Unfreeze |
| V6 | Driving{n} | Sample | `ground_contacts == 0` and `n+1 >= K_AIR` | Airborne | — |
| V7 | Driving{n} | Sample | `ground_contacts == 0` and `n+1 < K_AIR` | Driving{n+1} | Counter only |
| V8 | Driving{_} | Sample | `ground_contacts >= 1` | Driving{0} | Counter only |
| V9 | Airborne | Sample | `ground_contacts >= 1` | Driving{0} | — |
| V10 | Driving, Airborne | Sample | `impact >= WRECK_MPS` | Wrecked | Notify game (`VehicleWrecked`); FX |
| V11 | Driving, Airborne | Sample | `CRASH_MPS <= impact < WRECK_MPS` | Crashed{0} | Notify FX (shake, dust) |
| V12 | Crashed{h} | Sample | `impact >= WRECK_MPS` | Wrecked | As V10 |
| V13 | Crashed{h} | Sample | `h+1 < CRASH_HOLD` | Crashed{h+1} | Counter only |
| V14 | Crashed{h} | Sample | `h+1 >= CRASH_HOLD` and `ground_contacts >= 1` | Driving{0} | — |
| V15 | Crashed{h} | Sample | `h+1 >= CRASH_HOLD` and `ground_contacts == 0` | Airborne | — |
| V16 | Driving, Airborne, Crashed | Sample | `!in_bounds` | Respawning{0} | Notify OutOfBounds; teleport to last checkpoint; zero velocity |
| V17 | Wrecked | Respawn, or Command(Reset) | — | Respawning{0} | Teleport; zero velocity |
| V18 | Respawning{s} | Sample | `speed < 0.5` and `s+1 >= SETTLE` and `ground_contacts >= 1` | Driving{0} | — |
| V19 | Respawning{s} | Sample | `speed < 0.5` and `s+1 >= SETTLE` and `ground_contacts == 0` | Airborne | — |
| V20 | Respawning{s} | Sample | otherwise | Respawning{s+1} | Counter only |
| V21 | Driving, Airborne, Crashed, Wrecked, Respawning | Park | — | Parked | Freeze body |
| V22 | Driving, Airborne, Crashed | Command(Reset) | — | Respawning{0} | Teleport; zero velocity (same as the console `reset`) |
| V23 | Parked | Command(Reset) | — | Parked (unchanged) | Teleport to spawn; zero velocity; stays frozen |
| V24 | Driving, Airborne, Crashed, Parked | Command(Wreck) | dev build only | Wrecked | As V10 |
| V25 | Respawning | Command(Reset) | — | (rejected: AlreadyRespawning) | — |
| V26 | Unloaded, Loading | Command(*) | — | (rejected: NotLoaded) | — |
| V27 | Any loaded state | Unload | — | Unloaded | Despawn body |
| V28 | Any loaded state | Rebuild | — | Parked | Recreate body at spawn; frozen (console `reload`) |

Notes:
- V7, V8, V13 and V20 change counters only. The tag does not change, so they are not logged (section 4).
- V28 matches what `Engine::reload` does today: it rebuilds physics and puts the car at spawn with no velocity. The new part is that the car is frozen until released.

#### 1.4.4 Console verbs routed to the machines [Rec]

Commands are **events**. No console path writes state directly. Verbs not listed (`help`, `get`, `set`, `dump`, `logs`, `camera *`, `hdri *`) do not touch the machines.

| Console line | Today | Designed as | Accepted in |
| --- | --- | --- | --- |
| `mode practice` / `mode race` | Sets `game_mode` string | `UiStart(...)` (G5, G6; G34 rejects elsewhere) | Menu |
| `mode menu` (new verb) | — | `UiQuit` (G8, G12, G15, G21) | Practice, Race, Paused, Results |
| `pause` / `resume` | Sets `Engine.paused` | `UiPause` / `UiResume` (G7, G11, G14, G20) | Per table |
| `step [n]` | Clears pause temporarily | `StepRequested(n)` (G30; G33 rejects elsewhere) | Paused only |
| `reset` | Teleports car; ungated | `Command(Reset)` (G9, G13, G17, G22; V22, V23) | Practice, Race(Running); rejected in Countdown and Paused |
| `reload` | Rebuilds physics | `Reload` (G31, G32) | Menu, Practice, Results; rejected in Race and Paused |
| `status` | Reports `game_mode` string | Reports `GameState`, vehicle state, asset stages | Any |
| `car.wreck` (new, dev builds) | — | `Command(Wreck)` (V24) | Practice, Race(Running) |
| `quit` | `Effect::Quit` | Unchanged: process exit | Any (G-R4) |

Rejected commands produce a log record (section 4). They are never silent.

### 1.5 Asset lifecycle (splat and collider)

#### 1.5.1 Decisions [Rec]

- **Voxelisation is split by source.**
  - Car: runtime, from splat centres. This already exists in `splat-engine/src/voxel.rs` and runs inside `Engine::new` and `reload`. The `ColliderBuilding` stage calls it.
  - Track: offline. SplatTransform writes `.collision.glb`, and runtime `ColliderBuilding` imports it. This path is not built yet.
  Both appear as the same stage. Only the input differs.
- **One asset machine per part.** The car is one splat part today (`[splat] path`). Wheel splats (Milestone 2) add parts. A registry aggregator turns "all parts Ready" into one `CarReady` for the vehicle machine.
- **Linear pipeline.** `Idle → Fetching → Decoding → Uploading → ColliderBuilding → Ready`. If collider work later runs in parallel with upload, split into render and physics sub-machines with `Ready = render.Ready && physics.Ready`.
- **Retries belong to the asset machine.** Fetch errors retry up to 3 attempts. Decode errors are terminal, since they are deterministic. The game sees only terminal failure.
- **Keep the decoded payload until Ready, and for the session after that** [Rec default]. The car is small. This makes device-loss recovery (A12) possible. Revisit for tracks.
- **Failure policy changes.** Today a failed car load logs an error and runs with no vehicle. The design makes it `AssetFailed` → `Error` (G4). Open question 4.

#### 1.5.2 Transition table

| # | From | Event | Guard | To | Side effects |
| --- | --- | --- | --- | --- | --- |
| A1 | Idle | Request | — | Fetching{1} | Fetch bytes (async) |
| A2 | Fetching{n} | Fetched(Ok(p)) | — | Decoding | Decode payload `p` |
| A3 | Fetching{n} | Fetched(Err(e)) | `e.retryable` and `n < 3` | Fetching{n+1} | Fetch again after backoff |
| A4 | Fetching{n} | Fetched(Err(e)) | otherwise | Failed(Fetch(e)) | Notify aggregator |
| A5 | Decoding | Decoded(Ok(p)) | — | Uploading | Upload `p` |
| A6 | Decoding | Decoded(Err(e)) | — | Failed(Decode(e)) | Terminal |
| A7 | Uploading | Uploaded(Ok(g)) | kind is `SplatWithCollider` | ColliderBuilding | Build collider from payload |
| A8 | Uploading | Uploaded(Ok(g)) | kind is `SplatOnly` | Ready | Notify aggregator |
| A9 | Uploading | Uploaded(Err(e)) | — | Failed(Upload(e)) | Free partial GPU resources |
| A10 | ColliderBuilding | ColliderBuilt(Ok(c)) | — | Ready | Notify aggregator |
| A11 | ColliderBuilding | ColliderBuilt(Err(e)) | — | Failed(Collider(e)) | Terminal |
| A12 | Ready | DeviceLost | Payload kept | Uploading | Re-upload from payload |
| A13 | Ready | Release | — | Idle | Drop GPU resources and collider |
| A14 | Failed(_) | Request | — | Fetching{1} | Only via explicit retry (game `UiRetry`, G27) |
| A15 | Ready | Request | — | (unchanged) | No-op; no record |

Error enums (names only): `FetchError { NotFound, Network, Truncated }`, with `retryable` for `Network`. `DecodeError { BadContainer, UnsupportedVersion, Corrupt }`. `UploadError { OutOfMemory, DeviceLost }`. `ColliderError { Missing, Invalid, VoxelEmpty }`. The last one matches today's `"no splats to voxelise"` error.

Notes:
- On WASM, `Decoding` and `Uploading` run on the main thread, because `sog::load` and `scene::build` are synchronous today. This is a frame-time risk (open question 5).
- Also on WASM: `try_device` uses `pollster::block_on`, which does not work in the browser as written. Asset work cannot assume a blocking device request.

---

## 2. Implementation options

| Option | Fit | Strengths | Weaknesses |
| --- | --- | --- | --- |
| **A. Plain enum + pure `fn step(state, event) -> Transition`** | **Recommended** | No dependency. Exhaustive `match` makes the compiler list missing event×state arms. Pure, so it runs under `wasm32`, native tests and agents, with no GPU or physics. Effects are data, so logs and replays are simple. Matches the existing `Effect` pattern in `engine.rs`. | Nested states (`Race(Running)`) are coded by hand. Guards are hand-written. |
| B. Typestate (types encode state; transitions consume `self`) | Narrow use only | Illegal transitions fail to compile. | Session and vehicle are driven by runtime events (console, async results, physics samples). Heterogeneous states in one registry need enum erasure anyway. Adds generics; makes logging and serialisation harder. |
| C. `statig` 0.4.1 | Not chosen | Hierarchical states are its stated purpose. | Handlers mix logic and state; effects are not pure by default. Last release 2025-07-05. [Verify] async support. |
| D. `rust-fsm` 0.8.0 | Not chosen | Small DSL; outputs per transition. | Flat machines only; our game needs `Race` substates and guards. Last release 2025-07-21. [Verify] guard support. |
| E. `smlang` 0.8.0 | Not chosen as primary | DSL with guards and actions; no_std; async. [Verify] guard syntax. | The DSL hides the table we want to review. Actions are side-effecting closures. Last release 2024-08-09. |
| F. `sm` 0.9.0 | Reject | — | Last release 2019-11-09. |
| G. `typestate` crate | Reject | — | Last stable 0.8.0 (2021-09-02). Only a 0.9.0-rc2 prerelease is newer. |

**Recommendation: Option A.** [Rec]

Reasons:
1. The parts that must be tested are guards over a snapshot of data and effects as data. A pure function provides both directly. None of the crates above gives pure, data-only effects without a wrapper.
2. Exhaustive `match` gives the main thing a state-machine crate would add: a compile error when an event×state pair is missing.
3. Agents can drive the machine by feeding event sequences into `step` and reading back the records. No window, GPU or physics world is needed.
4. It matches the code already in `engine.rs`, where console commands return an `Effect`. Adding game effects extends that pattern instead of introducing a second one.
5. No new dependency. The crates above were last released between 2019 and 2025.

Single exception: a **typestate-style newtype** for the render and physics handles, `ReadyTrack` and `ReadyCar`. Only the asset aggregator constructs them. This makes "drive or render a model that is not loaded" a type error where the code uses these handles. The runtime guard `assets_ready` remains, because the type cannot express session timing.

Crate facts were checked on crates.io today: existence, version and last-update date only. Items marked [Verify] were not checked against their documentation.

---

## 3. Event, effect and state signatures

Signatures only, not implementations. Names may change. Keep the split between pure data and I/O.

```rust
// ---------- Shared (crates/splat-game, no dependencies) ----------
pub type Tick = u64;                    // physics tick counter, owned by Engine
pub struct AssetId(pub &'static str);   // "car.body", "track.main"
pub struct CarId(pub u16);
pub struct TrackId(pub &'static str);
pub struct CheckpointId(pub u16);
pub struct PayloadId(pub u32);          // key into the driver's payload store
pub enum Outcome { Applied, Unchanged, Ignored, Rejected(RejectReason) }
pub enum RejectReason { WrongPhase, Paused, InRace, NotLoaded, AlreadyRespawning, AssetsNotReady }

// ---------- Game-mode machine ----------
pub struct GameState {
    pub mode: Mode,
    pub assets_ready: bool,              // mirrors registry.all_ready(); driver asserts
}
// Engine.paused is derived: matches!(state.mode, Mode::Paused(_))
pub enum Mode {
    Boot, Loading, Menu, Practice,
    Race(RacePhase),
    Paused(ResumePoint),
    Results,
    Error(ErrorKind),
}
pub enum RacePhase { Countdown, Running, Finished }
pub enum ResumePoint { Practice, RaceCountdown, RaceRunning }
pub enum ErrorKind { AssetFailed(AssetId), Fault(u16) }
pub enum StartRequest { Practice, Race { track: TrackId } }

pub enum GameEvent {
    BootDone,
    AssetsChanged { all_ready: bool },
    AssetFailed(AssetId),
    Fault(u16),
    UiStart(StartRequest),               // console `mode practice|race`
    UiPause, UiResume, UiQuit, UiRestart,
    UiRetry(AssetId),
    StepRequested(u32),                  // console `step n`
    Reload,                              // console `reload`
    Command(VehicleCommand),             // console `reset`, `car.wreck`
    CountdownElapsed,
    RaceFinished { time_ms: u64 },
    ResultsDue,
    VehicleWrecked(CarId),
}

pub enum GameEffect {
    Asset(AssetCommand),                 // Request(id), Retry(id), Release(id)
    Vehicle(CarId, VehicleCommand),      // Release, Park, Unload, Rebuild, Respawn, Command(..)
    Engine(EngineCmd),                   // Stop, Run, ResetAccumulator, Step(n)
    Timer(TimerCmd),                     // Start / Cancel a named timer
    Ui(Screen),
    Exit,
}

pub struct GameTransition {
    pub next: GameState,
    pub effects: Vec<GameEffect>,
    pub outcome: Outcome,
}
pub fn step_game(state: &GameState, event: GameEvent) -> GameTransition;

// ---------- Vehicle machine (one per car) ----------
pub enum VehicleState {
    Unloaded, Loading, Parked,
    Driving { air_ticks: u8 },
    Airborne,
    Crashed { hold: u16 },
    Wrecked,
    Respawning { settle: u16 },
}
pub struct PhysicsSample {
    pub ground_contacts: u8,             // wheel count once the raycast controller exists
    pub chassis_impact_mps: Option<f32>,
    pub speed_mps: f32,
    pub in_bounds: bool,
}
pub enum VehicleEvent {
    Load, CarReady, CarFailed, Release, Park, Unload, Rebuild,
    Sample(PhysicsSample),
    Respawn,
    Command(VehicleCommand),
}
pub enum VehicleCommand { Reset, Wreck }
pub enum VehicleEffect {
    Freeze(bool),
    Teleport(CheckpointId),
    ZeroVelocity,
    SpawnBody,
    DespawnBody,
    Notify(VehicleNotice),
}
pub enum VehicleNotice { CarFailed, Wrecked(CarId), OutOfBounds }
pub struct VehicleTuning { pub k_air: u8, pub crash_mps: f32, pub wreck_mps: f32,
                           pub crash_hold: u16, pub settle: u16 }
pub struct VehicleTransition { pub next: VehicleState, pub effects: Vec<VehicleEffect>, pub outcome: Outcome }
pub fn step_vehicle(state: &VehicleState, event: VehicleEvent, tuning: &VehicleTuning) -> VehicleTransition;

// ---------- Asset machine (one per part) ----------
pub enum AssetKind { SplatOnly, SplatWithCollider }
pub enum AssetStage {
    Idle,
    Fetching { attempt: u8 },
    Decoding,
    Uploading,
    ColliderBuilding,
    Ready,
    Failed(AssetFailure),
}
pub enum AssetFailure { Fetch(FetchError), Decode(DecodeError), Upload(UploadError), Collider(ColliderError) }
pub enum AssetEvent {
    Request,
    Fetched(Result<PayloadId, FetchError>),
    Decoded(Result<PayloadId, DecodeError>),
    Uploaded(Result<GpuId, UploadError>),
    ColliderBuilt(Result<ColliderId, ColliderError>),
    DeviceLost,
    Release,
}
pub struct GpuId(pub u32);
pub struct ColliderId(pub u32);
pub fn step_asset(stage: &AssetStage, event: AssetEvent, kind: AssetKind) -> AssetTransition;

// ---------- Observability (section 4) ----------
pub enum Machine { Game, Vehicle(CarId), Asset(AssetId) }
pub struct TransitionRecord {
    pub tick: Tick,
    pub machine: Machine,
    pub from: String,                    // from an exhaustive tag function over the state
    pub event: String,                   // from an exhaustive tag function over the event
    pub outcome: Outcome,
    pub to: String,
}

// ---------- Driver (splat-engine; the only I/O) ----------
// Maps splat_engine::console::Command to GameEvent, feeds PhysicsSample per tick,
// applies GameEffect / VehicleEffect / AssetEffect, and logs each TransitionRecord.
pub fn console_to_event(cmd: &splat_engine::console::Command) -> Option<GameEvent>;
```

---

## 4. Observability requirement

**Requirement.** Every machine transition must produce a log record, and the console must show it. [Rec: a project requirement, not optional.]

The existing `splat_engine::log::Logger` is the single sink. It already gives `seq`, `tick`, `level`, `module` and an in-memory tail (`logs` verb). No new logging channel is needed.

Rules:

- **R1. One record per event that is Applied, Rejected, or Unchanged with an effect.** Module names: `game`, `vehicle`, `asset`. Each record carries `tick`, `from`, `event`, `outcome`, `to`.
- **R2. Ignored events are logged at debug.** Tick-rate samples are never logged as Ignored. They would flood the log.
- **R3. Same-tag updates with no effect are not logged.** Counter-only changes (V7, V8, V13, V20) and no-op rows (A15) produce nothing. Same-tag rows that emit effects (G9, G17, G18, G30) are logged as `Unchanged`.
- **R4. Rejections are always logged, at warn**, with the reason. Silent rejection of console commands is not allowed.
- **R5. Tick stamp.** `tick` is the engine tick at which the driver applies the event. Asset results arrive asynchronously and are stamped when the driver dequeues them, not when the work finished.
- **R6. Message format.** The message is the transition text, so `Logger::tail` and `logs` show it directly:
  `Race(Countdown) --CountdownElapsed--> Race(Running) [applied]`.
  Levels: applied is info; unchanged-with-effect and ignored are debug; rejected is warn.
- **R7. Tests use the same records.** The driver returns `Vec<TransitionRecord>` for a step sequence, and the same text is logged. Tests may also read `Logger::open(None, …).tail(…)`. Tests should not parse console output.
- **R8. `status` shows full state.** Extend `Engine::status_json` to add `game` (mode), `vehicle` (state tag, ground contacts) and `assets` (stages). The existing `game_mode` string is removed.
- **R9. Plain data.** Records, events and effects derive `Debug`, `Clone` and `PartialEq`. The `Record` type has no structured fields today. Structured fields are not needed now, since the message format is stable and tests use typed records.

Example lines (format as `Logger` writes them):

```
{"seq":40,"tick":1204,"level":"info","module":"game","msg":"Race(Countdown) --CountdownElapsed--> Race(Running) [applied]"}
{"seq":41,"tick":1204,"level":"info","module":"vehicle","msg":"Parked --Release--> Driving [applied]"}
{"seq":77,"tick":1391,"level":"info","module":"vehicle","msg":"Driving --Sample(impact 16.2 m/s)--> Wrecked [applied]"}
{"seq":78,"tick":1391,"level":"info","module":"game","msg":"VehicleWrecked --> Race(Running) [unchanged]"}
{"seq":12,"tick":93,"level":"warn","module":"game","msg":"Race(Countdown) --Command(Reset)--> (rejected: WrongPhase)"}
```

---

## 5. Test plan

### 5.1 Unit tests (pure; no GPU, no physics)

Every row in the three tables needs at least one test: 34 game rows (G1–G34), 28 vehicle rows (V1–V28) and 15 asset rows (A1–A15). Specific cases:

- **Game.** G13, G22, G32, G33 and G34 (rejections). G29 (ignored default). G23 with `assets_ready == false`: no transition.
- **Game, pause.** G7 then G30 with `n = 3`: exactly 3 engine ticks, then back to Paused with no other change.
- **Vehicle hysteresis.** `Driving` with `K_AIR - 1` airborne samples stays Driving. The `K_AIR`-th enters Airborne. A ground contact in the middle resets the counter.
- **Vehicle thresholds.** Impact exactly `CRASH_MPS` enters Crashed. Exactly `WRECK_MPS` enters Wrecked. Just under `CRASH_MPS` does nothing.
- **Vehicle settle.** Speed 0.49 for `SETTLE` samples respawns to Driving. Speed 0.5 does not count.
- **Vehicle reset.** V22 vs V23: reset while driving starts a respawn; reset while parked stays parked.
- **Asset retry.** `Network` retries up to 3 attempts, then fails. `NotFound` fails on attempt 1. `DecodeError` never retries.
- **Asset device loss.** `Ready` plus `DeviceLost` goes to `Uploading`, not `Failed`.

### 5.2 Invariants (random event sequences, seeded, 10 000 sequences each)

| ID | Invariant |
| --- | --- |
| I1 | `Race(*)` or `Practice` is entered only when `assets_ready == true`. G5, G6 and G23 check it. G20 resumes only from `Paused`, and G25 leaves `Paused` when assets are lost. |
| I2 | `Results` is entered only from `Race(Finished)`. |
| I3 | While `Paused(_)`, the engine tick counter does not change, except inside a `StepRequested(n)` that advances it by exactly `n`. The engine gate is derived from the mode, so no path sets it separately. |
| I4 | Each event that is Applied, Rejected, or Unchanged with an effect produces exactly one record. Counter-only updates produce none. |
| I5 | A vehicle in Driving, Airborne, Crashed, Wrecked or Respawning implies car assets are Ready (debug assertion in the driver). |
| I6 | An asset reaches Ready only through ColliderBuilding (`SplatWithCollider`) or Uploading (`SplatOnly`). |
| I7 | `assets_ready` equals `registry.all_ready()` after every driver step. |
| I8 | `Error(_)` is left only by `UiRetry` (for `AssetFailed`) or `UiQuit`, or by a process exit. |
| I9 | Console verbs change state only by producing an event that matches a table row. There is no direct setter, and `Engine.paused` is written only by the mode derivation. |

### 5.3 Existing and later integration tests

Existing, in `physics.rs`: `a_dropped_voxel_body_comes_to_rest_on_the_ground` and `reset_moves_the_vehicle_and_clears_velocity`. These cover the physics side that the vehicle machine's `Sample` depends on.

Later:
- Native headless: load the test SOG through `sog::load`; drive the asset machine from `Request` to `Ready`.
- Physics: drop the car on a flat collider and expect `Parked` → `Driving` after `Release`. Drive off a ramp and expect `Airborne`. Push past the boundary and expect `Respawning` with a checkpoint teleport. These need `ground_contacts` and `chassis_impact_mps`, which do not exist yet.
- WASM: the same asset path, after `try_device` stops using `block_on`.

---

## 6. Open questions

1. **Crash and wreck policy.** Does a wreck cost time, reset to the checkpoint only, or both? This sets G17 and G18. It is tied to the arcade-vs-simulation decision in `research/research.md`.
2. **Gating of `reset` and `reload`.** The design gates `reset` in Countdown and Paused, and `reload` in Race and Paused. Today both are ungated in the console. Confirm the gating, or keep them ungated for debugging.
3. **Start Practice frozen or dropped in?** The design freezes the car on the grid (`Parked`). Today the car starts as a free dynamic body at `vehicle.spawn` and falls. Choose one, since it changes what the player sees on entry.
4. **Asset failure policy.** Today a failed splat load runs the world with no vehicle and one error log line. The design makes it `Error` (G4). Choose fatal, or degrade to a menu with an error banner.
5. **WASM decode and upload threading.** Decode and upload are synchronous today. Decide whether they run on the main thread with a per-frame time budget, or in a Web Worker. This sets the async shape of the asset machine.
