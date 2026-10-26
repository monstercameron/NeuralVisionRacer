# Foliage contact research and implementation

## Findings

Rapier distinguishes solid colliders, which resolve contact forces, from sensors, which detect overlap without blocking. Reducing friction or restitution on a solid voxel would still leave its nonpenetration constraint in place, so it would still behave like a wall. The appropriate lightweight contact representation is a sensor plus an explicitly limited dissipative response. [Rapier collider documentation](https://rapier.rs/docs/user_guides/rust/colliders/).

Interactive foliage commonly separates touch detection from a flexible spring/damping response. Unreal exposes foliage stiffness, damping, touch impulse scales and maximum force/impulse limits. This supports treating flexible growth differently from structural geometry; it does not supply physical constants for our scanned plants. [Unreal InteractiveFoliageActor](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Foliage/AInteractiveFoliageActor).

Rapier supports applying impulses to dynamic bodies and warns that ordinary added forces persist until cleared. A per-substep impulse is convenient here: it represents only the current foliage overlap and stops immediately on exit. [Rapier rigid-body forces and damping](https://rapier.rs/docs/user_guides/rust/rigid_bodies/).

The implemented response is a gameplay approximation of compliant foliage contact. It does not solve deforming botanical meshes or animate the captured splats bending. Its purpose is to eliminate rigid blocking and remove a small, predictable amount of vehicle energy.

## Material refinement

The scan voxels have occupancy but no semantic materials. The offline `bake_foliage` example uses matching source SOG colour samples in 25 cm world-space cells. It restricts candidates to existing obstacle/floor geometry and rejects faint or oversized splats. A candidate needs at least six green samples, at least 60% green support, more than 1.4 cm of smallest-axis PCA thickness, two neighbouring candidate cells, and a location more than 30 cm above the nearby floor estimate. Runtime uses only the small baked map, with no colour classification in the frame loop.

The geometry test reduces false positives from flat painted surfaces. Visual review exposed a noisy green Delft garage door that still passed the geometric heuristic, so `specs/foliage-overrides.json` protects its reviewed extent. The final overlay confirms that door remains rigid. Classification is conservative inference, not ground-truth segmentation: brown trunks, planters, masonry and unknown material remain solid. Some uncertain vegetation can therefore remain rigid and may need additional reviewed material cells.

Final baked cells: Delft 166; NMBU Park 26. Basilica and Saburi produced no confident cells and retain existing collision. Delft now has 713 soft obstacle pieces and rejects 549 leaf-top floor samples; Park has 92 soft pieces and rejects 180 leaf-top samples. These are counts of fitted pieces/samples, not trees. Original SOG and voxel assets are unchanged.

Existing obstacle boxes split at material-cell boundaries. A leaf touching a facade therefore cannot soften the entire original box. Soft pieces form one sensor compound per streamed tile, sharing the tile's existing fixed body and removal lifecycle. Solid pieces retain their previous collision. Leaf tops no longer form elevated hard floor heightfields; the authored Delft road remains continuous.

## Energy response

For horizontal speed `v`, overlap time `dt` and drag constant `k`, the uncapped update is `v_next = v / (1 + k*v*dt)`. The loss is additionally limited to `a_max*dt`, then applied as a centre-of-mass impulse. Defaults are `k = 0.002 /m` and `a_max = 0.8 m/s²`: approximately 2% speed loss or 4% horizontal kinetic-energy loss through 10 m of continuous foliage at 72 km/h.

The response never reverses the velocity, adds energy or changes vertical speed. It has no rebound/penetration correction or torque. Multiple overlapping sensor compounds produce one bounded response, so voxel density and tile seams cannot multiply the drag. Wheel raycasts exclude sensors, preventing foliage from becoming suspension ground. Normal solid contacts remain separate from foliage, so touching vegetation does not generate a chassis crash impulse.

`perf` now reports a foliage section with active overlap count, cumulative energy removed in joules, deceleration, material resolution, soft piece count, rejected floor sample count and currently streamed sensor compounds. Manual reset clears contact/energy telemetry. The collision debug overlay uses cyan for floor, magenta for rigid obstacles and green for soft foliage.

Configuration: `level.foliage = true`, `physics.foliage_drag_per_m = 0.002`, `physics.foliage_max_decel_mps2 = 0.8`. These collision/physics settings require level reload after editing. Source classifier and reviewed overrides are offline inputs; rebake and rebuild WASM to change the material map.

## Validation

- 93 engine tests pass; one preexisting test ignored. New cases cover substep stability, no energy gain/reversal, capped deceleration, unchanged vertical motion, planar green paint versus volumetric foliage, mixed material boundaries, rigid-neighbour preservation, sensor-only leaf shapes, and removal of leaf-top floor support.
- A 1,500 kg test body entering the foliage fixture at 20 m/s exits the measured interval at 19.802 m/s. Removed energy is 5,911 J; there are zero hard-contact observations. Three coincident sensors produce exactly the same speed/energy result as one. The solid version produces nine hard-contact observations and stops/rebounds before crossing.
- A real drivetrain/suspension integration test places a foliage sensor over all four wheel rays. All wheels remain supported by the road, with less than 2 cm body-height deviation and no crash impulse.
- Six controlled Delft full-route probes at 72 km/h, three lane offsets and both directions, still complete without chassis hits or airborne frames. The rig guides horizontal speed/heading; this verifies the surface rather than autonomous driving or unrestricted off-road coverage.
- Matched source/physics overlays reviewed for Delft and Park, including the protected painted door. Native examples check and release WASM build pass.

## Reproduce

```sh
cargo run --release --manifest-path crates/splat-app/Cargo.toml --example bake_foliage -- specs/foliage-materials.json
cargo test --manifest-path crates/splat-engine/Cargo.toml --tests
cargo run --release --manifest-path crates/splat-app/Cargo.toml --example foliage_review -- trompetstraat-delft /tmp/foliage
FROM=-80 TO=90 cargo run --release --manifest-path crates/splat-engine/Cargo.toml --example road_drive -- trompetstraat-delft /tmp/foliage-drive.json
```

The Pages packaging script also includes the shader-menu module introduced in the preceding change, ensuring the browser entry's new import is copied on a future site build. No site deployment was performed.
