// Only the car scene enables this buffer; level scenes keep the cutoff at zero.
var<storage, read> carLight: array<vec4f>;

// Remove very faint Gaussian tails smoothly, preserving opaque cores. The same mapping is used
// before colour and depth coverage tests, so their silhouettes agree.
fn carClarityAlpha(alpha: f32) -> f32 {
    let cutoff = clamp(carLight[3].w, 0.0, 0.08);
    return clamp((alpha - cutoff) / (1.0 - cutoff), 0.0, 1.0);
}

fn modifySplatColor(gaussianUV: vec2f, color: ptr<function, vec4f>) {}
