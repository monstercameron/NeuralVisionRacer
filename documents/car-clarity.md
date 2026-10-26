# Car clarity

The car has an edge-limited unsharp mask plus wider local contrast in `splat-render/src/shaders/post.wgsl`.
The visible car depth masks it; level depth excludes walls and posts in front. The mask is eroded at silhouettes
to avoid bright outlines. The adjustment is bounded and fades where deliberate speed blur is active. The HUD
is drawn afterward and stays untouched. Native and WASM use the same shader.

Faint splat contributions are also suppressed before blending using `alpha = max(0, (alpha - cutoff) / (1 - cutoff))`.
Only the car scene enables the cutoff. Opaque cores retain full opacity; the colour and depth passes use the same
mapping. This reduces faint tails and floaters without editing the SOG. It cannot recover missing scan detail or
remove opaque reconstruction blobs. High cutoff values can thin glass and small details, so the default is modest.

Live controls under `[effects]`:

| Setting | Default | Range | Purpose |
| --- | ---: | ---: | --- |
| `car_sharpen` | 0.85 | 0–1 | Fine edge-limited detail |
| `car_clarity` | 0.18 | 0–0.5 | Wider local contrast |
| `car_haze_cutoff` | 0.02 | 0–0.08 | Remove faint alpha contributions |

For example, `set effects.car_sharpen 0.85` applies immediately. Set all three to zero for the original rendering.
The stronger defaults bring out body-panel edges, lettering and rim detail while the existing depth mask and
bounded contrast keep sharpening off the background and avoid bright silhouette halos.
Disabling `effects.enabled` disables them too. Screenshots used for comparison hold the camera and simulation fixed
and disable film grain in both images, so changing grain patterns do not obscure the difference.

Validation includes Naga shader composition, native GPU rendering and WASM compilation. A GPU postprocessing test
checks that the effect changes visible car detail, leaves background pixels unchanged, and respects level occlusion.
