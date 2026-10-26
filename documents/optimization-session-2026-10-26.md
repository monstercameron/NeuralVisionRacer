# Four-hour optimization session — 26 October 2026

Window: **16:29:58–20:29:58 UTC**, or **12:29:58–4:29:58 PM EDT**. Hard deadline; reserve 20:09:58 onward for final integration. Heartbeat: `neuralvisionracer-four-hour-optimization`, every 15 minutes in this chat, ending within the original window. Respect later human steering; never extend the window.

Repository baseline: `6346b7bc7c39e773543e28ee22392e21d384dcb5`; initial working tree clean. User requests optimization in every dimension, excluding Metal. Work on Rust/WASM, WebGPU/WGSL, physics, streaming, memory, loading, scheduling and developer diagnostics. Preserve visuals, resolution/detail defaults, mechanics and controls. Do not claim disabled effects as optimization. Do not spawn agents. Existing authorization covers coherent tested commits/pushes and rebuilding Pages for publication.

Evidence: `/Users/earlcameron/Documents/Codex/2026-10-24/let/outputs/optimization-2026-10-26`. Browser-only GPU measurements; native backend timings are not acceptable browser evidence. Leave the user's active game tab/state intact. Run paired samples without concurrent builds/GPU tests. Keep CPU and GPU work distinct. Record noisy/unsupported timings honestly.

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

## Continuation checkpoint

First pass is ready to commit: loader, benchmark/validation tools, build-script module inclusion and documentation. Inspect Git status for completion. The current production Rust/WGSL remains baseline `6346b7b`; web/pkg is restored to its retained baseline snapshot. No final Pages rebuild/push yet; reserve that for accepted integrated work.

Next priorities:
1. Consider rendering post/debug/HUD directly to the acquired sRGB surface, keeping the offscreen path for screenshots. Current `Frame::render` always draws post/overlays to an intermediate RGBA8 target, then blits it to the surface; eliminating this last full-screen write/read/copy may reduce bandwidth. Needs format-specific post/line/overlay pipeline variants (browser surface BGRA8 sRGB versus offscreen RGBA8 sRGB), paired browser measurement, pixel/overlay validation and resize handling. This is a candidate, NOT implemented.
2. Audit shadow/SH vertex repetition and portal clipping/overdraw, preserving shape/lighting/detail. Existing static benchmark has ~1.5M level slots and 648,207 car splats.
3. CPU/physics/streaming/loading allocations remain to audit; see work queue.

Keep baseline snapshots and rejected experiment evidence. Don't run native GPU probes or count Metal backend timing as browser evidence. Check UTC first. Hard deadline **20:29:58 UTC**, integration reserve **20:09:58 UTC**. Automation ID remains `neuralvisionracer-four-hour-optimization`.
