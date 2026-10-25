# Splat Racer — refined technical plan

Revision of `research.md` (researched October 24, 2026). This version corrects claims the original overstated, adds verified version and licence facts, records what remains unresolved, and turns the milestones into gated experiments with pass/fail criteria. Nothing here is a benchmark result. No Rust build, shader port, training run or paid API call has been performed.

## 1. Summary of changes from the original

| Item | Original | Refined | Source |
| --- | --- | --- | --- |
| `rapier3d` version | 0.36.1 | Confirmed: newest on crates.io, published 2026-10-10 | [crates.io API](https://crates.io/api/v1/crates/rapier3d) |
| OpenAI TTS shutdown | Jan 6, 2027 | Confirmed for `tts-1`, `tts-1-hd`, `gpt-4o-mini-tts` snapshots. Legacy realtime and audio models shut down **Jan 20, 2027** (missing from original) | [OpenAI deprecations](https://developers.openai.com/api/docs/deprecations) |
| OpenAI replacements | `gpt-realtime-2.1-mini` for all | TTS → `gpt-realtime-2.1-mini`; realtime → `gpt-realtime-2.1` / `-mini`; audio → `gpt-audio-1.5` | same |
| "Not a drop-in swap" for `/v1/audio/speech` | Stated as fact | Reported by secondary sources only; verify against the speech guide before relying on it | secondary sources |
| ElevenLabs commercial use | Paid plan required | Confirmed. Free tier has no commercial licence; Starter ($6/mo monthly, $5/mo annual) includes it | [pricing](https://elevenlabs.io/pricing), [TTS FAQ](https://elevenlabs.io/docs/overview/capabilities/text-to-speech) |
| ElevenLabs attribution on free tier | Stated | **Removed.** Only in a third-party blog; not on the official pages checked | — |
| SOG format | v2 | Confirmed v2. Lossless WebP is the default; other 8-bit RGBA formats are allowed; readers must accept directory or ZIP layouts | [SOG spec](https://developer.playcanvas.com/user-manual/gaussian-splatting/formats/sog/) |
| Brush status | "Exports splats" | README does not call it a proof of concept; browser support is Chrome 134+ on Windows/macOS. It loads `.ply` and `.compressed.ply`; **SOG export is not documented**, so SOG must come from SplatTransform | [Brush README](https://github.com/ArthurBrussee/brush) |
| Rust splat renderers | Not surveyed | `wgpu_3dgs_viewer` reads PLY and SPZ, not SOG. Spark (JavaScript) reads SOG. No Rust crate found that reads SOG | [docs.rs](https://docs.rs/wgpu-3dgs-viewer), [Spark](https://github.com/sparkjsdev/spark) |
| Articulated car rendering | Proposed | No 2025–2026 real-time precedent found for per-wheel articulation in splats. Treat as a research risk | search only |

## 2. Recommended stack (revised)

| Area | Choice | Confidence | Note |
| --- | --- | --- | --- |
| Runtime | Rust, `wgpu`, native + `wasm32-unknown-unknown` | High | Keep one codebase; test WebGPU in the target browser early |
| Splat renderer | Custom `wgpu` renderer; port PlayCanvas WGSL *logic*, not text | Medium | No existing Rust crate reads SOG; a decoder must be written or ported |
| Physics | `rapier3d` 0.36.1, pinned, `Cargo.lock` committed | High for the library; medium for the vehicle controller | Vehicle controller ported from Bullet's `btRaycastVehicle`; braking code is not shown in the guide |
| Track collision | SplatTransform voxels → `.collision.glb` → Rapier trimesh | Medium | Expect stepped contacts; a hand-authored invisible road surface is the likely fallback |
| Training | Brush for local experiments; gsplat (CUDA) if Brush proves insufficient | Medium | Brush is a fit for the Rust/WebGPU direction but its export path needs checking |
| Delivery | Single-file SOG v2 bundle | High | Streamed SOG only after measurement |
| Audio | Pre-generated ElevenLabs assets on a paid plan; local engine/skid synthesis from state; OpenAI Realtime only for optional dialogue | Medium | Plan tier and voice rights must be confirmed before shipping |

Decision on the renderer: the original plan's "port PlayCanvas" is still right, but the scope is larger than it first looked. The PlayCanvas WGSL is a template language inside JavaScript strings, and the host code (sorting, resource lifetime, SOG textures) matters as much as the shader maths.

## 3. Verified facts

**Rapier.** `rapier3d` 0.36.1 is the newest version on crates.io and was published 2026-10-10. Features relevant to this project: `enhanced-determinism`, `simd8`, `parallel`, `f32`, `dim3`, `serde-serialize`. Recommendation: enable `f32` and `dim3`; leave `parallel` off for WASM until shared-memory deployment is configured; test `enhanced-determinism` only if replays or networking need it.

**Rapier vehicle controller.** `DynamicRayCastVehicleController` models one dynamic chassis with raycast wheels and spring suspension. Wheels are added in chassis-local space with a position, suspension direction, axle, rest length and radius, and tuned with `WheelTuning` (stiffness, damping, grip). Per step: set engine force, steering and brake per wheel, call `update_vehicle(dt, queries)`, then `world.step()`. Ray casts must exclude the chassis. The guide states no limitations.

**SOG v2.** Format version 2 is the one the spec describes. Textures should be lossless WebP to keep quantised values exact. A bundled SOG is a ZIP of the same files. Readers must check `version`.

**OpenAI.** Scheduled shutdowns, as listed on the deprecations page:

| Date | Models | Replacement |
| --- | --- | --- |
| Jan 6, 2027 | `tts-1`, `tts-1-hd`, `gpt-4o-mini-tts` snapshots | `gpt-realtime-2.1-mini` |
| Jan 20, 2027 | `gpt-realtime` family, `gpt-4o-realtime`, `gpt-audio` family | `gpt-realtime-2.1`, `gpt-realtime-2.1-mini`, `gpt-audio-1.5` |
| Feb 26, 2027 | `whisper-1`, `gpt-4o-transcribe` family | `gpt-live-transcribe` or `gpt-transcribe` |

The deprecations page does not say whether `/v1/audio/speech` survives in any form after January 6, 2027. Check the speech guide before designing the callout pipeline around it.

**ElevenLabs.** Free tier: no commercial licence. Starter: commercial licence included, $6/month monthly or $5/month annual, with a promotional first month. Text-to-speech FAQ: commercial rights require a paid plan; the user keeps ownership of generated audio. The sound-effects docs page contains no licence terms. Confirm the licence for SFX specifically with ElevenLabs before shipping.

**Brush.** Targets macOS, Windows, Linux, Android and browser. Browser build uses `wasm-pack`, and the README says Chrome 134+ on Windows and macOS. Loads `.ply` and `.compressed.ply`. No SOG export documented. The Releases section was empty in the fetched copy, so no current version is confirmed.

## 4. Unresolved gaps

| Gap | Why it matters | How to close it |
| --- | --- | --- |
| Brush's training quality and speed on our footage | Determines whether we use it or gsplat | Train one short track section and one car; compare PSNR/visual result and wall-clock time |
| Brush's export path | Needed for SOG pipeline | Confirm PLY export; convert with SplatTransform 3.10.3 |
| Real-time articulated wheels in splats | Car is the visual centrepiece | Prototype early (Milestone 2); fall back to a rendered mesh for wheels if the splat wheels look wrong |
| Rapier braking | Braking code not shown | Read `ray_cast_vehicle_controller.rs` and the example before Milestone 3 |
| Collision fidelity from scans | Driving may feel rough | Measure tire contact on a test section; test the hand-authored road fallback |
| ElevenLabs SFX licence | Shipping rights | Check ElevenLabs terms for sound effects specifically |
| OpenAI speech endpoint after Jan 2027 | Affects optional callouts | Check the speech guide and changelog |
| Commercial licences of community scenes | Shipping rights | Use only self-captured or explicitly licensed assets |

## 5. Gated milestones

Each gate has a pass condition. Do not start the next gate until the current one passes or its fallback is chosen.

1. **Renderer gate.** Render one known Gaussian PLY natively and in WASM with the same camera. Pass: projection, alpha, SH colour and sort order match a PlayCanvas reference frame within an agreed tolerance on the same asset. Fallback: restrict to PLY and defer SOG.
2. **Decode gate.** Decode SOG v2 (directory and ZIP) and match the SplatTransform round-trip counts and bounds. Pass: no attribute misalignment on the test asset. Fallback: use SplatTransform's decode output as an intermediate PLY.
3. **Car gate.** Render separate body and wheel objects over a static track, with wheel spin and steer. Pass: no visible depth popping at car–road contact at 60 FPS on the chosen baseline. Fallback: mesh wheels with splat body.
4. **Driving gate.** Import one Rapier trimesh collider and drive the raycast vehicle. Pass: no tunnelling at target speed, stable resting on the road, recovery from a reset. Fallback: hand-authored road surface.
5. **Asset gate.** Train one short track and one car with Brush (gsplat if Brush fails the quality check). Pass: visual comparison accepted by the team; provenance recorded.
6. **Audio gate.** Generate the asset set and blend engine audio locally from RPM and load. Pass: no audible loop seams at the tested speeds.
7. **Scale gate.** Measure visible splat count, sort time, overdraw and GPU memory on target hardware. Only then consider streamed SOG.

Provisional targets (unmeasured): 60 Hz fixed physics with interpolated rendering; 60 FPS on the chosen baseline. Set the hardware baseline before gate 1 so the targets can be checked.

## 6. Open decisions

1. Browser or native first?
2. Arcade or simulation handling? (Affects tuning and the need for tire models.)
3. Which track section and car to capture first?
4. Hardware baseline for performance gates.
5. Custom `wgpu` runtime, or Bevy? The gated plan above assumes custom `wgpu`.

## 7. Risks

- **Scope of the PlayCanvas port.** Shader and sort logic are coupled to JavaScript host code. Mitigation: port one vertical slice (PLY, no SH, then SH, then SOG) rather than the whole pipeline.
- **Splat geometry is not physical.** Scans have holes, glass and missing road edges. Mitigation: invisible physics surfaces.
- **Licence provenance.** Community scenes and generated audio carry separate terms. Mitigation: per-asset records of source, licence version and credit.
- **API churn.** OpenAI shutdowns are scheduled inside six months. Mitigation: keep provider and model configurable; cache generated callouts.

## 8. Method and limits

Sources were checked with web search and direct fetches of official pages on October 24, 2026. Where a secondary source was the only source, it is marked. Vendor pages were not tested by use. No claim in this document is a measured performance result.
