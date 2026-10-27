# StopThePop: hierarchical per-pixel resorting

**Research date:** 2026-10-26
**Scope:** rasterization, sorting, and overdraw for the Rust/WASM + WebGPU/WGSL renderer. This is a paper/architecture assessment, not a measured browser result.

## Recommendation

Treat StopThePop as a view-consistency and correctness candidate, not as the lead performance optimization. It addresses a real limitation of global mean-depth sorting, but its published full-size 3DGS comparison is approximately 4% slower on average, and the official implementation is a CUDA compute rasterizer. A browser port would be a substantial replacement for the current indexed-quad render path. It might complement another optimization if order artifacts are visible during driving; it is not an obvious path to fewer current GPU operations.

**Inference:** For the current title, the meaningful acceptance target is less view-dependent popping at moving-camera angles without slowing complete browser frames. Improvements in paper sort-error/view-consistency metrics do not establish value for the reconstructed driving levels.

## Method

StopThePop replaces a single global center-depth order with hierarchical per-pixel ordering. The pipeline computes a per-ray depth estimate from the point of maximum 3D Gaussian contribution along that ray, conservatively culls Gaussian/tile pairs, uses a coarse tile ordering, then locally resorts candidates in a small window at 2x2-tile and per-pixel levels. The paper's selected queue sizes are eight for each 2x2 tile and four per pixel, with work distributed in queues. Culling and resorting are interleaved; this avoids the impractical cost of fully sorting thousands of candidate splats separately for every pixel.

The motivation is 3DGS's approximation: the global depth is the projection of the Gaussian mean onto the view direction, which is independent of pixel ray. That is often stable under translation but can change relative order abruptly as the camera rotates. StopThePop reports eliminating popping in tested cases, while also acknowledging its approximate resort does not guarantee correct order for complex geometry and still ignores volumetric overlap.

## Paper evidence and limits

Radl et al., *StopThePop: Sorted Gaussian Splatting for View-Consistent Real-time Rendering*, ACM TOG 43(4), Article 64, July 2024. The paper's performance measurements are Full HD, NVIDIA RTX 4090, CUDA 11.8, camera paths interpolated from COLMAP poses, averaged over four runs. For approximately 2.98 million Gaussians, original 3DGS is 4.797 ms and the fully optimized StopThePop configuration is 4.986 ms: about 4% slower. With opacity-decay training and about half as many Gaussians, it reports 2.276 ms for 3DGS versus 2.976 ms for StopThePop, while emphasizing improved view consistency. A per-scene comparison is mixed: StopThePop versus original 3DGS is 3.399 vs 2.574 ms on Bonsai (slower), and 4.225 vs 5.548 ms on Train (faster). Its tile culling reduced approximate sort entries from 4.36M to 1.72M on Bonsai and 10.2M to 2.85M on Train. The paper's own full per-pixel sort baseline is over 100x slower; the hierarchy is its essential contribution.

These are CUDA native results on a high-end desktop GPU, not WebGPU, WASM, integrated graphics, or this renderer. Its 3DGS input and output path are not a direct match for this engine's SOG decoded streams, custom fragment shading, depth passes, or scene composition. It does not establish a speedup for the current globally sorted quad draw.

## Fit with this renderer

The renderer currently radix-sorts each resident scene's splat indices by view depth (five 4-bit passes over the high 20 key bits), then issues indexed quad draws. It merges level chunks for one level ordering, while car order is independent. It also performs separate level and car solid-core depth prepasses: each color draw tests against the other object's depth but does not depth-write, allowing each object's own blending order to remain intact. Car shading includes SH, clearcoat/specular and sunlight effects; wheel centers are deformed in compute. Level LOD transitions crossfade via per-splat opacity. The performance notes identify splat count, coverage, and transparency overdraw as major costs, and explicitly note overlapping GPU timestamp spans must not be summed as exclusive costs.

- **Assets:** SOG V2 can remain unchanged in principle; the algorithm needs decoded Gaussian centers/covariance, opacity and shading attributes, not a new training format. **Inference:** the existing engine shaders already recover covariance/shape and opacity for quad expansion, but a compute renderer would need those attributes available in compatible buffers and an explicit projected representation. Reusing SOG does not mean reusing the existing draw pipeline.
- **WebGPU feasibility:** Per-ray depth evaluation, staged queues and shared tile work are expressible in WGSL compute with storage buffers and workgroup memory. However, the paper is specialized for CUDA's cooperative execution/memory hierarchy. WebGPU does not promise CUDA warp-level primitives or shared-memory behavior; WebGPU atomics are integer atomics and workgroup barriers have strict uniform control-flow requirements. The current renderer already stays within the portable eight-storage-buffer limit, so extra queue/geometry/depth bindings must be budgeted or repacked. Adapter workgroup storage, max workgroup size and storage binding limits vary. No native-only assumption is safe in browser.
- **Ordering/transparency:** Local per-ray resort changes blending relative to the current 2D Gaussian approximation and therefore changes pixels by design. Its approximate order may still fail in complicated overlaps. Validate against image sequences under slow yaw/translation, not only a still screenshot.
- **Multi-object composition:** The paper orders one Gaussian scene. It does not define this app's independent car/level composition contract. **Inference:** apply the same pipeline separately to the merged level and car to preserve cross-object depth tests; never merge car and level resort lists without explicitly reproducing the current “test against other object, never self” behavior.
- **Wheel motion:** Wheel centers change with the animated pose. The current sort is updated from deformed centers and/or model transforms. A projected-tile hierarchy must be rebuilt or updated for moving wheels each relevant frame; the static level may be cacheable for a fixed view only, but the view changes during play. **Inference:** higher-frequency dynamic resort/build work could erase any draw-side gain.
- **LOD/crossfades:** Fade opacity must participate in tile culling and depth prepass semantics; otherwise tile membership and visible solid cores can disagree during transitions. Sort/cull data must be invalidated on stream writes/releases, as current sort order is.
- **Shadows/depth/post:** The existing analytic car shadow proxy is evaluated inside splat vertex shading; a replacement must preserve its attenuation and clearcoat occlusion. Current depth prepasses produce depth textures consumed by DOF, post effects and car sharpening/occlusion. A compute color result alone is insufficient: the replacement needs to emit compatible level/car depth (including the current alpha clips) or retain hardware depth passes, which likely retains some quad work. Postprocessing itself need not change if these outputs remain identical in dimensions/format/meaning.

## Licensing and implementation source

The official [r4dl/StopThePop repository](https://github.com/r4dl/StopThePop) identifies its StopThePop rasterizer as a CUDA hierarchical rasterizer and states that most projects use the Gaussian-Splatting License, with its StopThePop header files under MIT and PoppingDetection under MIT. Do not port source code until the exact files and license terms are audited; a clean algorithmic reimplementation still needs attribution/citation review. The paper page identifies ACM TOG 43(4), Article 64 and DOI 10.1145/3658187.

## Smallest falsifiable prototype

Implement a diagnostic-only compute path for a fixed, tiny crop of one resident scene and one camera, without replacing the production draw. Use one small tile and bounded candidate list; compare baseline ordered blending and a 4-entry local resort (then 8 if register/workgroup budget permits). First test static level splats, then one animated wheel subset. Capture exact output and slow camera yaw sequence.

**Pixel gates:** (1) render identical candidate list and shader math to current path; report max/mean channel error and SSIM; (2) document expected pixel deltas caused specifically by resorting; (3) verify level/car depth-pass fixtures and LOD crossfade endpoints; (4) ensure no NaN/overflow for flat/degenerate covariance.
**Timing gates:** time complete browser frames at the documented 1280x720 fixed LOD harness, keeping GPU pass spans separate because they can overlap. The prototype only proceeds if the full-frame median improves by at least 5% on both Saburi and Delft and p95 does not regress by more than 3%, with the above image and depth gates passing. Also record queue-build/sort operations and memory bytes; CUDA paper milliseconds are not a pass criterion.

## Primary sources

- [StopThePop paper (arXiv 2402.00525)](https://arxiv.org/abs/2402.00525)
- [Official author repository and licensing overview](https://github.com/r4dl/StopThePop)
- [ACM DOI / publication record](https://doi.org/10.1145/3658187)
- [Project's browser benchmark and profiler guidance](../../performance.md)
