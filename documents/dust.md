# Ground-material dust

The Rust vehicle simulation emits world-space dust from the same four loaded wheel contacts used for tyre marks. WebGPU renders the particles through `crates/splat-render/src/shaders/dust.wgsl`. Enabled by default with `effects.dust=1`.

## Behaviour

Emission increases with contact-point road speed, longitudinal wheel slip, and lateral sliding. Ordinary rolling starts above 0.8 m/s. Slip is computed from `abs(wheel_angular_speed * radius - longitudinal_speed)`, so a driven wheel can throw dust while the body is blocked. RPM and gearing affect the actual wheel simulation; the effect does not create dust from engine RPM alone. Airborne wheels, unloaded contacts, and stationary non-slipping wheels emit nothing.

Material scales emission and changes tint:

| Ground | Relative dust | Tint |
| --- | ---: | --- |
| Dry soil / Saburi sand | 1.0 | Brown soil / pale warm sand |
| Grass | 0.18 | Muted dry earth |
| Stone / cobble | 0.065 | Pale neutral mineral |
| Pavement / unknown | 0.025 | Neutral grey |

`specs/surface-materials.json` contains 1 m world-space maps for the four collidable levels. `bake_surfaces` samples floor-adjacent source splats, rejects distant/large/dark samples, and distinguishes green ground and neutral paving using colour votes. Level-specific base materials fill uncertain cells. This is colour inference with authored fallbacks, not captured physical material labels. Sparse cells, shadows, and mixed surfaces can be misclassified. Sampled-cell heights reject contacts more than 0.65 m above or below the floor, preventing a floor label from being reused on a roof.

Rebuild maps from the repository root:

```sh
cargo run --release --manifest-path crates/splat-app/Cargo.toml --example bake_surfaces
```

## Rendering and cost

Particles expand, drift, rise gently, and fade over 1.7–2.4 simulation seconds. Procedural two-octave noise breaks up the soft billboard. Simulation time drives all motion, so pausing freezes the complete effect; reset, flip, and level changes clear it.

There are at most 2,048 particles, one fixed 160 KiB instance buffer, a 128-byte uniform buffer, and one optional render pass. CPU preparation culls beyond 90 m and outside the view, sorts visible particles back to front, and uploads the visible instances. A paused unchanged camera reuses the prepared instances. There is no particle GPU readback or collision simulation.

The pass samples existing road and car opaque-core depth, with a 30 cm soft intersection fade. A conservative car-core dilation closes small holes. Dust is composited after road decals and before the car's translucent Gaussian colour tails; this keeps a clear car silhouette despite incomplete splat depth. Consequently this ground-dust layer does not fully engulf the car like a volumetric fog. Depth-of-field and the film grade process dust together with the scene. With DOF disabled, the road depth prepass expands only to the union of car, decals, and visible dust screen bounds.

## Controls and profiling

Console commands:

```text
set effects.dust 0
set effects.dust 1
set effects.dust 2
```

Strength is clamped to 0–2. Zero disables drawing and new emission; existing particles still age in running simulation. The general effects switch also disables dust.

The render profiler includes the `dust` pass, WGSL shader, draw/instance counts, and GPU timestamps where supported. Exported JSON includes simulation generation time, per-wheel inferred material and requested emission rate, active/emitted/evicted counts, culling/sorting/upload CPU time, visible count, and uploaded bytes. GPU timestamp spans can include scheduling/dependency waits and should not be treated as exclusive shader costs.

## Verification

120 tests pass across engine and renderer suites; one preexisting driving test remains ignored. New checks cover speed/material differences, blocked-wheel slip, time-step independence, airborne suppression, pause/reset/expiry, capacity eviction, map dimensions/grass separation, height filtering, GPU depth occlusion, unchanged target alpha, disable/restore while paused, and resize. Native examples check and the release WASM build succeeds.

`dust_review <level> <output-dir> <drive|burnout>` captures matching on/off frames using the production renderer and actual vehicle contacts. The burnout case locks the body while letting driven wheels spin. In this comparison Saburi emits 352 particles over three seconds versus Delft's 22; 252 versus 16 remain alive. These are material-response checks, not overall FPS benchmarks.
