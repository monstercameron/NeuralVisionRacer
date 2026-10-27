# SwitchSplat assessment for NeuralVisionRacer

**Research date:** 26 October 2026
**Recommendation: defer for performance work; prototype only as a separate articulated-asset feature.**

## Executive finding

SwitchSplat is a learned scene representation for independently toggling discrete object states, such as a car door being open or shut. It is not a SOG compression method, level-of-detail scheme, sorter, or faster Gaussian rasterizer. Its base-plus-slots decomposition addresses a different problem from the current measured game bottleneck: preserving several captured states of changing objects and composing unseen state combinations at render time. The paper reports novel-view quality gains, not a transferable browser-frame-time improvement. ([paper](https://arxiv.org/html/2610.24812), [project page](https://switchsplat.github.io/))

For the authorized Rust/WASM + WebGPU optimization scope, do not integrate SwitchSplat now. Consider a small, isolated experiment if the game specifically needs learned open/closed geometry for a door, hood, or trunk. Continuous steering/spinning wheels and Rapier collision remain separate systems; SwitchSplat does not supply their kinematics or physical constraints.

## What the method actually does

Each Gaussian receives a learned categorical membership over a shared static base and two channels (off/on) for each binary object slot. At inference, the requested state combines the base and the selected state per slot into an effective opacity. This lets state-specific geometry occupy different locations; the approach does not simply rigidly rotate one object mesh or reuse the existing Gaussian set unchanged. ([paper, method](https://arxiv.org/html/2610.24812#S3), [project method summary](https://switchsplat.github.io/#method))

The training input is a captured image sequence with camera calibration/reconstruction and approximate per-object state windows. The project preprocesses new fisheye captures through its FullCircle/COLMAP workflow; slot windows are manually authored or initialized by a VLM and refined during optimization. The documented main recipe uses 30,000 optimizer steps, half-resolution images, batch size four, and the 3DGUT renderer; a 3DGS variant is available. This is a retraining pipeline, not a converter that annotates an existing SOG. ([README](https://github.com/theialab/switchsplat#usage), [main training config](https://raw.githubusercontent.com/theialab/switchsplat/main/configs/apps/colmap_switchsplat.yaml), [paper](https://arxiv.org/html/2610.24812))

The paper's comparison table shows SwitchSplat improving average PSNR over the evaluated static/dynamic methods on its capture benchmark. That supports a reconstruction-quality claim for those scenes and metrics. It does not establish reduced splat count, memory, load time, sort cost, or frame time for this game. The official project page has an interactive browser viewer and says the scenes run live with WebGPU, which shows browser rendering is possible in a separate viewer. It does not establish that the training repository exports a ready-to-load SOG or exposes a reusable WebGPU runtime. ([paper results](https://arxiv.org/html/2610.24812#S4), [project page](https://switchsplat.github.io/))

## Compatibility with this game

The game loader expects SOG V2 bundles: `meta.json` plus `means_l`, `means_u`, `quats`, `scales`, `sh0`, and SH label/centroid WebP streams. The renderer decodes the SOG codebooks, shares decoded centres for sorting and wheel deformation, merges level chunks, and uses a global depth order because chunks overlap. Its wheels are rigidly transformed from selected wheel splat indices before sorting; game physics and collision data are separate. ([local `sog.rs`](../crates/splat-render/src/sog.rs), [local wheel path](../crates/splat-render/src/wheel.rs), [local wheel rig](../crates/splat-engine/src/wheel_visual.rs))

No direct compatibility path was found in SwitchSplat's documented usage or the inspected repository entry points. The training output is a PyTorch checkpoint; the repository's generic 3DGRUT config has optional PLY/USD export, not an identified SOG V2 export. A custom conversion would need to evaluate each requested slot state, map state-conditioned opacity/membership into a runtime representation, preserve Gaussian geometry/appearance and SH, and then encode the game’s WebP/codebook SOG streams—or add a new stream/runtime path. This is an engineering inference from the inspected interfaces, not a claim that conversion is impossible. ([training README](https://github.com/theialab/switchsplat#usage), [3DGRUT export config](https://raw.githubusercontent.com/theialab/switchsplat/main/configs/base_gs.yaml), [render entry point](https://raw.githubusercontent.com/theialab/switchsplat/main/threedgrut/render.py))

A plausible state-rendering prototype could precompute one selected Gaussian set per discrete state and feed the active union into the existing global sorter. That would need explicit validation: filtering state-inactive splats may reduce active GPU work, but duplicating base/state geometry or retaining all groups can instead increase asset size, resident memory, upload traffic, and sorting work. In either case the game still needs its usual LOD and sort behavior. The learned per-Gaussian membership adds state data; SwitchSplat itself does not define spatial LOD, progressive streaming, voxel collision, or Rapier colliders. These are engineering inferences from its stated representation and method scope.

The state model is binary and capture-timeline based. It does not directly represent continuous wheel rotation, steering, suspension travel, or a rigid articulated transform. Using it for the existing car would require new multi-view capture showing each desired discrete state, and it could disrupt the current wheel index/rigid-body fitting assumptions. The paper explicitly frames physics-based priors/simulation as future work. ([paper limitations](https://arxiv.org/html/2610.24812#S5))

The current project already has SOG-compressed LODs and a 648,207-splat car. The optimization session reports roughly 1.5M level splat slots in its static benchmark and 2.35M resident level splats (plus the 648,207-splat car) in settled Saburi streaming. Its stated dominant steady-state costs are splat count, screen coverage, and transparency overdraw; SwitchSplat does not target those. Existing work also removed redundant position GPU textures, saving 48.9 MB for Saburi and 73.1 MB for Delft with identical settled frames, while measured frame-time gains remained unproven. A representation change should clear a high bar against these measured constraints. ([`performance.md`](performance.md), [`optimization-session-2026-10-26.md`](optimization-session-2026-10-26.md))

## Runtime, platform, and dependency constraints

The research/training code is not a portable browser component. The install scripts call for CUDA 12.x, compile CUDA code for the local GPU, install PyTorch CUDA wheels and CUDA extensions, and initialize tiny-cuda-nn; the repository also includes OptiX and renderer/tracer submodules. Its Python dependency set includes Torch, torchvision, `nvidia-ncore`, and GPU renderer packages. This would add an offline NVIDIA training dependency rather than a drop-in Rust/WASM component; the trained representation would still need an exporter and a compatible runtime implementation. CUDA training does not require CUDA at game runtime: the official viewer is separately advertised as WebGPU. ([install script](https://raw.githubusercontent.com/theialab/switchsplat/main/install.sh), [dependency setup](https://raw.githubusercontent.com/theialab/switchsplat/main/install_env_uv.sh), [pyproject](https://raw.githubusercontent.com/theialab/switchsplat/main/pyproject.toml), [submodules](https://raw.githubusercontent.com/theialab/switchsplat/main/.gitmodules))

The repository declares Apache-2.0 and includes a NOTICE describing its 3DGRUT and FullCircle basis. It also contains third-party code/submodules; attribution and per-component licenses must be reviewed before redistribution. The published SwitchSplat dataset is CC BY 4.0, which is separate from the software license. Use of any captured vehicle imagery/assets for a shipped game needs its own rights review. ([LICENSE](https://github.com/theialab/switchsplat/blob/main/LICENSE), [NOTICE](https://raw.githubusercontent.com/theialab/switchsplat/main/NOTICE), [dataset license](https://huggingface.co/datasets/xieyizheng/switchsplat#license))

## Smallest useful prototype and gates

**Prototype only if an interactive binary car part is an actual product requirement.** Use a new, self-owned capture of one part (for example, hood closed/open), multiple viewpoints and stable images of both states. Train outside the game repository on a compatible CUDA machine. Export a tiny single-slot test asset through a custom offline converter; keep the production SOG and renderer untouched until it proves out. The prototype should produce two state variants sharing the unchanged base, carry only the state-selection data needed at runtime, and use the game's WebGPU renderer/global sort.

Success gates:

1. **Representation:** converter emits a valid small asset; both states render without ghosting or missing interior surfaces from held-out views.
2. **Visual integration:** state toggle preserves the existing SH appearance, car lighting/clearcoat/shadow treatment, wheel animation, and scene sorting; compare full-frame images at fixed settings.
3. **Resource cost:** report active/resident splat counts, SOG bytes, GPU bytes, upload bytes and sort input counts against the equivalent static-car baseline. Do not accept a state asset that increases ongoing costs without a user-visible feature benefit.
4. **Platform:** run in the production browser WebGPU path with no CUDA/native dependency at runtime; preserve current LOD, loading and collision behavior.

A single-slot offline conversion experiment is roughly **medium-to-high complexity**: SwitchSplat training is ready-made if a suitable capture and CUDA host exist, but model export/format mapping and game renderer integration are custom. Risks are state leakage/ghosting, larger assets, appearance mismatch with existing car lighting, lack of smooth transition geometry, and license/asset provenance. Unknowns include the exact trained car splat count, output checkpoint-to-export path for SwitchSplat-specific fields, and whether its official viewer uses a compact partitioned representation that could be reused. Resolve those in the prototype rather than assuming favorable memory or performance.

## Evidence limits

This was a source/documentation review only. I did not install dependencies, execute SwitchSplat, train a model, inspect generated model files, benchmark either renderer, or verify the official viewer's internal implementation. Browser/WebGPU feasibility comes from the authors' project-page claim; SOG conversion and runtime-cost observations above are explicit engineering inferences. The paper's reported PSNR applies to its benchmark capture dataset and is not evidence of NeuralVisionRacer performance.

## Primary sources

- [SwitchSplat repository and README](https://github.com/theialab/switchsplat)
- [SwitchSplat paper (arXiv HTML)](https://arxiv.org/html/2610.24812)
- [SwitchSplat project page and WebGPU viewer](https://switchsplat.github.io/)
- [Training configuration](https://raw.githubusercontent.com/theialab/switchsplat/main/configs/apps/colmap_switchsplat.yaml)
- [CUDA installation script](https://raw.githubusercontent.com/theialab/switchsplat/main/install.sh)
- [Python dependencies](https://raw.githubusercontent.com/theialab/switchsplat/main/pyproject.toml)
- [3DGRUT export configuration](https://raw.githubusercontent.com/theialab/switchsplat/main/configs/base_gs.yaml)
- [Repository license](https://github.com/theialab/switchsplat/blob/main/LICENSE) and [NOTICE](https://raw.githubusercontent.com/theialab/switchsplat/main/NOTICE)
- [SwitchSplat dataset card/license](https://huggingface.co/datasets/xieyizheng/switchsplat)
