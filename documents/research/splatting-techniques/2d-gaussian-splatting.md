# Technique report: 2D Gaussian Splatting (2DGS)

**Research focus:** surface-aligned Gaussian disks as a nearby alternative to the engine’s volumetric 3D Gaussian representation.
**Recommendation:** defer as a speed optimization; consider only as an offline surface reconstruction/mesh-extraction route or a quality-driven research experiment.

## Representation and algorithm

**“2D Gaussian Splatting for Geometrically Accurate Radiance Fields”** by Huang et al. is a SIGGRAPH 2024 paper (arXiv:2403.17888). 2DGS replaces volumetric 3D ellipsoids with oriented planar elliptical disks (surfels). Its key technical feature is perspective-correct ray/splat intersection: each pixel ray is intersected with the transformed disk plane, the resulting local disk coordinate evaluates a 2D Gaussian, and the renderer sorts screen-space bounds into tiles and alpha-composites front-to-back. The paper adds depth-distortion and normal-consistency losses during training to align disk surfaces and improve surface geometry, and can reconstruct a triangle mesh by fusing training-view depth maps with TSDF.

Primary sources: [paper (arXiv HTML)](https://arxiv.org/html/2403.17888v3), [project page](https://surfsplatting.github.io/), [official implementation](https://github.com/hbb1/2d-gaussian-splatting).

## Performance and quality evidence

The authors describe 2DGS as real-time and report a 30–40% training speed improvement after fusing CUDA operators in their repository. Their paper’s quantitative evaluation on DTU is reconstruction/training time (e.g. 5.5 minutes for 15k steps and 10.9 minutes for 30k on a single RTX 3090), not a claim that it renders faster than 3DGS in a WebGPU game engine. The paper says all experiments use one RTX 3090 and implementation uses custom CUDA kernels. Do not treat reconstruction speed as runtime FPS, and do not transfer CUDA timings to this project.

The distinct value is surface accuracy, stable depth/normals and mesh extraction. The paper reports better DTU Chamfer distance than 3DGS (mean 0.80 vs 1.96 for the 30k-step setting), while Mip-NeRF360 appearance metrics are close but slightly lower than 3DGS on aggregate (e.g. mean PSNR 27.03 vs 27.20 and LPIPS 0.223 vs 0.214 in its appendix). The authors identify limitations on transparent surfaces and holes in high-light-intensity regions. Sources: [paper](https://arxiv.org/html/2403.17888v3), [official repo notes](https://github.com/hbb1/2d-gaussian-splatting).

## Fit to NeuralVisionRacer

The engine’s SOG V2 content stores 3D Gaussian means, rotations, scales, base color, SH labels/centroids and codebooks as compressed WebP streams. Current runtime draws indexed Gaussian quads, sorts centers globally with a GPU radix sort, and uses a depth prepass plus color blending. Largest level occupancies are about 2.35M Saburi and 5.15M Delft residents; the car has 648,207 Gaussian splats. The existing renderer also has wheel-center deformation, SH/view-dependent highlights, shadow proxy, streaming LOD and chunk fades. See [performance.md](../../performance.md), [optimization session](../../optimization-session-2026-10-26.md), [SOG format loader](../../../crates/splat-render/src/sog.rs), and [renderer scene](../../../crates/splat-render/src/scene.rs).

### Asset compatibility and runtime feasibility

- Existing SOG V2 cannot be reinterpreted as 2DGS. Each Gaussian would need a constrained planar covariance/orientation and its angular/appearance representation retrained or otherwise adapted. Conversion that simply collapses an existing covariance’s smallest axis may be possible as a lossy experiment, but does not apply the paper’s geometry regularization and is not an equivalent trained result. A quality-faithful path needs original posed images and new offline training/export.
- The official renderer depends on CUDA custom kernels and its viewer uses the Gaussian Splatting ecosystem. At runtime, one could implement ray-plane intersection, Gaussian evaluation, sorting, and alpha composition in Rust/WGSL/WebGPU; there is no CUDA requirement in the runtime design, but it would be a new renderer, storage schema, culling/tile assignment and shader pipeline. A regular mesh extracted from 2DGS could instead use ordinary WebGPU triangles, but that is a different path and can bake/freeze appearance.
- **Sorting/overdraw:** 2DGS remains translucent Gaussian compositing. The paper still sorts by center depth and tile-overlap and integrates front-to-back. It therefore does not remove this engine’s primary global order requirement; it could change projection math and surface consistency, but no paper result establishes lower overdraw. Tile-binned sorting is an architectural alternative requiring separate investigation, not a free speed benefit from switching the primitive.
- **LOD/memory:** 2DGS remains one disk per primitive and retains per-element attributes. It does not intrinsically reduce primitive count, texture payload, or streamed LOD residency. The paper’s model/storage numbers are research dataset artifacts; the paper does not show that SOG-to-2DGS cuts this engine’s asset bytes. Existing SOG LOD would need retraining/export per level and a compatible 2DGS chunk format.
- **Car and lighting:** the disks have explicit normals, which could be useful for surface shading, shadow maps and geometry extraction. However 2DGS’s novel-view appearance is still alpha-weighted learned color, and its code paths are designed for reconstruction/rendering, not this game’s moving body/wheel rig, car lighting, shadow proxy, or dynamic relighting. The paper’s mesh extraction can yield geometry for downstream mesh shading, but appearance fidelity must be measured after export. Treat animated-wheel suitability as unproven.
- **Quality risks:** surface alignment should improve depth/normal coherence over 3DGS, but surfaces are still disconnected surfels rather than guaranteed topology; paper notes transparency and bright-region artifacts. Planar support may lose volumetric robustness for floaters and unstructured scenes. Thin surfaces viewed grazing-angle require a low-pass fallback in the paper. The current car’s specular effects and per-splat SH are not automatically preserved.

### Dependencies and license

The official 2DGS repository implementation builds custom CUDA kernels and inherits the Gaussian Splatting research license for core components. That license restricts use to research/evaluation and prohibits commercial use without prior explicit permission. This should not be copied into a game runtime without a code/license review. A clean Rust/WGSL implementation of the published math may avoid taking a dependency on that code, but dataset/model licensing and paper attribution still need review. The engine crate itself is Apache-2.0 AND MIT; that does not relicense upstream assets or training code.

## Smallest useful prototype and acceptance gates

1. Create a standalone WebGPU shader prototype for one small test surface using a hand-authored set of oriented disks. Implement ray-plane intersection and the 2D Gaussian alpha function, using simple center-depth sorting; do not change production SOG decoding or the shipping renderer.
2. Compare the same surface rendered as the current 3DGS-style quads and 2DGS disks at equal primitive count and screen size, then capture GPU spans, draw/sort costs, pixel coverage/overdraw and memory. This isolates projection/shading cost from the massive retraining question.
3. Only if speed is competitive and geometry quality is valuable, train/export one static environment subset from posed images, convert it to a proposed WebGPU asset format, and compare visual quality and asset/resident bytes using the same camera and browser benchmark.

**Success gate to expand:** produce an in-browser end-to-end gain beyond measurement noise without increasing sort/overdraw or memory enough to erase it; preserve accepted appearance over multiple views and at grazing angles; establish a reproducible LOD/export path; and prove WebGPU-only operation. If the target is geometry/shadow/collision quality rather than FPS, score that separately and compare the extracted mesh pipeline against the existing voxel/Rapier collision and shadow paths.

**Recommendation:** **defer for runtime FPS work.** The technique primarily improves surface reconstruction, not sort/overdraw/LOD cost, and its paper provides no relevant WebGPU speed claim. Prototype only if explicit geometry becomes a project objective or if an offline comparison is useful for surface/shadow/collision quality.

## Source snapshot

Paper evidence is arXiv:2403.17888v3 (22 February 2025), published at SIGGRAPH 2024. Official repository `hbb1/2d-gaussian-splatting` was at `f3e3b9fa67bbd1c75e05167ff37391d8dab2a678` (`HEAD` on 2026-10-26). Its README notes CUDA kernels, a Web-based viewer link, and the 30–40% training speedup from fused operators; those are source metadata, not an evaluated Rust/WASM/WebGPU renderer.
