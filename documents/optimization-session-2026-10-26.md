# Optimization session — 26–27 October 2026

Current authorized continuation: **2026-10-27 00:20:31–08:20:31 UTC**, or **8:20:31 PM–4:20:31 AM EDT**. The human explicitly requested eight more hours. Reserve 08:00:31 UTC onward for final integration. The existing heartbeat `neuralvisionracer-four-hour-optimization` now follows this window. Commit checkpoints are approximately each half hour (next 00:30 UTC), always with a fresh Pages rebuild before committing. Respect newer human steering.

Original window was 16:29:58–20:29:58 UTC on October 26. Clock readings jumped from around 17:13 to 00:18 UTC during the CPU pass; host time confirmed the later time. The promised historical half-hour commit cadence was not met after the 17:02 UTC checkpoint. This continuation does not retroactively claim those missed checkpoints. Latest published checkpoint is `612e5db` (17:02 UTC), direct surface presentation and rebuilt Pages.

Repository baseline: `6346b7bc7c39e773543e28ee22392e21d384dcb5`; initial working tree clean. User requests optimization in every dimension, excluding Metal. Work on Rust/WASM, WebGPU/WGSL, physics, streaming, memory, loading, scheduling and developer diagnostics. Preserve visuals, resolution/detail defaults, mechanics and controls. Do not claim disabled effects as optimization. Do not spawn agents. Existing authorization covers coherent tested commits/pushes and rebuilding Pages for publication.

Evidence: `/Users/earlcameron/Documents/Codex/2026-10-24/let/outputs/optimization-2026-10-26`. Browser-only GPU measurements; native backend timings are not acceptable browser evidence. Leave the user's active game tab/state intact. Run paired samples without concurrent builds/GPU tests. Keep CPU and GPU work distinct. Record noisy/unsupported timings honestly.

The current goal also requires a commit about every 30 minutes and a **fresh Pages rebuild before every commit**. Use approximately 17:00, 17:30, 18:00, 18:30, 19:00, 19:30, 20:00 and final 20:29 UTC checkpoints. Verify packaged sources/config/WASM after each rebuild; don't commit a broken experiment to satisfy cadence. This requirement was added after the first loader commit.

## Work queue

1. Establish repeatable browser WebGPU fixed-scene baseline; preserve raw samples, scene occupancy, dimensions and feature states.
2. Post-processing: redundant colour conversions, repeated depth reads, DOF spiral evaluation and depth-mask work.
3. Splat draw/culling/sorting: overdraw, portal clip cost, shadow/SH vertex work, unchanged-frame skipping, portable binding limits.
4. WASM/CPU allocations: rendering preparation, uniform uploads, HUD/metrics serialization, scene transforms, streaming selection/decode.
5. Physics/collision: tile rebuilds, queries and contacts under actual driving; preserve determinism and stepping.
6. Asset loading/cache/startup and memory lifecycle; avoid changing source assets/licences.
7. Integration, visual regression, release WASM/Pages package, coherent final commit/push and measured results.

## Pass 1 — browser baseline and post grade

Completed baseline tooling and accepted an asset-loading optimization. New local diagnostic pages:
- `web/benchmark.html` / `.js`: production browser viewer, fixed 1280×720/LOD 1, 40 warm-up frames and 120 submitted measured frames with backpressure, paused settled physics, grain/debug off; shader effects intact. UI prints JSON and becomes idle.
- `scripts/save_browser_baseline.py`: snapshots compiled browser package/post shader with source status and hashes to ignored `logs/browser-benchmark`. Baseline is already saved. Use `build=baseline` versus `build=current` on the benchmark URL.
- `web/post-validation.html` / `.js`: browser WebGPU pixel readback against the retained production shader preimage; five grade/clarity/blur cases.

**Rejected experiment:** reuse linear scene colour to avoid the grade's sRGB round trip on unchanged background pixels. Browser pixel fixtures passed (maximum delta 1/255; car clarity and grade-off byte-identical), but frame median stayed **21.233664 ms** for both baseline and candidate in the 120-submitted-frame Saburi case. Baseline repeat also 21.233664 ms. No demonstrated gain; shader source and local web/pkg were restored. Patch retained in `grade-experiment.patch` under evidence. No shader change is currently being shipped.

**Accepted loader change:** `web/asset-cache.js` now checks whether a cached entry exists before validating it. Cold misses go straight to GET and cache its actual response validator; existing entries still HEAD-validate before use. One cache handle per page replaces repeated opens. Unavailable/open/read/write cache failures fall back to network. Explicit cache clearing resets the handle. Metrics distinguish validator requests, payload requests and cache opens. Pages packaging includes the new module.

Four alternating real-local-HTTP pairs, eight Saburi chunk files, **10,217,317 bytes** per run: old loader **16** cold requests (8 HEAD + 8 GET), new loader **8** (8 GET), identical SHA-256 for every payload. Warm runs remain 8 HEAD requests, with all bytes served from cache. Median cold fixture times 41.76 ms baseline and 28.46 ms candidate, with high variance and warm filesystem; don't generalize this to full startup or FPS. Acceptance is the deterministic 50% cold-request reduction and byte identity.

Actual browser game startup succeeded with **104 cache hits, 122,005,402 cached bytes, 1 cache open, zero failed loads and zero payload network requests**, followed by normal rendering/streaming. Temporary review tabs were closed; user's active tab untouched. Fourteen Node loader regressions and JS/Python syntax checks pass. Screenshot and raw JSON retained under evidence.

GPU timestamp pass spans overlap/coalesce on this browser (post and blit boundaries can coincide); use total GPU frame time, paired runs and operation counts, not a sum of exclusive pass costs. All measurements so far use browser WebGPU; no Metal optimization/native GPU benchmark was run.

## Pass 2 — direct surface presentation

Implemented a surface-format variant of the existing post/line/HUD pipelines, with a shared final draw path and the original offscreen/copy path retained for readback and comparison. `perf present direct|copy` switches the path live; profile metadata records the path/sRGB surface format. The direct path skips `present_blit` (8 final sampled passes versus 9 in the reference scene) and avoids about 7.37 MB of intermediate full-screen traffic at 1280×720; it does not reduce texture allocations or scene quality.

Browser measurement uses the SAME compiled WASM binary for the two paths, fixed scene/camera/detail/dimensions, actual configured external car spec and dashboard. The harness was corrected to load those external files; earlier baseline measurements did not include the external dashboard, so do not compare their frame numbers with these HUD-enabled cases. The production game already loaded them correctly.

First HUD-enabled pair: copy median **21.168128 ms**, direct **21.102592 ms**. Small, noise-scale timing difference; no FPS claim. Repeated runs are retained in `presentation-summary.json`. Full-resolution framebuffer PNGs were read through the diagnostic page: **zero changed channels** across all 1280×720 RGBA pixels, including the HUD. Surface format is BGRA8 sRGB in both cases; original copy and direct paths remain available. Browser resize/debug verification completed at 960×540 and 1280×720, with collision/LOD overlays both on and off; the rendered scene and dashboard remained intact. All work is WebGPU/wgpu, no Metal-specific code or native GPU benchmark.

## Continuation checkpoint

First pass was committed as `5dcca2f`: loader, benchmark/validation tools, build-script module inclusion and documentation. The current production Rust/WGSL remains baseline `6346b7b`; web/pkg is restored to its retained baseline snapshot. The 17:00 UTC checkpoint rebuilt Pages (129 MB, 111 files), verified matching packaged JS/config/WASM and actual browser startup. The packaged game reported direct_surface/BGRA8 sRGB, 104 successful cache hits through one handle and zero failed loads. Rust renderer tests compile; 14 asset-loader tests pass. Copy/direct reference images remain byte-identical. Next scheduled checkpoint is about 17:30 UTC.

Next priorities:
1. Direct surface presentation is implemented and browser-verified; retain the copy/readback fallback. Baseline `Frame::render` drew post/overlays to an intermediate RGBA8 target, then blitted it to the surface; eliminating this last full-screen write/read/copy may reduce bandwidth. Needs format-specific post/line/overlay pipeline variants (browser surface BGRA8 sRGB versus offscreen RGBA8 sRGB), paired browser measurement, pixel/overlay validation and resize handling. Pass 2 verification and Pages rebuild completed for this checkpoint.
2. Audit shadow/SH vertex repetition and portal clipping/overdraw, preserving shape/lighting/detail. Existing static benchmark has ~1.5M level slots and 648,207 car splats.
3. CPU/physics/streaming/loading allocations remain to audit; see work queue.

Keep baseline snapshots and rejected experiment evidence. No native GPU probes or Metal-specific work. Check UTC first and follow the current continuation window at the top of this document. Automation ID remains `neuralvisionracer-four-hour-optimization`.


## Pass 3 — collision search reuse and driveline allocations

The mechanics step previously cloned the entire public car spec (seven allocations per tick) and sampled the full peak-power curve every tick for thermal load. It now borrows the spec and caches peak power while checking the exact bits of every dependent input (idle/limiter RPM and torque curve), preserving public live edits and reset behavior. CPU regression tests cover changes to curve torque/RPM/length and idle/limiter settings. All 13 car tests passed.

Two reversed-order pairs of CPU-only component probes, 100,000 steps per case × four rounds per run, produced identical fingerprints over every output field for cruising, mixed controls and live spec edits. Median instrumented time across the eight rounds per case: cruise **890.02 → 61.07 ns/step**, mixed **885.94 → 63.34 ns**, spec edits **889.31 → 64.66 ns**. Allocations **7 → 0 per step** in these warmed fixtures. These include hashing/allocation-counter overhead and are native CPU component results, not whole-game FPS or WebGPU timings.

Collision streaming now retains a conservative distance guard: within it no absent tile can enter the build radius and no loaded tile can leave the retention radius. Crossing the guard performs the original exact search. Tile sizes, thresholds, geometry and physics stepping are unchanged; telemetry reports full versus reused searches. Dropping far tiles uses retain instead of allocating a temporary vector. Differential tests compare every build/drop and tile set against forced full search across shifted origins, three tile sizes, boundary crossings, empty regions and teleports. All 73 engine tests passed.

Two reversed-order actual Delft floor probe pairs (6,000 updates/case) preserve all build/drop traces and final collider geometry fingerprints. The road trace reuses **5,884 of 6,001** searches (98.1%); stationary/jitter reuse every post-initialization search. Timer resolution quantizes these tiny single-call measurements (some report zero), so accept deterministic skipped work rather than claiming zero cost. Collider construction still dominates aggregate teleport/road workloads; no claim that those builds became faster. Raw records and `cpu-summary.json` are in the evidence directory. Fresh Pages rebuild completed (129 MB, 111 files); packaged JS matches source, correct Saburi config and valid WASM verified. Actual packaged browser simulation advanced through 1,746 ticks, loaded/retained colliders, accelerated to 44.6 km/h with rotating wheel telemetry, and generated tread/dust output without renderer failure. Collision telemetry: 15 full searches, 1,732 reused of 1,747 updates. A later console brake request did not remain held through browser input updates, so this browser run is not evidence of sustained braking/reverse; the existing targeted mechanics/physics tests cover those semantics. Temporary review tab closed. Package hash and browser status/profile/screenshot retained under evidence.


## Pass 4 — depth prepass, DOF tap constants and profiler bookkeeping

Depth shaders now test unchanged SOG opacity × LOD fade against the existing depth cutoff before calculating projected Gaussian covariance/corners. The later original rejection remains. The early path applies only with AA disabled; AA-enabled shaders retain the original order, and both variants pass Naga validation. Colour shader behavior and thresholds are unchanged.

Same browser, retained `eee0bc0` package, fixed Saburi LOD1 / 1280×720 / actual HUD, 180 submitted frames per run in reference/candidate/candidate/reference order: reference frame medians **20.709376 / 21.364736 ms**, candidate **20.381696 / 21.168128 ms**. Car-depth median **1.310720 → 1.048576 ms** in both pairs; level-depth spans about 2.03 → 1.97 ms. A second camera gave 19.267584 → 18.546688 ms, but total-frame variability remains material. Do not generalize these static-scene numbers to driving FPS or add overlapping pass spans. Default and second-view frames are byte-identical over every 1280×720 RGBA pixel. Resize/debug checks also passed.

The DOF spiral retains all 16 samples and its exact filter/foreground rejection, but fixed unit-disc offsets are evaluated at shader creation. Browser-only 1280×720 pixel/timestamp fixtures cover disabled, sharp, mixed depth with thin foreground/car edges, and wide blur. Two runs × 30 alternating-order samples/case are byte-identical. Mixed-depth GPU median **0.458752 → 0.393216 ms**, wide blur **0.589824 → 0.458752 ms** in both runs; sharp/disabled unchanged at timestamp granularity (0.065536 ms). These are isolated shader fixtures, not full-game timings. Combined shader build also produced a byte-identical game frame, with resize/debug checks and a 20.971520 ms frame median inside the observed noise range.

Metrics now borrows existing BTreeMap keys instead of constructing/discarding String keys on every record/count. Instrumented CPU-only 100,000-iteration fixtures × four rounds in each of two reversed-order pairs retain identical serialized summaries. Four-record update median **156.35 → 49.69 ns**; four-counter update **147.51 → 43.77 ns**; mixed eight-call update **295.36 → 91.36 ns**. Warm allocations drop **4/4/8 → 0/0/0** per fixture update. Window capacity, validation, counter values and summary precision are unchanged; existing metric regression passes. No GPU adapter is created by this probe.

Evidence: `prepass-summary.json`, paired PNG comparisons, `dof-constant-taps-{1,2}.json`, `shader-combined-1.json`, `metrics-summary.json`. Diagnostic `web/dof-validation.html` uses the retained preimage; named baselines can now be saved without overwriting earlier experiments using `scripts/save_browser_baseline.py --name depth-prepass-reference`. Developer diagnostics are not packaged into Pages.

Next measured opportunities: GPU memory and streaming uploads retain means_l/means_u textures despite the renderer already reading shared decoded centres; audit removing those unused streams while preserving the existing shared-centre quantization exactly. The SH palette texture also reserves unused rows; size it to reachable chunk palettes/device limits without reducing any actually usable palette slot.

Pass 4 packaging: fresh Pages build completed (129 MB, 111 files). Packaged JS/config/.nojekyll and WASM equality to the browser-verified local build passed; SHA-256 `b590dfe904007be3d6b18a445a333e10aff5ff7d1dd560d4632885dd0d2be029`. All 15 WGSL tests pass; metrics window regression and JS/Python syntax checks pass.
