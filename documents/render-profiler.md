# WebGPU render profiler

Open **Render profiler** at the top right of the browser game. It shows the latest sampled GPU passes, their
WGSL source files, rolling mean/p95 spans and CPU/streaming totals. **Download metrics JSON** exports the snapshot,
shader catalogue, active effects, pass workload counts, CPU statistics, simulation statistics and asset counters.
Opening the panel does not resize the canvas. The table refreshes once per second only while expanded.

Console commands work in the browser:

| Command | Result |
| --- | --- |
| `perf` | Full metrics snapshot with named CPU phases, simulation, assets, streaming and GPU passes. |
| `perf trace` | Latest GPU timeline plus rolling GPU statistics and source catalogue. |
| `perf shaders` | Pass registry, WGSL source paths and effects sharing each shader. |
| `perf sample 1` | Timestamp every rendered frame for a detailed investigation. |
| `perf sample 8` | Default: sample once every eight rendered frames. Valid range is 1–600. |
| `perf reset` | Clear GPU/CPU history and scene/sort counters; sampling continues. Asset-session counters remain. |
| `perf log off` / `perf log on` | Disable/enable periodic structured performance logs. Sampling remains active. |
| `logs 10` | Recent engine records, including structured performance snapshots. |

Every actual GPU compute/render pass has a stable ID and target. The registry covers sort/culling, wheel centre
deformation, sky, level/car depth, level/car colour, depth of field, post effects, LOD/collision debug lines, HUD,
and the final WebGPU surface blit. A cached sort or unchanged wheel pose issues no compute pass, so it is absent
from that sampled frame rather than assigned an invented zero cost. Clear-only depth passes can still exist
without a draw call. Shader files are relative to `crates/`; gsplat roots are composed with the local hook chunks.

The radix sort remains one batched compute pass, with its dispatch/workgroup counts and complete source list.
Likewise, sharpening, clarity, blur, fringing, grain and grading share the post shader; the profiler measures that
combined pass. To establish the incremental impact of one effect, compare full-frame metrics at the same camera
and settled LOD before and after toggling it, then restore it. A source appearing in a composed shader does not
mean every optional effect branch is enabled; the snapshot records active effect strengths separately.

## Timing semantics

GPU measurements use WebGPU `timestamp-query` through wgpu pass descriptors. They are **pass spans**, from the
pass's beginning timestamp to its end timestamp. They include dependency waits. GPU stages and passes can overlap,
so adding spans does not yield frame time and a large span is not an exclusive shader execution cost. The report
also contains the whole sampled frame's timestamp envelope (`gpu_frame_ms`), a measured-pass sum explicitly marked
non-additive, and pass offsets (`start_ms`) for timeline analysis. The final surface blit is included; compositor,
display/vsync time and queue uploads before the first pass are excluded.

CPU times, GPU execution spans and queue completion latency are separate. CPU statistics cover simulation ticks,
collision tiles, physics, ECS/events/camera, setup/decode, stream selection/upload, HUD/debug preparation and command
encoding/submission. Browser asset metrics cover payload requests routed through `bytes()`: downloaded/cache bytes,
cache hits, requests, failures, in-flight count and last request duration. These are payload counters rather than
wire bytes or totals for config/validator requests. Streaming includes residency, pool occupancy, pending chunks,
LOD/fade switches, SH palettes, detail coverage and scale. Scene counters include cached uniform writes, their
payload bytes, wheel-pose payload, streamed splats/bytes/releases and sort/cache counts. They are selected counters,
not a complete allocation tracker or measurement of all driver traffic.

Timestamp results are asynchronous and tagged with a sampled frame ID. The default three readback slots never
block rendering; a busy slot skips the sample. History contains at most 120 sampled frames and each CPU series
contains at most 120 observations. At most 256 passes can be timed per sampled frame; overflow and readback errors
are explicit counters. Rare unmeasurable pass timestamps are `null` with `timestamp_unavailable`, not zero.
Adapters without timestamp-query keep CPU/counter reporting, expose queue latency as a fallback, and leave GPU
pass timings unavailable. Passes absent from the latest sample do not imply the effect never runs.

## Logs and exports

The browser keeps logs in the existing bounded in-memory engine log. Rendering snapshots are logged at most once
per second as `module: "perf"`; running simulation snapshots are logged once per simulated second as
`module: "perf.simulation"`. Metrics are JSON objects under `data`, not escaped JSON inside `msg`. They can be read
with `logs` or exported with the dashboard button. Browser logs are local; nothing is sent to a telemetry service.
`perf log off` disables those periodic records, not normal game logs or GPU queries.

The native shared renderer also supports the console commands and writes structured metrics to the configured
engine JSONL file. The release `perf_probe` example saves `profile-<variant>-<mode>.json` alongside raw frame data;
`GPU_SAMPLE_EVERY=1` requests detailed timestamps. These native captures are separate from browser measurements.

## Validation

GPU regression tests verify conditional pass presence, finite timings, bounded submissions/readbacks, reset/control
behavior and identical rendered pixels with/without detailed sampling. CPU tests verify bounded windows and p95
calculation; logger tests verify that metrics remain structured through JSONL and the console log tail. Native and
WebAssembly builds share the pass instrumentation. Browser startup, live table, commands and JSON export should be
checked after rebuilding `web/pkg` with wasm-bindgen.
