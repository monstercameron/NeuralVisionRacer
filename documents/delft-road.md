# Delft road and shadow refinement

The main street now has a continuous collision corridor spanning world Z -83 to +93 m. Its main width is 5.3 m, smoothly widening to 6.4 m between Z +56 and +72 m to give the whole chassis room through the terminal transition. `specs/road-collision.json` stores a reviewed centreline and grade. Its shoulders extend roughly 0.35 m beyond the original intended lane on each side. This is an authored driving surface fitted to the scan, rather than a claim that the entire scan contains accurate ground voxels.

The collision loader fills missing and elevated/noisy floor samples inside this corridor, using a smooth C1 height curve at full float precision. Existing streamed heightfields sample the curve directly, so the road no longer inherits 5 cm voxel steps or ground-to-roof bridges. Neighbouring tiles use the same world-space curve. A 0.4 m floor apron keeps elevated source samples from pulling boundary mesh cells upward into the driving lane. Beyond the apron, the existing source floor stays in use; fitted obstacles outside the cleared corridor remain active.

Before foliage material splitting, 17,245 fitted obstacle boxes intruding into the road's three-metre clearance band are suppressed. This intentionally clears road furniture and scan debris from the demo lane as well as floating artifacts. Outer building walls and high overhangs are retained. Because fitted boxes have at most 20 cm lateral footprints, overlap filtering can free up to one such footprint beyond a shoulder. Source SOG and voxel files were not edited.

Delft's car shadow now uses a profile multiplier of 1.35 instead of 0.75, with softness 0.14 instead of 0.20. At the default global shadow strength 0.55, the maximum core attenuation setting rises from 0.4125 to 0.7425. Receiver orientation and proxy coverage still modulate the actual pixel result. Sun direction is shared with car lighting and now comes from an offline fit to source-ground brightness and full-scene voxel visibility: azimuth 168 degrees, elevation 32 degrees, world sun (0.1763, 0.5299, -0.8295). This rotates the old direction by approximately 53 degrees in plan, so shadows run mainly along the street instead of across it. Other levels retain their previous lighting profiles. The comparison frames hold the camera and new softness fixed and compare the old versus new intensity settings.

The saved Delft spawn remains (-3.648744, -2.928622, -56.761772), body yaw -166.3618 degrees. Manual reset uses that pose. Collisions recover in place; only the explicit development wreck command exercises the old wreck timer. Out-of-bounds recovery remains separate.

## Validation

- Twelve controlled 72 km/h drives: six full-route runs between Z -80 and +90 m at lateral positions (-1.25, 0, +1.25 m), and six terminal runs between Z +72 and +90 m at positions (-1.8, 0, +1.8 m), both directions. All twelve complete with zero chassis-hit ticks, zero fully airborne ticks, and all four wheels in contact on every measured tick. Maximum body-height deviation from the rig's nominal reference is under 10 cm.
- The terminal bug was a cleanup cutoff at Z +79 m. Beyond it, raw elevated ground and floating source obstacles returned. Before the extension, the terminal probe records 40–55 airborne ticks per run; all three southbound runs fail to complete. The repaired profile clears 1,512 additional fitted boxes and supplies a smooth authored runoff to the grid end. This is deliberately a demo driving surface where the terminal scan is incomplete.
- The drive rig holds horizontal speed and follows the authored centreline; suspension, vertical motion, pitch, roll and contacts remain physical. It is a repeatable surface test, not a claim of autonomous player control or a completed race lap.
- The same rig on original collision exposes missing ground and obstacles: northbound runs fail to complete, and southbound runs record 144–422 airborne ticks and 20–49 hull-hit ticks.
- Regression coverage samples the repaired surface across tile joins and three wheel lanes, and checks floating obstacle removal, exterior wall preservation, road limits/grade continuity, level scoping, saved spawn, and the darker shadow profile.
- Engine suites: 86 tests pass, one preexisting test ignored. Native examples check and release WASM build pass.

## Reproduce

```sh
cargo test --manifest-path crates/splat-engine/Cargo.toml --tests
FROM=-80 TO=90 cargo run --release --manifest-path crates/splat-engine/Cargo.toml --example road_drive -- trompetstraat-delft /tmp/delft-drive.json
FROM=72 TO=90 OFFSETS=-1.8,0,1.8 cargo run --release --manifest-path crates/splat-engine/Cargo.toml --example road_drive -- trompetstraat-delft /tmp/delft-end.json
RAW=1 cargo run --release --manifest-path crates/splat-engine/Cargo.toml --example road_drive -- trompetstraat-delft /tmp/delft-baseline.json
cargo run --release --manifest-path crates/splat-app/Cargo.toml --example road_review -- trompetstraat-delft /tmp/delft-review after
```

## Shadow alignment calibration

`fit_sun` bins reliable source-ground splats into 0.5 m cells, takes median log-luminance, and compares that brightness to ray visibility through the full source voxel scene at 20 cm resolution. A coarse angular search followed by refinement selects the sun direction with the strongest brightness/visibility correlation. This work runs offline; runtime still uses the existing bounded analytic shadow shader.

For Delft, 2,510 road samples produce correlation 0.550 for the selected direction, with 24.3% of sampled points predicted sunlit. The previous direction predicts all sampled points occluded. Correlations remain positive in southern, middle and northern sections (0.391, 0.538, 0.580). Texture albedo, indirect lighting and reconstruction errors mean this is an approximate capture-light fit, not exact lighting metadata. Direction and darkness now match the scan more closely, but full scene relighting is not implemented.

Reproduce the fit:

```sh
cargo run --release --manifest-path crates/splat-app/Cargo.toml --example fit_sun -- trompetstraat-delft /tmp/delft-sun.json
```

The subsequent foliage refinement splits material boundaries before counting road suppression, so current `obstacle_boxes_removed` counts split hard pieces and is not directly comparable with the original box total. Classified vegetation uses non-blocking sensors with small capped drag; see `documents/research/foliage-collision.md`.
