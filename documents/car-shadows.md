# Shared sunlight for highlights and shadows (WebGPU / WGSL)

The per-level sun fitted from baked scan shadows now drives both added clearcoat and the car's projected shadow.
It already supplied their common direction; this change replaces the padded draw-bounds box with a surface-fitted
occlusion proxy and reuses that proxy to suppress added highlights when another part of the car blocks the sun.

At car load, the renderer reuses the packed normal reliability from the specular surface estimator. Planar,
reasonably opaque, compact Gaussians contribute to robust longitudinal cross-sections; reconstruction floaters
and unreliable normals are excluded. Seven overlapping ellipsoids approximate those sections. The fit uses the
original scan coordinates and follows the current car model transform, including body rotation and suspension
motion. It is independent of the enlarged screen-culling bounds.

Level splats test their ray towards the fitted sun against the proxy. Analytic penumbra coverage gives a soft
edge; Gaussian thin-axis normals reduce attenuation on grazing receivers. Ambiguous normals receive a moderated
weight instead of an arbitrary normal deciding their shadow. Attenuation is applied in linear light after gamma
decode. It changes RGB only: Gaussian opacity, depth coverage, source assets and the baked environment appearance
outside the projected footprint are preserved.

The same occlusion test gates visible clearcoat glints on the car. Its existing normal estimate supplies a surface
bias; a proxy enclosing the biased point is ignored to avoid false self-intersection. This affects the added
specular contribution, not the car's captured base RGB. Very small GGX contributions (maximum channel <= 0.002 in
linear light) bypass occlusion testing and retain their original contribution. Opaque occlusion exits early.

The lighting catalogue now records independent `shadow_strength_scale` and `shadow_softness` values. These are
artistic fits informed by the same scan evidence, not recovered radiometry or material roughness. Provisional
Basilica and Delft profiles use weaker, softer shadows. Rotating the level rotates the shared sun consistently;
manual `effects.sun` works when `effects.level_sun` is false. Disabling the added specular does not disable the
projected shadow. `effects.shadow = 0` disables the proxy shadow and its highlight occlusion.

The existing `shadow_box` uniform binding now holds the transform, light and seven shapes (320 bytes). No texture,
extra storage/uniform binding, shadow map or GPU pass is added. The shaders remain within WebGPU's portable binding
limits. The profiler's `shadow` object reports sun, strength, softness and fitted segment count, while
`scene_metrics.car.shadow_proxy` exposes the fitted scan-space bounds and ellipsoids.

Controls: `set effects.shadow 0` / `set effects.shadow 0.55`; `set effects.car_specular 0` / `1`;
`set effects.level_sun false` selects the manual direction. These controls apply live. `status` includes the resolved
lighting profile; `perf` and the Render profiler JSON export include shadow diagnostics.

This is a bounded approximation. It does not recover ambient/diffuse lighting, remove baked shadows, trace
buildings or trees, or reproduce exact spoiler/tyre silhouettes and individual wheel articulation. Full-body model
motion is tracked; the fitted proxy itself is static. Cars with too few reliable surface splats have no fitted
proxy, reported as zero segments. The shader does not invent a box fallback for those assets.

Validation covers opposite sun directions moving the shadow across the caster, opacity preservation, shadow-off
restoration, highlight suppression without changing captured base RGB, outlier-resistant fitting, per-level
profile behavior, WGSL validation and portable resource limits. The `perf_probe` example also saves per-variant
images; `CAMERA_HEIGHT=5 VARIANTS=base,no_shadow` gives an overhead visual comparison at fixed LOD.
