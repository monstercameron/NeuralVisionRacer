# Four-hour optimization session — 26 October 2026

Window: **16:29:58–20:29:58 UTC**, or **12:29:58–4:29:58 PM EDT**. Hard deadline; reserve 20:09:58 onward for final integration. Heartbeat: `neuralvisionracer-four-hour-optimization`, every 15 minutes in this chat, ending within the original window. Respect later human steering; never extend the window.

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

Keep baseline snapshots and rejected experiment evidence. Don't run native GPU probes or count Metal backend timing as browser evidence. Check UTC first. Hard deadline **20:29:58 UTC**, integration reserve **20:09:58 UTC**. Automation ID remains `neuralvisionracer-four-hour-optimization`.
