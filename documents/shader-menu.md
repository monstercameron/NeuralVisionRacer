# Pause-menu shader controls

Press Esc in the browser to open the pause menu. Twenty-four individual switches plus the effects master apply through the engine console settings on the next frame. Numeric strength switches remember the last nonzero value, including console edits. Restore settings returns all switches and strengths to their values when the level loaded. Choices last until reload.

Scene controls cover the sky, level and car, plus the car visibility cutout. Disabling the car or level skips its preparation, sort/deformation compute, depth draws and colour draws; depth buffers still clear to prevent phantom occlusion. Hiding the car also removes its shadow/cutout. GPU assets stay resident for immediate restoration. Streaming and physics continue normally.

Car/road controls cover wheel animation, shadows, specular highlights, sharpening, clarity, haze removal, tire marks and dust. Post controls cover DOF, speed blur, chromatic aberration, grain and the complete colour grade. Camera/overlay controls include shake, FOV, pullback, dashboard and collision/LOD/wheel debug views. The effects master bypasses its members without losing their individual choices.

These are visual feature switches. Combined shader stages such as grade and sharpening remain one measured post pass; neutral settings still require the final image copy/presentation. Sorting and depth are controlled with their parent scene to retain valid blending and occlusion. The profiler shows actual conditional execution rather than estimating separate timings for shared functions.

New live config paths: render.sky, render.level, render.car, effects.wheels. Existing effects.*, render.car_cutout, HUD and debug paths also apply live. Both native and WASM rendering honour the new paths; the pause-menu UI is browser-specific.

Verification: all 24 individual browser controls exercised off/on; master bypass, restore, Escape/resume and retention of a custom shadow strength (0.37) checked. Four renderer profiling/visibility tests pass, including shader-work omission, stale depth clearing, unchanged pixels after restore and timestamp sampling. All 62 engine unit tests pass; native check and release WASM build pass.
