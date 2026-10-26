# Engine performance

The renderer uses four indexed vertices per Gaussian, with the existing two-triangle coverage. Decoded centre
positions are shared between sorting, wheel deformation and vertex shading instead of being decoded again from
SOG textures in every vertex invocation. This reuses an existing buffer; it does not duplicate centre storage.
Wheel centres are deformed once by compute, while Gaussian orientation and spherical harmonics retain their wheel
transforms. Car sharpening, sunlight, opacity, wheel animation and scene detail remain enabled.

## Changes by cost

| Area | Change |
| --- | --- |
| Vertex work | Indexed quads reduce vertex invocations from six to four per Gaussian. Reuse decoded centres; avoid RGB/SH lighting work in the depth-only variant. |
| Sorting | Encode depth keys, draw arguments and radix dispatches in one compute pass. Aggregate visible counts per workgroup. Skip repeated order copies when the destination is unchanged. Streaming writes/releases invalidate the cached order. |
| Lighting | Add specular radiance in linear space at the existing output stage, avoiding an extra gamma encode/decode round trip. |
| CPU submission | Cache identical scene uniforms and car-light data. Deform wheel centres only after wheel pose changes. |
| HUD | Reuse the exact overlay when telemetry, viewport and visible blink phases match. Moving gauges and blinking shift lights still update. |
| Post effects | Bypass disabled depth of field; return early for already in-focus pixels; sample unfringed colour once when chromatic fringing is zero. |
| Collision | Remember empty immutable collision tiles instead of rebuilding and smoothing them every tick. Collision geometry, physics substeps and tick frequency are unchanged. |
| Frame loop | Permit at most two pending rendering submissions. Continue measuring elapsed simulation time independently of presented frames. |
| Instrumentation | Optional pass-boundary GPU timestamps, nonblocking readback once every eight frames, and separate queue-completion latency. |

The shared centres can differ very slightly from the old shader's second SOG quantization. Reference images should
therefore be compared numerically and visually rather than expecting byte equality. Indexed quads alone produced
an identical reference image during development.

## Reproduce a benchmark

Run from the repository root, with other GPU workloads stopped:

```sh
cargo build --release --manifest-path crates/splat-app/Cargo.toml --example perf_probe
VARIANTS=base target/release/examples/perf_probe saburi-alley-yazd /tmp/saburi-perf 90 1 1280 720
```

Arguments are level folder name, output directory, frames per case, streaming (`0` or `1`), width and height.
The probe uses the production native renderer and current `engine.conf`, settles the car for 180 fixed ticks,
pauses physics, and measures a still camera followed by a slow orbit. It excludes loading, compilation, warm-up
and screenshots from frame timings. Each timed frame waits for GPU completion. The resulting FPS is GPU-completed
throughput, not requestAnimationFrame frequency or a driving/race benchmark. Raw samples, mean/median/p95 frame
times, median CPU submission cost and reference images are saved beside `results.json`. Physics is measured
separately over 600 unpaused ticks. Debug overlays and random grain are off; HUD and the configured car effects
remain on. Use a release build on the same hardware and resolution for comparisons.

`VARIANTS=base,no_hud,no_dof,no_specular,no_clarity,no_shadow` isolates feature costs without changing defaults.
Streaming uses real-time LOD hysteresis and fades, so faster runs can have slightly different transient occupancy.
Use streaming `0` for a fixed LOD-1 scene when strict identical geometry is required; record `stream_stats` for
streaming comparisons. Do not compare these results to a browser canvas of a different size.

## Browser diagnostics

The local browser console accepts `perf`. Its JSON reports six CPU phase timings (tick, decode, stream,
preparation, encoding, total), GPU milliseconds, timing source, queue latency, pending frames and canvas size.
`set render.frame_pacing off` disables refresh-divisor pacing for a throughput check; `auto` restores even pacing.
The setting applies live. Leave resolution and LOD settings unchanged when comparing builds.

GPU timing now includes a sampled per-pass timeline and the final surface blit, with a WGSL source catalogue,
rolling mean/p95 and structured periodic logs. Pass spans can overlap and must not be added as exclusive shader
costs. Unsupported timestamps are null, with queue latency kept separate. See the
[WebGPU profiler guide](render-profiler.md) and the
[wgpu timestamp query example](https://wgpu.rs/doc/wgpu_examples/timestamp_queries/index.html).

## Wider audit and remaining limits

- Splat count, screen coverage and transparency overdraw remain the principal GPU costs. Frustum/distance culling,
  LOD selection, the shared pool and depth prepasses remain in place. No extra opacity threshold, lower resolution
  or reduced detail budget was introduced to obtain the gains.
- Streaming already decodes one texture per browser frame and uses a reusable GPU pool. Assets already use a
  validator-aware browser cache. Decode/upload spikes and cold network loading are separate from steady-state FPS;
  a worker decoder or further upload scheduling needs its own cold-load traces before changing that pipeline.
- Native and browser builds share shaders and physics. The changes fit the portable eight-storage-buffer shader
  limit, covered by validation. Other GPUs/browsers still need their own measurements.
- The collision cache removes repeated work but does not reduce collision fidelity. Dynamic driving, first-time
  tile construction and high-speed level traversal can cost more than the settled physics probe.
- More aggressive tile rasterization, preprojected Gaussian buffers, transparency approximations and reduced SH
  are possible future experiments. They carry memory, portability or image-quality tradeoffs and are not part of
  this optimization.
- Release compiler flags, network formats, the audio pipeline and source assets were not changed. Build/startup
  time and audio latency are not included in FPS claims.

Regression coverage includes GPU sort/order and indirect arguments, wheel/body/caliper deformation, specular
coverage, car sharpening/occlusion, HUD cache/blinks, DOF bypass equivalence, timestamp readback and bounded pending
frames, plus the existing physics, collision and driving tests.

## Repeatable browser comparisons

Build the browser module, then retain a baseline with `python3 scripts/save_browser_baseline.py`.
The snapshot stays under ignored `logs/browser-benchmark`; the helper refuses accidental replacement.
Open `web/benchmark.html?level=saburi-alley-yazd&build=baseline&label=before&frames=120` on the local server.
After changing/rebuilding the engine, run the same page with `build=current&label=after`.
The harness uses the production WASM viewer, fixes the canvas at 1280×720 and the scene at LOD 1,
pauses/settles physics, disables random grain/debug overlays and keeps the configured visual effects.
It warms up 40 submitted frames, respects GPU backpressure, measures 120 submitted frames and then goes idle.
Results include the exact WASM SHA-256, scene occupancy, CPU submission samples, presentation intervals and
per-pass GPU statistics. Loading/compilation are excluded. Alternate before/after runs; don't benchmark
while building or running other GPU tests. Treat frame throughput and overlapping pass spans separately.

`web/post-validation.html` compares the production post shader with the saved baseline on real browser WebGPU,
using grey ramps, colours, car/background depth fixtures, clarity, partial grade and peripheral blur.
It reads back both outputs and requires every channel to differ by at most one 8-bit code value.
These diagnostic pages are local developer tools; the Pages build does not ship them.

Asset-cache checks: `node --experimental-default-type=module --test scripts/asset-cache.test.mjs`.
With the local server running, `node --experimental-default-type=module scripts/asset-cache-benchmark.mjs <output.json>`
compares cold/warm requests against commit `6346b7b` using actual Saburi chunk files and payload hashes.
Cold assets now skip the unnecessary HEAD and use the GET response's validator; warm entries still validate.
One cache handle is reused per page, and unavailable/unreadable caches fall back to network loading.
The asset profiler reports validator requests, payload network requests and cache opens separately.

## Direct surface presentation

The final post, debug lines and HUD can draw directly into the acquired sRGB surface. This removes the old
post-target-to-surface blit without changing the scene/depth/DOF passes. Offscreen rendering/readback remains
available, and the original copy path is retained for paired checks: `perf present direct` / `perf present copy`.
`perf` reports the selected presentation path and surface format. Format-specific final pipelines preserve the
browser's BGRA8 sRGB surface and the RGBA8 sRGB offscreen target. Resize reuses these pipelines and rebuilds inputs.
At 1280×720 the direct path avoids an intermediate 3.69 MB colour write plus a 3.69 MB sampled read per frame;
the optional readback/copy texture remains allocated. The profiler omits `present_blit` on the direct path.

The initial 120-frame browser Saburi pair (configured dashboard and car spec, fixed LOD 1) changed median GPU frame
time from 21.168 to 21.103 ms. That small difference is not a demonstrated FPS gain. The deterministic acceptance
evidence is one fewer full-screen pass and byte-identical 1280×720 output including the dashboard. Raw paired
measurements and images are retained in the optimization session evidence. Use `presentation=copy` or `direct`
on the browser benchmark; `verify=1` additionally checks two resize sizes and debug overlays after measurements.
