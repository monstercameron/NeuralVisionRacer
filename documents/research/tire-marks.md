# Tire tread and skid marks: research and implementation proposal

Researched 2026-10-25 for NeuralVisionRacer's Rust/WASM WebGPU engine. Research only; no gameplay or renderer changes accompany this document.

## Recommendation

Generate persistent marks from each wheel's ground contact in fixed simulation time. Use road-relative motion, wheel rotation, tire load, and traction loss to control deposition. Feed RPM through the existing torque curve, transmission, clutch, and driven-axle assignment. Keep each deposited mark fixed in world space.

For the first rendering experiment, use a bounded set of instanced, depth-projected tread segments in one WGSL pass. Compare this against simple contact ribbons on the current scans before committing to the more complex projection path. The main uncertainty is how closely the existing Gaussian depth represents the visible road.

The immediate prerequisite is telemetry: **our current wheel angular speed cannot independently reveal wheelspin or brake lock.** A shader alone cannot recover that missing information.

## What the engine actually provides

These findings come from the current local working tree, including uncommitted work. Paths below are relative to the repository.

| Signal | Current source | Meaning and limitation |
| --- | --- | --- |
| Grounded, world contact point/normal, collider | `crates/splat-engine/src/wheels.rs`, `RayCastInfo` | Already available internally. Contact point follows a footprint envelope and the normal averages ray hits; neither is an exact scanned surface. |
| Wheel radius, axle, steering | `wheels.rs`, `Wheel`; `drive.rs` | Enough to construct a steered tangent frame on the contact plane. |
| Angular speed and continuous phase | `wheels.rs`, `update_vehicle`; `drive.rs`, `WheelVisualState` | Grounded rotation is derived from chassis velocity divided by radius. It is rolling animation, not an independent spinning-wheel simulation. |
| Tire load and applied impulses | `Wheel::wheel_suspension_force`, `forward_impulse`, `side_impulse` | Useful for contact weighting and a friction-work proxy. Use the force actually clamped/applied by the solver. |
| Traction saturation | private `Wheel::skid_info`, `update_friction` | Starts at 1 and falls when requested combined impulses exceed the grip budget. It is an impulse scaling factor, not longitudinal slip ratio. |
| RPM, torque, gear, shift, launch, wheelspin flag | `crates/splat-car/src/car.rs`, `Output` | `wheelspin` reports requested drive exceeding an aggregate traction limit before capping; it does not measure per-wheel overspeed. |
| Clutch/gearing and torque curve | `car.rs`, `Car::step`; `spec.rs`, `CarSpec` | Launch clutch slip explicitly permits high engine RPM at low road speed. Engine speed itself is generally derived from road speed and gearing. |
| Ground-relative contact velocity | Rapier body velocity at point, ground collider/body | Can be computed, including chassis yaw and moving-ground motion. Not currently exposed as a mark sample. |
| Tire width | `.car` descriptive tire strings | Current car says 275 mm front / 335 mm rear, with radii 0.350 / 0.367 m and rear-wheel drive. These are nominal specification widths, not measured footprint widths. Add explicit tunable mark widths. |
| Surface/material classification | Collision tiles and level metadata | No verified per-contact asphalt/dust/mud map. Do not infer friction or rubber retention from the scene's name or baked color. |

The upstream [Rapier controller source](https://docs.rs/rapier3d/latest/src/rapier3d/control/ray_cast_vehicle_controller.rs.html) also derives grounded rotation from motion. The local fork is the implementation authority for this game.

Important consequences:

- Comparing `WheelVisualState.angular_speed * radius` with forward speed will yield approximately zero slip by construction, apart from sampling/geometry differences. That difference is not evidence of a physical burnout.
- Holding throttle and brake together currently suppresses wheel engine force in `Drivetrain::update`. A conventional brake-held burnout is not supported by that behavior.
- Accurate wheel lock, wheelspin, differential behavior, or ABS cannot be claimed without additional wheel rotational state and appropriate controls.

## Physical quantities to use

[NVIDIA's tire-slip documentation](https://nvidia-omniverse.github.io/PhysX/physx/5.1.2/_build/physx/latest/function_group__vehicle2_1ga2407f0b9eb2ee99df3643f4ceabd53e6.html) bases slip on wheel circumference speed and contact velocity resolved along the tire axes, with protected low-speed denominators. Following that model, define a consistent signed wheel convention:

```text
v_rel = chassis.velocity_at_point(contact) - ground.velocity_at_point(contact)
v_long = dot(v_rel, tire_forward_on_contact_plane)
v_side = dot(v_rel, tire_right_on_contact_plane)
u = wheel_angular_speed * effective_radius

long_slip_speed = u - v_long                    # m/s
slip_ratio = (u - v_long) / max(abs(v_long), v_min)
slip_angle = atan2(v_side, max(abs(v_long), v_min))
```

Choose `v_min` through telemetry, with a separate absolute slip-speed threshold so low-speed numerical noise cannot create dark marks. Use signed velocities for reverse. Unity's [WheelHit API](https://docs.unity.com/en-us/engine/6000.0/script-reference/unityengine/wheelhit) independently exposes contact, force, tire directions, and longitudinal/lateral slip for effects; its slip sign convention differs, so thresholds must not be copied blindly.

### How RPM belongs in the effect

For a coupled driveline, the axle-average relationship is:

```text
wheel_rpm ≈ engine_rpm / (abs(gear_ratio) * final_drive)
wheel_torque ≈ engine_torque(rpm, throttle) * abs(gear_ratio) * final_drive * efficiency
```

This is not a valid wheel-speed measurement during clutch slip, neutral, or a shift. It also does not resolve individual wheel speeds in a turn. Use delivered torque and driven-axle status to weight acceleration marks; use actual contact slip to determine rubbing. Brake and drift marks must still work with a closed throttle and low RPM.

Proposed artistic intensity model, **not a calibrated rubber or thermal simulation**:

```text
deposition_rate ∝ surface_retention * loaded_contact * rubbing_activity
```

With independently simulated wheel speed, rubbing activity can use friction-work magnitude such as `abs(F_long * long_slip_speed) + abs(F_side * v_side)`. Accumulate it with `dt`, then map accumulated work/area to bounded opacity. A parked spinning tire should darken one patch rather than allocate thousands of overlapping segments.

### Honest first version versus deeper physics

| Option | Inputs | Scope |
| --- | --- | --- |
| Recommended first version | Existing load, delivered impulses, lateral contact velocity, `skid_info`, mechanical traction-excess flag, RPM/torque/gearing | Visual traction proxy. Can show corner scrub and traction-limited launches; cannot prove physical wheel overspeed or brake lock. Record that limitation in telemetry. |
| Independent wheel dynamics | Integrate angular momentum from drive torque, brake torque, and tire force; couple slip to tire forces and engine/clutch | Supports real longitudinal slip and brake lock. Requires tuning and driving regression tests; not a shader-only change. |

Keep `rolling_angular_speed`, `visual_angular_speed`, and any later `simulated_angular_speed` distinct. If an artistic spin estimate is added, label it as estimated and keep it out of measured-slip fields. Do not alter the accepted rim animation while adding marks without a separate comparison.

## Appearance and persistence

These are proposed art rules, pending visual evaluation:

| Driving state | Mark behavior |
| --- | --- |
| Stopped, free revving, airborne, or negligible tire load | No new trail. A stationary patch needs demonstrated or explicitly estimated wheelspin. |
| Normal rolling | Optional faint tread, stronger on a configured dusty surface; near-zero rubber on the default dry paved preset. |
| Hard acceleration with traction loss | Darker on driven wheels. Current Porsche is RWD. High RPM alone does not activate it. |
| Hard braking | Rubber when traction saturation/sliding warrants it. Do not label strong braking as wheel lock unless wheel dynamics support it. |
| Drift/corner scrub | More continuous, smeared marks based on lateral slip and load. Steering rotates the tread relative to the trail path. |
| Recovery to grip | Smoothly taper deposition and tread smear; already deposited sections remain fixed. |

Sample by traveled distance with curvature refinement, not once per display frame. Resample long simulation steps so fast driving cannot leave gaps. Use ground distance for trail placement and accumulated wheel phase for the contact pattern; record both at deposition. A printed track must not scroll when the wheel continues to rotate elsewhere. Under large slip, blend crisp tread into a smeared rubber band and filter fine grooves to prevent shimmer.

Break continuity on contact loss, reset, respawn, teleport, incompatible surface normals, or an implausible contact jump. Retain old world-space marks across collision-tile streaming; collider handles are not permanent surface identities. Change level/restart clears history. Clamp or bevel sharp turns to avoid wide ribbon spikes.

## Rendering on Gaussian splat roads

Ordinary splat color rendering does not supply a conventional solid road surface. [PlayCanvas documents the depth limitation and proxy-depth workaround](https://developer.playcanvas.com/user-manual/gaussian-splatting/rendering-architecture/renderers/). Our renderer already adds alpha-clipped depth prepasses: level cutoff 0.5 and car cutoff 0.8 in `crates/splat-render/src/frame.rs`. These record opaque Gaussian cores, not exact road geometry.

| Method | Advantages | Risk in this renderer |
| --- | --- | --- |
| Contact ribbon triangles | Small mesh, one draw, straightforward tread UVs | Collision heights are deliberately smoothed and can disagree with visible splats. A fixed offset can leave ribbons buried or floating. Depth bias only resolves small depth conflicts. |
| Depth-projected segment decals | Conform coloring to reconstructed visible depth; no new splats or sorting | Needs valid level depth wherever marks appear. Core-depth holes and foreground splats can cause missing marks. Projection slabs need height/normal rejection to avoid painting walls. |
| World-space mark atlas sampled by level shader | Persistent surface darkening without floating geometry | Per-center tint blurs narrow tread across large Gaussians; accurate per-fragment mapping costs more. Atlas needs spatial/height handling and streaming. |
| Added Gaussian marks | Compatible with Gaussian appearance | Extra splats, sorting/streaming complexity, and soft tread edges. Poor first choice for sharp tread. |

PlayCanvas's [custom shader documentation](https://developer.playcanvas.com/user-manual/gaussian-splatting/building/custom-shaders/) confirms per-center modification is cheaper, while per-fragment modification supports detail across a splat's footprint. That supports the atlas tradeoff; it does not establish that their engine provides a ready-made tire decal feature.

### Proposed WebGPU experiment

1. Expose contact telemetry and generate a short deterministic mark path on a flat test road and Saburi Alley.
2. Compare ribbon placement against a projected segment pass using identical marks. Judge side views, cobbles, kerbs, car overlap, and distance changes.
3. For projection, draw one indexed quad per segment's conservative projected slab bounds. Reconstruct world position from `depth_level`, reject clear depth and positions outside the finite contact slab/tread footprint, and reject pixels hidden by `depth_car`. Handle near-plane crossings conservatively.
4. Blend dark tread over level color **after level color and before car color**, with no depth writes. Run before DOF/post so marks share the scene's image treatment. Do not sample a depth texture while also binding it as a writable attachment.
5. Extend the existing level-depth scissor from the car rectangle to include visible marks; measure the extra depth cost. Preserve the current path when no marks are visible. Full-frame depth may be needed for scattered visible trails, but should not become unconditional.
6. Use a filtered procedural tread initially, with edge fade and bounded opacity. Add authored tread textures only if the procedural result lacks quality. Modulating road color preserves baked light and the existing car shadow rather than applying another sun shadow to the decal.

These are design proposals, not benchmarked claims. WebGPU/wgpu already supports the needed [blend state](https://docs.rs/wgpu/latest/wgpu/struct.BlendState.html) and [depth bias for the ribbon alternative](https://docs.rs/wgpu/latest/x86_64-apple-darwin/wgpu/struct.DepthBiasState.html). No additional rendering framework is necessary.

## Budget and instrumentation

Start with an **8,192-segment ring**, distance sampling around 0.25 m, and at most one visible-segment draw. At 96 bytes per proposed instance that is 768 KiB, excluding CPU history and visible-list staging. At four continuously marking wheels, this capacity represents about 512 m of vehicle travel; a time-to-live cannot override that storage limit. Eviction may happen before the nominal fade time.

Update only changed data, keep buffers allocated, and cull old/distant/off-screen marks. Avoid iterating every historical segment in every level-splat shader. Avoid synchronous GPU readbacks.

Add a `tire_marks` pass to the existing shader catalogue and sampled GPU timings. Also record CPU generation/culling/upload time, active/visible/emitted/evicted segments, uploaded bytes, continuity breaks, surface preset, per-wheel load/saturation/slip estimates, source labels for wheel speed, and stationary-patch accumulation. Report additional `level_depth` work separately. Use the current optional [timestamp-query support](https://docs.rs/wgpu/latest/wasm32-unknown-unknown/wgpu/enum.QueryType.html); retain CPU/count metrics if timestamps are unavailable.

Initial experiment targets at the same 1280×720 scene/camera: tire pass at or below 0.5 ms and total frame increase at or below 1 ms. **Targets only, not measured performance.** Compare matched captures with marks off/on and DOF off/on; the expanded depth prepass could cost more than the decal shader.

## Validation before shipping

- Idle, neutral revving, airborne wheels, and resets produce no unwanted marks or connecting streaks.
- At matched contact speed, changing display FPS does not change mark density or accumulated darkness. Repeat with different physics substep counts and check tolerance.
- Rolling, lateral sliding, traction-limited launch, reverse, braking, and gear shifts exercise distinct inputs. A proxy version never reports estimated wheelspin as measured slip.
- Mark width/spacing follows all four real contacts. Deposited tread stays fixed after driving away; stationary rubbing darkens a bounded patch.
- Car/walls obscure marks; road texture and baked shadows remain visible. Inspect depth holes, sharp kerbs, slopes, low camera angles, near-plane crossings, and LOD transitions on every level.
- Streaming out/back does not move marks; capacity, fade, and level changes remain bounded and deterministic.
- Export existing profiler JSON, compare total GPU/CPU cost, and visually review an actual WASM/WebGPU driving capture.

The research is sufficient to begin the telemetry/projection experiment. Remaining unknowns are depth quality per scan, surface presets, visual thresholds, and actual GPU cost; those require measurement rather than more generic tire equations.
