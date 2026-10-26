# Persistent tire tread and rubber

Implemented 2026-10-25 in the shared Rust engine and WGSL/WebGPU renderer. The preceding design investigation is in [research/tire-marks.md](research/tire-marks.md).

Each wheel deposits a world-space contact trail. Road-relative contact speed controls rolling deposition; continuous wheel rotation determines tread phase. Lateral sliding, the solver's clipped tire impulses, and excess driven-axle torque strengthen and smear the rubber. Engine RPM affects torque through the existing torque curve, selected gear, final drive, clutch, and shift interruption. The front tires of the current RWD Porsche do not receive the driven-wheel boost.

Driven wheels now integrate excess angular rotation from signed, pre-traction axle torque, estimated wheel inertia (25 kg solid disc), each contact's loaded grip, and the current engine RPM / applied gear / final-drive speed limit. The no-slip rolling constraint and existing chassis force solver are preserved; this is a rotational slip model, not a full clutch/differential or coupled tire-force replacement. Passive wheels follow road-relative motion. Engine RPM retains the existing road-speed/launch-clutch model. Telemetry reports actual wheel surface speed minus road speed separately from requested torque excess.

A grounded driven tire can spin while a wall or other obstruction holds the chassis still. Below 0.6 m/s chassis contact motion, more than 0.75 m/s longitudinal slip accumulates rubber in a 25 cm patch. Each wheel updates one patch in place until its contact moves 12 cm; opacity approaches a bounded maximum according to sliding distance and load, independently of physics rate. There is no per-frame instance growth. Idle revs without wheel slip and airborne wheels do not paint. Wheel rotation feeds the existing Gaussian animation. Reverse is signed, and reset/flip clears angular state.

The subsequent handbrake control adds an explicit rear-wheel lock: hold either Shift key to lock rear rotation, reduce rear lateral grip, and deposit smeared rubber while sliding. Release restores motor-driven rotation and grip. The mechanical lock overrides engine torque. S brakes forward travel to rest before becoming reverse gas; W brakes reverse travel to rest before becoming forward gas. Both keys together apply the service brakes without selecting a direction. Tiny grounded motion under braking is physically settled to zero before direction selection, rather than engaging reverse at a creeping-speed threshold.

## Behavior and limits

- Contact samples come from the existing suspension/footprint solver; no additional raycasts.
- Fixed physics substeps generate segments about every 0.25 m, including interpolation at high speed. Rendering FPS does not determine their spacing.
- Front/rear widths use the car specification's nominal metric tire sizes, with a 280 mm fallback for other size formats.
- Tread phase is saved when deposited. Existing tracks remain still when the car's RPM changes.
- Ground contact loss, teleports, resets, flips, and abrupt surface-normal changes break continuity. Old marks remain in world space when a car resets; level changes clear the history.
- At most 8,192 segments. Marks fade between 70 and 120 seconds of simulation time; the ring can evict older marks sooner. Pausing stops aging.

## Rendering and controls

One instanced `tire_marks` pass runs after level color and before car color/DOF/post. It validates the visible road using Gaussian core depth within a 0.30 m contact slab, then intersects each pixel's ray with the contact plane for continuous tread coordinates. This avoids imprint distortion from approximate splat depth. It rejects car occlusion and missing depth. Blending darkens the existing road color while preserving opacity, scanned detail, and shadows.

Each instance is 96 bytes; the fixed GPU instance allocation is 768 KiB. Screen bounds cover the contact footprint, while depth validation checks the finite slab. This avoids rasterizing an extruded volume around every mark. CPU culling limits marks to 180 m and the view frustum. Unchanged camera/history reuses the uploaded instance data. With DOF disabled, the level-depth scissor also includes visible tire marks; no visible marks preserve the original depth path.

```text
set effects.tire_marks 0       # hide
set effects.tire_marks 1       # default strength
set effects.tire_marks 2       # stronger
perf                          # render + simulation metrics
perf shaders                  # includes tire_marks.wgsl
```

The setting applies live and follows `effects.enabled`. It changes display strength without deleting contact history.

## Diagnostics and validation

The profiler table displays the new pass and visible/active segment counts. Exported JSON includes CPU generation and cull/upload timings, GPU pass spans, memory/upload counts, continuity breaks, expiration/eviction counts, oldest/newest/strongest segment samples, and per-wheel contact load, road speed, wheel speed, lateral speed, RPM, and traction estimates. GPU spans include dependency waits and must not be interpreted as exclusive shader costs.

Tests cover idle/airborne suppression, distance and tread-phase consistency across substep rates, traction-dependent intensity, reset discontinuities, bounded storage and expiration, actual four-wheel driving contacts, road-only darkening, car/empty-depth rejection, fade/off pixel restoration, resize, and pass profiling. Paused car/camera interpolation now holds the current simulation state, allowing stable on/off visual comparisons.

The WASM build was exercised in the in-app WebGPU browser on Saburi Alley. Rendered mark quality remains limited by Gaussian core-depth coverage: sparse or displaced splats can interrupt a mark. This version changes road appearance; it does not deform ground or simulate rubber temperature/material wear.

## Browser capture

A matched Saburi Alley capture at 672×748 with 206 stored / 94 visible segments measured 19.49 ms mean GPU frame time enabled versus 19.22 ms disabled (difference 0.27 ms). Both sequential windows contain 120 samples of the same paused tick/camera and 2,327,861 resident splats, with DOF, grain, and dashboard disabled for comparison. This is one scene/camera measurement, not a hardware-wide guarantee. The expanded level-depth region is included in the total cost.
