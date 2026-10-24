# Splat Racer — initial technical research

Researched **October 24, 2026**. Working name: **Splat Racer**. Goal: a Rust racing game with 3D Gaussian splats for both the environment and car, running natively and in a browser through WASM/WebGPU. “Free physics” is interpreted as an open-source engine with commercial use allowed by its license.

## Recommended starting stack

| Area | Initial choice | Reason / qualification |
| --- | --- | --- |
| Runtime | Rust + `wgpu`, WASM browser target | Keeps the renderer and game in Rust; native and WebGPU paths. |
| Splat renderer | Port the pinned PlayCanvas WGSL pipeline | Good reference for SOG decoding, projection, SH, sorting and rendering; requires adaptation to `wgpu`. |
| Physics | `rapier3d` | Native Rust, WASM support, raycast vehicle controller, CCD and static mesh/voxel shapes. |
| Level collision | SplatTransform voxel generation → `.collision.glb` → Rapier static triangle mesh | Fastest route to a driveable prototype; hand-clean the road where necessary. |
| Additional queries | Port SuperSplat Viewer's sparse voxel octree queries | Useful for camera obstruction, picking and diagnostics; optional alongside Rapier. |
| Training | Brush first; gsplat on an NVIDIA machine as an alternative | Brush fits the Rust/WebGPU direction and works on macOS; gsplat has a mature CUDA/Python training path. |
| Delivery format | Bundled SOG v2 initially; streamed SOG later | Start with one track region and one car before adding spatial streaming. |
| Audio | ElevenLabs for authored voice/SFX; OpenAI for optional interactive speech | Bake core race audio into assets; use local playback during the race. |

These are engineering recommendations, not benchmark results. [`wgpu`](https://github.com/gfx-rs/wgpu) provides the cross-platform Rust graphics foundation. A custom renderer is proposed because porting PlayCanvas source into Rust is an explicit project goal; Bevy remains an option if we want its ECS, asset management and tooling later.

## 1. PlayCanvas: branch, exact pins and source already collected

The official engine is [`playcanvas/engine`](https://github.com/playcanvas/engine). Its **default development branch is `main`**. “Latest branch” here means that development line, not whichever experimental branch happened to receive the newest commit.

| Repository | Development ref inspected | Exact commit | Release observed |
| --- | --- | --- | --- |
| PlayCanvas Engine | `main` | `209ce5752527ab5050ec9632ca6dc1272542b70d` | [`v2.23.3`](https://github.com/playcanvas/engine/releases/tag/v2.23.3), Oct 15, 2026 |
| SplatTransform | `main` | `4f90012be983078010d631fa0572baa99d5c1965` | [`v3.10.3`](https://github.com/playcanvas/splat-transform/releases/tag/v3.10.3), Oct 21, 2026 |
| SuperSplat Viewer | `main` | `06d1c880edc4756a0ccf0a9f03f90fd1182ef178` | Source pinned directly; release not required for this research |

The engine `main` commit is dated Oct 23, 2026, 22:45:10 UTC. The [`release-2.23`](https://github.com/playcanvas/engine/tree/release-2.23) branch also exists, observed at `b7655d8fccc1284678af8f098b26b0725e20dd3a`. Use `main` for current implementation research and the released engine as a comparison baseline. Sources: [engine commit](https://github.com/playcanvas/engine/commit/209ce5752527ab5050ec9632ca6dc1272542b70d), [SplatTransform commit](https://github.com/playcanvas/splat-transform/commit/4f90012be983078010d631fa0572baa99d5c1965), [viewer commit](https://github.com/playcanvas/supersplat-viewer/commit/06d1c880edc4756a0ccf0a9f03f90fd1182ef178).

**Collected locally:** 196 unchanged upstream reference files under `third_party/`, including each repository's MIT license. `upstream-lock.json` records every path, commit and Git blob hash. `scripts/fetch_upstream.py` downloads or verifies this exact set; it refuses to overwrite modified files. These are source subsets for reference, not independently buildable copies of the three projects. No shaders or collision code have been ported to Rust yet.

### Shader and loader map

Paths below are relative to `third_party/playcanvas-engine/`:

| Source path | What we need from it |
| --- | --- |
| `src/scene/shader-lib/wgsl/chunks/gsplat/vert/gsplat.js` | Vertex pipeline assembly and splat projection flow |
| `.../vert/gsplatCenter.js`, `gsplatCorner.js`, `gsplatQuatToMat3.js` | Center transform, projected ellipse and quaternion math |
| `.../vert/formats/sog.js`, `sogSH.js` | Compressed SOG property and spherical-harmonic access |
| `.../vert/gsplatEvalSH.js` | View-dependent SH color evaluation |
| `src/scene/shader-lib/wgsl/chunks/gsplat/frag/gsplat.js` | Gaussian fragment evaluation and alpha handling |
| `src/scene/shader-lib/wgsl/chunks/radix-sort/` | GPU sort building blocks |
| `src/scene/gsplat/`, `src/scene/gsplat-unified/` | Host-side resources, sorting and unified scene management |
| `src/framework/parsers/sog.js`, `sog-bundle.js` | Metadata, textures and bundled SOG loading |
| `src/scene/gsplat/gsplat-sog-data.js`, `gsplat-sog-resource.js` | Decode behavior, codebooks and GPU resource layout |

Both GLSL and WGSL splat chunks were collected. Use WGSL first. The source [WGSL entry point](https://github.com/playcanvas/engine/blob/209ce5752527ab5050ec9632ca6dc1272542b70d/src/scene/shader-lib/wgsl/chunks/gsplat/vert/gsplat.js) contains PlayCanvas-specific `#include`, `#ifdef`, `uniform`, `varying` and half-type conveniences inside JavaScript strings. It **cannot be pasted directly into a `wgpu` shader module**. We must compose chunks, replace engine declarations with explicit WGSL structs/bindings, choose shader variants and match texture/buffer layouts.

The host code is as important as the shader math. The port must implement sorting, camera/object transforms, culling, blend/depth conventions, SH lookup, SOG textures and resource lifetime. Begin with the simplest sorted raster path before considering the unified GPU pipeline.

### Reuse licenses

The [engine](https://github.com/playcanvas/engine/blob/209ce5752527ab5050ec9632ca6dc1272542b70d/LICENSE), [SplatTransform](https://github.com/playcanvas/splat-transform/blob/4f90012be983078010d631fa0572baa99d5c1965/LICENSE) and [viewer](https://github.com/playcanvas/supersplat-viewer/blob/06d1c880edc4756a0ccf0a9f03f90fd1182ef178/LICENSE) are MIT licensed. Keep their copyright/license notices with copied or adapted substantial code. Track the origin of each Rust port. Community splat assets have separate licenses; the engine license grants no rights to them.

## 2. Their voxel collider: two distinct parts

**Offline generation** is in SplatTransform. Collected source includes `src/lib/voxel/`, `src/lib/gpu/gpu-voxelization.ts`, `src/lib/writers/write-voxel.ts`, `collision-glb.ts`, and `src/lib/mesh/`. It produces a sparse voxel octree (`.voxel.json` + `.voxel.bin`) and, optionally, a `.collision.glb` triangle mesh. See the [generation guide](https://developer.playcanvas.com/user-manual/splat-transform/collision/) and [pinned source](https://github.com/playcanvas/splat-transform/tree/4f90012be983078010d631fa0572baa99d5c1965/src/lib/voxel).

**Runtime queries** are in `third_party/supersplat-viewer/src/collision/`, especially [`voxel-collision.ts`](https://github.com/playcanvas/supersplat-viewer/blob/06d1c880edc4756a0ccf0a9f03f90fd1182ef178/src/collision/voxel-collision.ts) and `collision.ts`. They expose ray, sphere and capsule queries plus surface-normal and free-space helpers. The octree uses packed 32-bit nodes and mixed-leaf occupancy masks. Preserve unsigned shifts, axis conventions, grid origin, bounds and voxel resolution when translating to Rust.

**Racing recommendation:** import the generated GLB's transformed triangle geometry into a Rapier static collider. Use a dynamic convex/compound chassis collider and suspension wheel raycasts. The viewer collider is useful query code, but does not provide rigid-body dynamics, tire forces, torque or suspension.

Suggested outdoor conversion, run from this project folder with Node 22+:

```sh
npx --yes --package @playcanvas/splat-transform@3.10.3 splat-transform \
  assets/track.ply --seed-pos 0,0,0 \
  --voxel-size 0.05 --voxel-opacity 0.1 \
  --voxel-floor-fill --collision-mesh smooth assets/track.voxel.json
```

Replace the seed and units after aligning the actual track. Optionally add `--filter-cluster` for disconnected floaters; it requires the GPU path. Interior scans have different fill/carve settings. The default carve capsule is for a walking person, so do not assume its settings suit a car. A 5 cm voxel size is a starting experiment, not a requirement. Large maps should be partitioned before collider generation.

**Expected risk, to measure:** raw voxel faces can create stepped tire contacts. Start with smooth mesh output, then author a clean invisible road surface if driving feels rough. Splat opacity does not reliably define a physical surface, and scans can contain holes, vegetation, glass and missing road edges.

## 3. Free Rust/WASM physics

**Choose [`rapier3d`](https://github.com/dimforge/rapier)**, Apache-2.0. The [crate documentation](https://docs.rs/rapier3d/0.36.1/rapier3d/) currently exposes version **0.36.1**. Pin a release and commit `Cargo.lock` when implementation starts; avoid copying examples from moving `master` into an older release without checking API differences.

The [Rust vehicle guide](https://rapier.rs/docs/user_guides/rust/vehicle_controller/) documents `DynamicRayCastVehicleController`: one chassis rigid body, wheel raycasts, suspension tuning, engine force, braking and steering. This is an appropriate arcade-racing baseline. Handling quality still requires tuning grip, damping, mass, center of mass and force curves. [Upstream source](https://github.com/dimforge/rapier/blob/master/src/control/ray_cast_vehicle_controller.rs) and [example](https://github.com/dimforge/rapier/blob/master/examples3d/vehicle_controller3.rs) are useful implementation references.

Rapier also exposes [voxel geometry](https://docs.rs/rapier3d/latest/rapier3d/geometry/struct.Voxels.html). That API does not directly consume PlayCanvas's packed SVO files: either adapt occupied cells into Rapier geometry or use the exported collision GLB. Prefer the GLB route for the first driving test.

For WASM, begin with a single-threaded `wasm32-unknown-unknown` build. Leave optional Rayon parallelism off until worker/shared-memory deployment is deliberately configured. Validate the exact crate feature set and browser build early. Start at a fixed 60 Hz simulation step with interpolated rendering; test faster stepping/CCD for high-speed impacts. These are proposed settings, not proven performance claims.

Alternative: [Avian](https://github.com/avianphysics/avian), MIT/Apache-2.0, is designed around Bevy ECS and is attractive if we choose Bevy for the whole game. Rapier currently fits this custom-renderer plan better and gives us an existing vehicle controller to evaluate immediately.

## 4. Gaussian training code

| Candidate | License and platform | Proposed role |
| --- | --- | --- |
| [Brush](https://github.com/ArthurBrussee/brush) | Apache-2.0; Rust, Burn and WebGPU; native macOS/Windows/Linux and browser support described upstream | First local training experiment; takes COLMAP or Nerfstudio datasets and exports splats. |
| [gsplat](https://github.com/nerfstudio-project/gsplat) | Apache-2.0; CUDA rasterization with Python bindings | NVIDIA training workstation/cloud alternative; configurable trainer and explicit PLY export. |
| [Original Graphdeco implementation](https://github.com/graphdeco-inria/gaussian-splatting) | Custom research/noncommercial license | Reference paper implementation; do not make it the commercial production dependency without separate permission. |

Brush `main` inspected at `1388f74c6fe0236f68ee4915564bf00e9d2e3747`; gsplat at `512d366b67073d77ca099ede742683c165dfc23b`. Check each project's dependencies and selected datasets separately. The original implementation's restrictions are explicit in its [license](https://github.com/graphdeco-inria/gaussian-splatting/blob/main/LICENSE.md).

Proposed capture pipeline: overlapping photos/video frames → camera reconstruction/calibration → Brush or gsplat training → Gaussian PLY → cleanup/alignment → SOG + independent collision assets. Train offline; training code does not need to ship in the game.

The inspected gsplat [`simple_trainer.py`](https://github.com/nerfstudio-project/gsplat/blob/512d366b67073d77ca099ede742683c165dfc23b/examples/simple_trainer.py) has a `default` configuration and `save_ply` flag. From a correctly installed gsplat checkout, the command shape is:

```sh
python examples/simple_trainer.py default \
  --data-dir /path/to/colmap-dataset \
  --result-dir /path/to/training-results --save-ply
```

Verify that pinned checkout's `--help` and CUDA/PyTorch requirements before running. No training run has been performed here.

For the car: capture it stationary with a consistent background/masks; split body and wheels into separate rigid splat objects so wheels can rotate and steer. Keep a separate simple physical chassis shape. Baked reflections and view-dependent SH may look wrong under new lighting; evaluate them during the first car render. A whole-car scan alone does not supply wheel articulation or collision geometry.

## 5. PLY → SOG compression

Use the official [`@playcanvas/splat-transform`](https://github.com/playcanvas/splat-transform/tree/4f90012be983078010d631fa0572baa99d5c1965) CLI, pinned to **3.10.3**. Its package requires **Node >=22**. Format conversion is selected by the output extension:

```sh
# Bundled runtime asset; CPU mode avoids requiring a local GPU for compression.
npx --yes --package @playcanvas/splat-transform@3.10.3 splat-transform \
  --gpu cpu assets/track.ply assets/track.sog

# Separate metadata + textures, useful while implementing the loader.
npx --yes --package @playcanvas/splat-transform@3.10.3 splat-transform \
  --gpu cpu assets/track.ply assets/track-sog/meta.json

# Decode back to a standard Gaussian PLY for inspection.
npx --yes --package @playcanvas/splat-transform@3.10.3 splat-transform \
  assets/track.sog assets/track-roundtrip.ply
```

The [SOG v2 specification](https://developer.playcanvas.com/user-manual/gaussian-splatting/formats/sog/) defines metadata plus WebP textures, optionally bundled as a ZIP with `.sog` extension. Properties use position encoding, quantized orientation, scale/color codebooks and optional higher-order SH data. Preserve texture bytes as data; avoid accidental sRGB conversion. Check metadata versions. Compression is lossy; round-tripping does not restore the original PLY exactly.

Keep source PLYs as masters. Measure encoded bytes, decoded memory, download/decode latency, visible artifacts, SH quality and frame time. Disk size alone does not establish runtime speed or VRAM use. A generic point-cloud PLY is not necessarily a Gaussian PLY: training outputs must include the necessary opacity, orientation, scale and color/SH properties.

For larger tracks, evaluate [streamed SOG](https://developer.playcanvas.com/user-manual/splat-transform/streamed-sog/) with `lod-meta.json` and spatial chunks. It requires runtime LOD selection, loading and eviction; supporting single-file SOG does not automatically support streaming.

## 6. OpenAI API and ElevenLabs audio

### OpenAI

Use official [Realtime documentation](https://developers.openai.com/api/docs/guides/realtime) for optional responsive race-engineer conversations. Current [GPT-Realtime-2.1 Mini documentation](https://developers.openai.com/api/docs/models/gpt-realtime-2.1-mini) describes text/audio input and output over WebRTC/WebSocket/SIP. Keep the provider and model configurable.

**Current migration detail:** the [OpenAI deprecation notice](https://developers.openai.com/api/docs/deprecations) lists `tts-1`, `tts-1-hd` and `gpt-4o-mini-tts` for shutdown on **January 6, 2027**, with `gpt-realtime-2.1-mini` recommended as replacement. The [speech guide](https://developers.openai.com/api/docs/guides/text-to-speech) explicitly says this is a workflow change: that Realtime model is not accepted by `/v1/audio/speech`. Avoid basing a new long-lived integration on that legacy endpoint. The guide also requires disclosure that generated voices are AI voices.

For predictable race callouts, pre-generate and cache clips; reserve live model calls for optional dialogue. OpenAI speech is not the source of responsive engine/tire synthesis.

### ElevenLabs

Use [`POST /v1/sound-generation`](https://elevenlabs.io/docs/api-reference/text-to-sound-effects/convert) for authored effects and ambience. The inspected reference supports text, `loop`, `duration_seconds` (0.5–30), `prompt_influence`, and models `eleven_text_to_sound_v2` / `eleven_text_to_sound_v3`; the documented default is v2. Specify the chosen model rather than relying on its default.

Use [`POST /v1/text-to-speech/{voice_id}`](https://elevenlabs.io/docs/api-reference/text-to-speech/convert) for announcer/race-engineer lines, with `xi-api-key` authentication and an explicitly selected voice/model. Its [speech overview](https://elevenlabs.io/docs/overview/capabilities/text-to-speech) states commercial use requires a paid plan. Confirm the current plan and voice rights before shipping; a free experimentation tier is not equivalent to a commercial asset license.

Proposed asset list: engine idle plus several RPM/load bands, tire skid, gravel, crash impacts, gear changes, wind, UI cues and race announcements. Crossfade/pitch engine loops locally from simulated RPM/load; blend skid/wind from vehicle state. Listen for loop seams and unwanted music/voices in generated SFX. Store prompts, model, voice ID, generation date and usage rights with generated assets.

Use a backend or offline asset tool for full API keys. A browser build can use appropriately scoped short-lived credentials for supported live speech sessions. No paid generation or authenticated API call was made during this research.

## 7. Downloading splats from SuperSplat

SuperSplat's supported asset-download workflow is the scene page's **Download** control on scenes whose owners enable downloads. The [official announcement](https://blog.playcanvas.com/new-in-supersplat-downloadable-splats-licenses-and-social-links/) describes original/source and PLY download options, Creative Commons licenses and credit lines. The [getting-started guide](https://developer.playcanvas.com/user-manual/supersplat/getting-started/) describes the owner's Downloadable setting.

**Script found:** [`renbkna/tortuise/scripts/supersplat-dl.sh`](https://github.com/renbkna/tortuise/blob/main/scripts/supersplat-dl.sh). Its inspected implementation constructs a legacy CDN path of the form `/<scene-id>/v1/scene.compressed.ply`, handles gzip and invokes SplatTransform. It is a useful historical reference, but assumes one storage layout and uses an unpinned converter. It is not evidence that every current scene has a downloadable PLY at that path.

**Script supplied here:** `scripts/download_supersplat.py`. Obtain the explicit asset URL from a downloadable scene's Download dialog (or its actual download request), then use:

```sh
python3 scripts/download_supersplat.py \
  --scene-url 'https://superspl.at/scene/SCENE_ID' \
  --asset-url 'https://ACTUAL_HOST/ACTUAL_ASSET.sog' \
  --creator 'Creator name' \
  --license-url 'https://creativecommons.org/licenses/by/4.0/' \
  --credit 'Paste the provided attribution line here' \
  --output assets/test-track.sog
```

The placeholders must be replaced with the selected scene's actual values. The script downloads single-file `.ply` or `.sog`, decodes a gzip wrapper when present, checks the file container, applies a size limit, refuses overwrite and saves SHA-256 plus provenance in `.source.json`. Signed query strings are excluded from saved URLs. It does not scrape the gallery, infer undocumented endpoints, resolve share pages, authenticate, or download streamed SOG trees. Existing downloaded files can also be copied directly into `assets/`.

Prefer original/self-captured assets or scenes explicitly licensed for the intended commercial use and modifications. Record credit text and the license version per asset; public visibility alone does not supply reuse permission. No community scene is bundled with this project yet.

## 8. First implementation milestones

1. **Renderer proof:** render a known Gaussian PLY, then SOG v2, in native Rust and browser WASM. Compare the same camera against PlayCanvas. Verify projection, alpha, SH, sorting and axis conventions.
2. **Moving car proof:** render separate car-body/wheel splats over a static level. Update world transforms and depth ordering together; test close intersections between car and road splats. Rotate SH/view directions in the correct object space.
3. **Driving proof:** import one track collider into Rapier, add the raycast vehicle controller, and tune suspension/grip. Test ramps, curbs, high-speed wall impacts and resets with debug collider overlays.
4. **Asset proof:** train a short track section and one car using Brush; export PLY, compress to SOG and compare visuals. Lock meters, origin and orientation across rendering and physics.
5. **Audio proof:** generate a small reusable asset set; blend engine/skid audio locally. Add optional live race-engineer dialogue only after the offline race is functional.
6. **Scale proof:** benchmark target browsers/hardware; add spatial LOD/streaming only after memory and frame-time measurements justify it.

Provisional evaluation targets: 60 Hz physics, a 60 FPS render goal on the selected desktop/browser baseline, and documented budgets for visible splats, sorting time, overdraw, GPU memory and initial payload. No hardware baseline or performance result has been established yet. Browser WebGPU support must be tested on the actual target browser/device combinations.

## 9. Research status and verification

- Created this project folder, research document, exact upstream file manifest and source-fetch/download scripts.
- Downloaded and Git-blob-verified all 196 pinned reference files; retained all three MIT licenses.
- Verified Python 3.9.6 and Node 22.22.1 are available in this environment. `cargo` is not currently on PATH, so no Rust build or WASM compilation is claimed.
- Shader translation, vehicle integration, actual capture/training, streamed asset downloading and authenticated audio generation remain implementation work.
- Script checks and the compression smoke test are recorded in `verification.md`.

Open decisions for the next phase: browser/native priority; arcade versus simulation handling; first track/car capture; hardware baseline; and custom `wgpu` runtime versus adopting Bevy. The proposed stack above lets the first renderer and driving experiments proceed before those choices are final.
