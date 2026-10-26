# Ground-material dust

The Rust vehicle simulation emits world-space dust from the same four loaded wheel contacts used for tyre marks. WebGPU renders the particles through `crates/splat-render/src/shaders/dust.wgsl`. Enabled by default with `effects.dust=1`.

## Behaviour

Loaded wheel contacts choose the effect from the floor material. Airborne, unloaded and stationary non-slipping wheels do not emit. Wheel RPM only matters through actual angular wheel speed and slip, so revving an unloaded engine does not create a cloud.

| Floor | Effect | Trigger and look |
| --- | --- | --- |
| Pavement, asphalt, stone and cobble | Tire smoke | Pale neutral grey; wheelspin, locked braking and lateral sliding. Ordinary rolling emits nothing. |
| Dirt / dry soil / Saburi sand | Ground dust | Warm soil/sand tint; a modest rolling wake, stronger with slip. |
| Grass | Ground dust | Much lighter, muted earth-coloured puffs. |
| Unknown geometry | None | Avoid inventing a material effect. |

Street smoke uses longitudinal slip above 0.9 m/s or lateral slip above 0.7 m/s, eased through a 0.25 s rise / 0.45 s decay envelope. This normalized slip envelope is a visual approximation of rubber heating, not a tire-temperature solver. The rate is capped at 24 particles per second per wheel; opacity is 0.085–0.11 and life 1.45–1.9 s. Warm tires can leave a short fading wisp after grip returns.

Dirt responds to rolling speed above 0.8 m/s, wheel slip and lateral motion, capped at 60 particles per second per wheel. Dirt puffs use opacity 0.16–0.205, life 1.65–2.1 s and a low expanding wake. Grass scales this rate to 18%. Both styles remain below the car's splat colour pass to keep its silhouette clear.

The authored Delft driving corridor is known cobble, including repaired floor gaps at the far end; these wheel contacts always select street smoke. Other contacts retain the existing height-filtered material map. Teleport-sized contact jumps clear the emission anchor and heating envelope.

`specs/surface-materials.json` contains 1 m world-space maps for the four collidable levels. `bake_surfaces` samples floor-adjacent source splats, rejects distant/large/dark samples, and distinguishes green ground and neutral paving using colour votes. Level-specific base materials fill uncertain cells. This is colour inference with authored fallbacks, not captured physical material labels. Sparse cells, shadows, and mixed surfaces can be misclassified. Sampled-cell heights reject contacts more than 0.65 m above or below the floor, preventing a floor label from being reused on a roof.

Rebuild maps from the repository root:

```sh
cargo run --release --manifest-path crates/splat-app/Cargo.toml --example bake_surfaces
```

## Rendering and cost

Particles expand, drift, rise gently, and fade using simulation time. Tire smoke has a softer noise density and slightly greater expansion/buoyancy than dirt; CPU culling uses the same motion/radius equations as WGSL. Procedural two-octave noise breaks up the soft billboard. Simulation time drives all motion, so pausing freezes the complete effect; reset, flip, and level changes clear it.

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

137 tests pass across engine and renderer suites; one preexisting driving test remains ignored. New checks cover speed/material differences, blocked-wheel slip, time-step independence, airborne suppression, pause/reset/expiry, capacity eviction, map dimensions/grass separation, height filtering, GPU depth occlusion, unchanged target alpha, disable/restore while paused, and resize. Native examples check and the release WASM build succeeds.

`dust_review <level> <output-dir> <drive|burnout>` captures matching on/off frames using the production renderer and actual vehicle contacts. The burnout case locks the body while letting driven wheels spin. `TICKS=90` can lengthen the drive scenario for comparison.

The reviewed Delft stationary burnout has 80 active tire-smoke particles, with only the two driven rear wheels emitting; no ground dust is present. The Saburi driving scene uses warm ground dust and no tire smoke. Matched full-frame comparisons were inspected after increasing initially invisible dust slightly on the bright sandy road. Street cruising, small grip corrections, locked braking, drift, warm-up/decay, substep independence, material classification, pause/reset/airborne behaviour and both shader styles are covered by tests. Native examples and the release WebAssembly build pass. Particle structs still use the existing 80-byte GPU layout; no pass or buffer is added.
