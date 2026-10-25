# Third-party notices

Splat Racer is licensed under the Apache License 2.0 (see [LICENSE](LICENSE)). It includes or depends on the
following third-party material.

## Code and data bundled in this repository

| Component | Where | Licence | Notes |
| --- | --- | --- | --- |
| PlayCanvas engine WGSL shaders | `crates/splat-wgsl/shaders/` | MIT | Copied from the PlayCanvas engine. See [its notice](crates/splat-wgsl/NOTICE.md). |
| Blue-noise texture | `crates/splat-render/assets/blue_noise_32_rgba8.bin` | MIT | Extracted from the PlayCanvas engine. See the note in that folder. |
| PlayCanvas engine reference source | `third_party/playcanvas-engine/` | MIT | Pinned subset; `third_party/upstream-lock.json` records every file. |
| splat-transform reference source | `third_party/splat-transform/` | MIT | Pinned subset (same lock file). |
| supersplat-viewer reference source | `third_party/supersplat-viewer/` | MIT | Pinned subset (same lock file). |

## Rust dependencies

Each crate's `Cargo.lock` pins its dependencies, all of which are published on crates.io under permissive licences
(MIT, Apache-2.0 or similar). To list them with their licences, run `cargo license` (after installing it) in each
crate, or `cargo deny check licenses`.

## Assets not in this repository

The repository does not contain scene data. These are not redistributed here; see [assets/README.md](assets/README.md)
and [levels/README.md](levels/README.md) for what each one is and its licence.
