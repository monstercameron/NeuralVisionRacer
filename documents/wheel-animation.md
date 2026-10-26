# Splat wheel motion

The native and WASM renderers follow the full unwrapped `f64` physics wheel phase. The angle is wrapped only
when building the GPU quaternion. Each wheel completes continuous revolutions; it does not use a bounded sine
or reset its angle when speed reaches zero. Angular speed follows road-relative rolling plus independent driven-wheel slip, including reverse.

The hub and each splat's coordinate along the wheel axle remain fixed during spin. Gaussian centres and their
in-plane orientation co-rotate, with unchanged Gaussian scales and unchanged variance along the axle. Leaving
elongated spoke splats' orientation fixed while rotating their centres produced spiky quarter-turn frames, so
orientation must follow the rim to preserve its appearance. Steering and suspension remain physics-driven.
Calipers follow steering and suspension without axle rotation. Original SOG textures remain unchanged.

The drivetrain already uses engine RPM, selected gear ratio, final drive, tyre radius and traction to apply forces.
Passive wheels follow ground-relative contact velocity. Driven wheels add independent rotational slip from axle torque, loaded grip and estimated wheel inertia, capped by the engine speed through the applied gear and final drive. A blocked car can therefore spin its driven wheels and build rubber patches; Gaussian rotation uses this integrated phase. See `tire-marks.md` for the model and limits.

`crates/splat-engine/src/wheel_visual.rs` fits hubs in aligned metric car space and converts poses back to SOG space.
Part 0 is the body, 1–4 are rotating wheel poses, and 5–8 are steering-only caliper poses. The visible
centres and the selected-index compute pass use the same transformation of immutable original centres before GPU
depth sorting. Changes invalidate the sort cache even with a stationary camera. Disabling the rig restores original
sort centres too. The same wheel shapes are used in colour and depth passes.

The Porsche scan has no semantic labels. Selection uses fitted cylinders, nominal tyre sizes, a Gaussian extent
limit and a yellow caliper colour cue. Hub pivots are refined against the metallic rim ring with an angularly balanced
circle fit, so approximate spec axle locations do not make the rim orbit. Insufficient ring evidence falls back to
the spec estimate. Extra axial clearance inside the rim captures protruding spokes without
widening the tyre/fender boundary. Masks remain approximate: source floaters and some boundary/caliper pixels may
be misclassified. Other cars need authored masks or their own fitting/caliper rules. Restart after alignment or
geometry changes so the rig refits.

`set render.debug_wheels true` shows wheel selections in four colours, calipers magenta and the body grey. `status`
includes per-wheel `spin_rad`, `angular_speed_rad_s`, `steering_rad`, `travel_m` and `grounded`. Native and WASM share
one rig and shader implementation. Visual inspection uses native GPU rendering; WASM compilation is also checked.

From the repository root:

```sh
cargo run --locked --release --manifest-path crates/splat-engine/Cargo.toml --example wheel_preview -- /tmp/wheel-preview 90 20 0
```

Arguments: output directory, frame count, speed in km/h, front steering in degrees. Frames are RGBA PAM files at
1600×900, 30 fps; `selection.pam` shows the first frame's mask. `SIDE=-1` views the opposite side, `CLOSE=0` through
`CLOSE=3` frames a wheel, and `PHASE_DEG=90` offsets the physics phase. `MOTION=0` freezes the wheel phase at zero for a
comparison. `SIM=1` settles and accelerates the real physics car (ignoring the speed argument). The camera holds
the body in place for inspection. Headless `splat-racer` screenshots exercise the full frame and depth prepass.

Validation covers shader composition; GPU/CPU centre agreement, no accumulated drift, sorting with a stationary
camera, body/caliper preservation and disabled-rig restoration; idle/reverse/road-speed state; unwrapped phase
interpolation; and continuous full revolutions without direction reversal, retained phase when stopped, and fixed hub/axial position
and Gaussian variance along the axle.

For a labelled 0–60 mph animation, including a 0.5 s rest and a hold at 60 mph:

```sh
ACCEL60=1 CLOSE=1 SHUTTER_SAMPLES=4 cargo run --locked --release --manifest-path crates/splat-engine/Cargo.toml --example wheel_preview -- /tmp/wheel-0-60 180 0 0
```

Acceleration uses real physics wheel states until 60 mph, then advances their phases at a constant 60 mph.
`telemetry.csv` records every displayed frame. The optional shutter averages subframe renders in linear light,
using the same continuously rotating wheel shader with unchanged Gaussian scales. This is an offline preview effect; the live renderer
does not yet accumulate those exposures. Set `SHUTTER_SAMPLES=1` to see the live effect without shutter accumulation.
