# Car splat from generated views — feasibility

Researched October 24, 2026. Question: can we generate a car from 8 azimuth angles at 3–5 height layers with the Codex CLI, build a Gaussian splat locally, and view it in a local viewer?

## Verdict

**Feasible as a learning prototype, not as a clean capture substitute.** The viewer half is straightforward. The generation half is the risk: the proposed input (about 24–40 separately generated images) is not multi-view consistent, and the Codex CLI does not, per official docs found so far, generate images itself. Build the viewer first, then test the generation path with a 2-day spike and a go/no-go gate.

## 1. The Codex CLI step

- **Not confirmed as an image generator.** The official CLI docs found ([learn.chatgpt.com/docs/codex/cli](https://learn.chatgpt.com/docs/codex/cli)) describe image *input* (`codex --image`), not image output. The sidebar lists an "Image generation" page that was not fetched. Third-party blogs claim native generation with `gpt-image-2`; treat that as unverified.
- **Not installed here.** `codex` is not on PATH. The npm package `@openai/codex` is at 0.164.0.
- **The image API is real.** Official API page lists `gpt-image-2.5-sunburst` and `gpt-image-2.5-flare`. Sizes: 1024×1024, 1536×1024, 1024×1536, or custom sizes (edges multiple of 16, max edge 3840; above 2560×1440 is experimental). Output PNG by default, or JPEG/WebP. The `n` parameter returns several images per request.
- **Simplest route:** call the image API from a script, not through the CLI. The script can enforce the 8×N grid, fixed prompts, a fixed seed where supported, and file naming. Using the CLI adds nothing we can verify.

Cost and rate limits: per-minute limits were not found in the docs. A 40-image run is small, but check the pricing page before scaling.

## 2. Generated views to splat — the core risk

Problems with 24–40 independent generations:

| Problem | Effect on 3DGS |
| --- | --- |
| No consistent identity across calls | Surfaces, badges and wheel shapes change between views; the optimiser averages them into blur or floaters |
| Azimuths and heights are not exact | Camera poses must be estimated, not known |
| Intrinsics unknown | Need a guessed focal length and a lens model |
| Lighting varies | View-dependent colour (paint, glass) conflicts across views |
| Wheels and glass | Glass and chrome are the hardest areas for splats and for generators |

Standard pose recovery (COLMAP/SfM) needs enough matching features between views. Generated views of a car often lack this, so it may fail or register views incorrectly.

Better-suited approaches found in the research:

- **Feed-forward, pose-free reconstruction** — FreeSplatter (ICCV 2025) takes uncalibrated views and outputs Gaussians with poses, and reports processing synthetic multi-view diffusion output ([paper](https://openaccess.thecvf.com/content/ICCV2025/papers/Xu_FreeSplatter_Pose-free_Gaussian_Splatting_for_Sparse-view_3D_Reconstruction_ICCV_2025_paper.pdf)). This is the most promising reconstruction route, but a 2025 model; check for code and licence.
- **Sparse-view, pose-estimated optimisation** — InstantSplat and related work ([arXiv 2403.20309](https://arxiv.org/pdf/2403.20309)). Designed for few real views, not for model-generated views.
- **Multi-view-consistent generation** — a dedicated multi-view diffusion model (for example CAT3D-style) produces views that agree with each other, which the image API does not guarantee. Better input quality than independent calls.
- **Mesh-first alternative** — image-to-3D models (for example TRELLIS-style) produce a textured mesh or Gaussians from one or a few images. Not checked in this research; would need its own feasibility pass.

Recommendation: if we stay with the image API, use the 8 azimuths at a single height first (8 images), with one reference image as input for each call if the API supports it. Add heights only if the 8-view result reconstructs.

## 3. Local viewer

Native Mac options found:

- **MetalSplatter** — Swift/Metal library and sample app, loads PLY on macOS. Intentionally minimal ([GitHub](https://github.com/Spatial-Explorer/MetalSplatter)). Good first viewer for a Mac-only prototype.
- **SuperSplat / PlayCanvas** — browser viewer; the viewer source is already pinned in this project's `third_party/`. Loads SOG and PLY. Runs locally with no install.
- **Brush** — has a web build; Chrome 134+ on macOS per its README ([GitHub](https://github.com/ArthurBrussee/brush)). Also a training path.
- **Spark** — JavaScript renderer, reads SOG ([GitHub](https://github.com/sparkjsdev/spark)).

Not recommended: community WebGPU viewers that the search found were dated or unverified.

For the spike, SuperSplat-style browser viewing over a local file server is the least code. MetalSplatter is the fallback if we want a native app.

## 4. Local constraints (this machine)

- Apple M1 Pro, Metal 4.
- Disk: **4.3 GiB free** on the root volume. Model weights, COLMAP and splat outputs can exceed this. Clear space or use an external disk before any local generation or training.
- RAM: 16 GB unified memory.
- No CUDA. Any training is Metal/CPU-based (Brush, msplat) or remote.

## 5. Spike plan (2 days, gated)

1. **Viewer first (half day).** Take any public-domain or self-made PLY/SOG sample and view it in SuperSplat locally. Pass: renders and orbits on this Mac.
2. **Generation test (day 1).** Script 8 azimuths at one height for one car with a fixed prompt and a reference image. Save images with angle metadata in filenames. Pass: a human can see the same car in all 8 views.
3. **Reconstruction test (day 2).** Run one pose-free reconstruction (FreeSplatter if code is available, otherwise COLMAP with the known azimuths as priors) and view the result. Pass: the car is recognisable from at least 3 unseen angles without large floaters.
4. **Go/no-go.** If step 3 fails, switch to the mesh-first route or a real capture of a physical car.

Before starting, decide the licence status of any car image we use (self-made or cleared), and check the image-API usage terms for generated content.

## Sources

- [Codex CLI docs](https://learn.chatgpt.com/docs/codex/cli) — image input confirmed; generation not found
- [OpenAI image generation guide](https://developers.openai.com/api/docs/guides/image-generation) — models, sizes, formats
- [FreeSplatter, ICCV 2025](https://openaccess.thecvf.com/content/ICCV2025/papers/Xu_FreeSplatter_Pose-free_Gaussian_Splatting_for_Sparse-view_3D_Reconstruction_ICCV_2025_paper.pdf)
- [InstantSplat](https://arxiv.org/pdf/2403.20309)
- [MetalSplatter](https://github.com/Spatial-Explorer/MetalSplatter)
- [Brush](https://github.com/ArthurBrussee/brush)
- [Spark](https://github.com/sparkjsdev/spark)
