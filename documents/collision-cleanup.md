# Level collision cleanup

Reviewed the four playable imports using matched scan/physics overlays and repeatable driving probes. Madras Square has no voxel collision files and is still unavailable for driving.

## Changes

- Replace mixed 20 cm wall columns extruded from one metre below the road to three metres above it with boxes fitted to actual 5 cm occupied voxels in the existing obstacle band. Empty portions of a leaf and gaps below/among occupied surfaces remain empty.
- Remove isolated voxels lacking two face neighbours. Supported facades, thin sheets and posts retain collision. This is conservative scan-noise filtering, not a semantic object classifier.
- Slightly round exposed box edges inside their original bounds (at most 2 cm). Tile cuts stay flat so rounding cannot open a seam.
- Stop floor heightfields from bridging nearby road and high scan surfaces into blocking ramps: reject cells with more than 20 cm of rise and an edge steeper than 1:1. Ordinary ramps, kerbs and supported surfaces remain. Missing faces at steep boundaries are deliberate; visible obstacles retain fitted collision.
- Group detailed obstacles into one BVH-backed compound collider per tile, alongside one floor heightfield. Update the collider debug overlay and wall probe to show/query the geometry used by the game.

## Whole-level proxy volume and streamed collider counts

The counts below are from each fixed review camera's spawn region, with the same streaming radius. Volume reductions describe filled proxy space removed, not a claim that the same fraction of visible objects was deleted. Compound colliders still contain the fitted pieces internally.

| Level | Streamed colliders, before → after | Obstacle proxy volume removed |
| --- | --- | --- |
| saburi-alley-yazd | 2,839 → 26 | 71.1% |
| trompetstraat-delft | 5,682 → 25 | 73.8% |
| nmbu-park-03 | 4,667 → 37 | 76.0% |
| basilica-assumption | 7,080 → 44 | 77.1% |

## Driving observations

The six-second full-throttle/heading-hold probes stop measuring when the hull first receives a significant collision. Their results are not a completed lap or proof that every obstacle on a level is now correct.

Delft's old heightfield hit occurred after 0.32 seconds at about 10 km/h. With the cleanup, the first hit is on fitted obstacle geometry after about 1.77 seconds at 44 km/h. Park still hits a source-supported obstacle at the same point. Basilica's encounter stays near the same point. Saburi's first contact changes from a heightfield face to fitted obstacle geometry; it occurs earlier, so there is no claim of increased unobstructed distance there. Before/after logs accompany this report.

The source images show real street furniture, planting, poles, buildings, parked vehicles and narrow passageways. Those are not all intended to be traversable. Original SOG and voxel files were not edited. Connected scan artifacts can still survive; this cleanup removes the coarse-generation artifacts and isolated speckle, not every uncertain reconstruction.

## Reproduce and validate

From the repository root:

```sh
cargo test --manifest-path crates/splat-engine/Cargo.toml --tests
cargo run --release --manifest-path crates/splat-app/Cargo.toml --example collision_review -- levels/saburi-alley-yazd /tmp/collision-review
CONF=engine.conf HOLD=1 SEQ=W:6 cargo run --release --manifest-path crates/splat-engine/Cargo.toml --example ride_quality -- assets/car/porsche-992-gt3rs-2023.sog levels/trompetstraat-delft
```

The new regression tests cover exact mixed-leaf footprints, real surface/post preservation, isolated-noise rejection, overhang gaps, low-object height, steep floor bridges, tile seam continuity, and bounded broad-phase collider counts. Existing driving, ramps, suspension, handbrake and tire-mark tests remain included. The native application and WebAssembly build were checked.

`level.floor_max_slope` defaults to `1.0`; `0` disables the new steep-face rejection for diagnosis. Restart/reload the level after changing collision settings. `level.wall_height_m` remains for legacy/synthetic columns; imported scenes use their occupied geometry.
