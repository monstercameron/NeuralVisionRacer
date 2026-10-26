# Cinematic colour grade

The default look now uses cooler steel/teal shadows, warmer sunlight, stronger luminance contrast, a soft highlight shoulder and restrained colour density. A near-linear toe keeps shaded road detail readable; a small neutral black floor avoids tinting true blacks. Bright highlights desaturate gently, and gamut compression preserves hue instead of clipping channels. The preset has a soft vignette and finer grain.

The grade uses explicit sRGB transfer functions and performs exposure/white-balance adjustments in linear light. Contrast and split tones work in display space. White highlights are protected from the colour cast. The same production WGSL shader runs in native wgpu and browser WebGPU. The HUD is drawn after grading.

No extra render pass, texture or blur samples were added. The post uniform grows from 96 to 112 bytes; the profiler still measures the existing `post` pass and now exports the grade profile/controls. The grade adds arithmetic to that pass; no universal FPS improvement or zero-cost claim is made.

## Live controls

All grade controls are live; defaults are in `engine.conf` and `EngineConfig`.

```text
set effects.grade 0                 # ungraded image
set effects.grade 1                 # cinematic preset strength
set effects.grade_exposure_ev 0.10   # -3..3 stops
set effects.grade_temperature 0.16  # -1..1, cool to warm
set effects.grade_tint 0            # -1..1, green to magenta
set effects.grade_contrast 0.82     # 0..1
set effects.grade_saturation 0.96    # 0..2
set effects.grade_shoulder 0.60      # 0..1, highlight compression
set effects.grade_black_lift 0.008  # 0..0.04
set effects.grade_vignette 0.20     # 0..0.65
set effects.grain 0.018
```

Shadow/highlight tint vectors remain configurable. Grade strength blends continuously. Grade zero bypasses the colour correction; grain, sharpening and other effects retain their own controls. This is an SDR grade over scanned appearance, so it cannot recover highlight detail already clipped in the source/render target or change the level's baked lighting.

## Validation

38 renderer tests passed, including a new GPU colour-chart test checking the grade-off pixel round trip, monotonic grey ramp, neutral blacks/whites, shadow readability, warm/cool separation, highlight rolloff, grade-strength interpolation and opacity. Existing car-sharpening, shadow, depth, tire-mark and profiler tests passed. The 76 engine tests, native example checks and release WebAssembly build passed.

Matched production-renderer captures cover all four playable levels. They use the same camera, physics tick, source LOD, car, lighting and depth of field before/after. HUD and grain are disabled in these comparison captures to isolate the grade. The before image uses the previous shader/preset; the after image uses the new shader/preset. They are engine-rendered PNGs, not edited photographs. Saburi uses LOD 0, Delft and Park LOD 2, Basilica LOD 1, within a 3-million-splat whole-level preview budget.

The browser was also checked on Saburi: grade 0/1 switches without reloading, and `webgpu-profile.json` records `cinematic-v2`, full strength and the new controls. Normal grain and running gameplay were restored after comparison.
