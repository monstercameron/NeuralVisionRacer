# Splatting techniques: comparison for NeuralVisionRacer

Reviewed 26 October 2026. Four requested Luna researchers produced **eight individual technique reports**. The coordinator reviewed them against the actual Rust/WASM + WebGPU renderer and checked the strongest performance claims against primary sources. These are research recommendations, not implemented or browser-benchmarked replacements.

## Recommended order

1. **Improve the existing SOG path first.** The game already has spatial multi-LOD assets, screen-space selection, a bounded pool, fades and global GPU sorting. Preserve those strengths while eliminating redundant work and upload copies. No paper benchmark proves a speedup for this game.
2. **Prototype offline parent LODs or conservative importance pruning.** They can potentially reduce drawn splats while keeping the present renderer. Both need held-out-view quality checks; the published optimized versions require original camera/image data. An SOG-only approximation must be labeled as an adaptation.
3. **Investigate a tile-based compute raster path if profiling justifies a rewrite.** TileGS is a relevant algorithmic lead, but its modest whole-frame CUDA gains are over an already tile-based gsplat baseline. Our global sorted quad architecture is different.
4. **Keep triangle rendering as a separate static-scene experiment.** Opaque geometry has a plausible route to hardware depth rejection and removal of alpha sorting. Its headline mesh FPS uses a different visual/training setup; existing SOG assets are not a lossless input.

## Individual reports

| Technique | Main opportunity | Existing SOG asset path | Recommendation | Report |
|---|---|---|---|---|
| Hierarchical 3D Gaussians | Better coarse Gaussian parents and screen-space cuts | Offline merge prototype possible; optimized parents need training views; falloff encoding is unresolved | Prototype one static region | [Report](hierarchical-3d-gaussians.md) |
| LightGaussian | Fewer splats through significance pruning/recovery | Pruning-inspired proxy possible; published recovery/distillation needs source views | Prototype mild environment pruning; protect car/wheels | [Report](lightgaussian-global-pruning-sh-distillation.md) |
| TileGS | Shorter tile-local traversal with coarse bins and order repair | Existing attributes can remain, but color/depth rendering needs a substantial compute rewrite | Focused diagnostic prototype | [Report](tilegs-tile-local-depth-binning.md) |
| Triangle Splatting | Opaque triangle export using conventional hardware rasterization | New training/export or lossy reconstruction; no direct SOG reinterpretation | Separate static opaque-scene prototype | [Report](triangle-splatting.md) |
| StopThePop | More view-consistent transparency ordering | Assets can remain; new hierarchical compute rasterizer required | Defer for FPS; consider if popping is a priority | [Report](stopthepop-hierarchical-resorting.md) |
| Mini-Splatting | Better spatial distribution and much lower learned count | Requires source images/poses and reconstruction; preserve wheel semantics separately | Defer current assets; consider future level authoring | [Report](mini-splatting-reconstruction-and-simplification.md) |
| Octree-GS | Trained LOD-structured neural anchors | New representation/training/MLP path; not an incremental SOG selector | Defer wholesale adoption | [Report](octree-gs.md) |
| 2D Gaussian Splatting | Surface geometry, normals and mesh extraction | New planar representation/training or lossy adaptation | Defer as an FPS optimization | [Report](2d-gaussian-splatting.md) |

## How to interpret the headline numbers

Triangle Splatting advertises more than 2,400 FPS at 1280×720 in a conventional mesh renderer. The authors explicitly say that game-engine demonstration has no shaders and was not trained for that visual fidelity. It is not comparable to our shaded, streamed splat game. [Authors' project page](https://trianglesplatting.github.io/).

TileGS reports 1.44× raster-kernel speedup but only 1.069× total-frame speedup on RTX 4090 and 1.094× on RTX 1000 Ada over CUDA gsplat, with very small reported quality-metric differences. Those figures motivate a prototype; they do not predict browser performance. [Paper](https://arxiv.org/abs/2609.03613).

LightGaussian and Mini-Splatting reduce model count after importance analysis and/or reconstruction/recovery training. Their larger speedups include changed assets and quality tradeoffs. They do not demonstrate a free renderer-only gain on our existing car and downloaded SOG levels. See their separate reports for hardware, quality and data requirements.

## Common acceptance gates

Use the same browser, viewport, camera paths, effects, car/wheel animation and quality target. Compare complete GPU frame distributions, residency/upload bytes, sort input, cold-load behavior and transition peaks. Inspect thin geometry, car spokes/silhouette, shadows, transparency and motion; inspect held-out views, not just training cameras. Do not add overlapping pass timestamps as exclusive costs or equate CUDA paper FPS with WebGPU FPS. Review the actual reference-code licenses before taking dependencies; the math and the software are separate reuse questions.

The separate [SwitchSplat assessment](../../research-switchsplat.md) concerns learned open/closed object states rather than a renderer speed algorithm. It is relevant to future door/hood/trunk interactions, with its own training/export work, and does not replace continuous wheel kinematics.
