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

## Demo assets and local library

The GitHub Pages demo includes these assets under `docs/`:

| Asset | Creator and source | Licence |
| --- | --- | --- |
| Saburi alley — Yazd, Iran | [parsamahdavi48 on SuperSplat](https://superspl.at/scene/d56e46e4) | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| Porsche 992 GT3RS 2023 | [b2mbiczek on SuperSplat](https://superspl.at/scene/0b1ac6c1) | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |

The published SOG data is retained without downsampling or recompression; the car is packaged as a bundled SOG.
Per-file provenance and checksums accompany the demo assets. The game applies runtime transforms and renders them
with its own effects. These asset licences are separate from the Apache-2.0 project licence and the MIT code notices.

Other scene data and the HDRI remain local and are not redistributed. See [assets/README.md](assets/README.md) and
[levels/README.md](levels/README.md) for the full local asset list and each scene's licence.
