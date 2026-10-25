// Screen effects over the finished scene, before the HUD:
//
// - peripheral blur: samples along the line to the screen centre, weighted towards the edges, so speed smears the
//   periphery while the middle (the car) stays sharp;
// - colour fringing: red and blue sampled slightly outwards and inwards, growing towards the edges (chromatic
//   aberration);
// - the grade: a filmic contrast curve, teal shadows and warm highlights ("teal and orange"), a little saturation and
//   a soft vignette;
// - film grain: luminance-weighted noise that changes every frame.
//
// Every effect scales with its uniform; all zero is a plain copy.

struct PostUniforms {
    // x: peripheral blur (0..1), y: fringing (uv units at the corner), z: grain, w: grade (0..1).
    strength: vec4f,
    // x: time in seconds, y: 1 / width, z: 1 / height, w: unused.
    frame: vec4f,
    // x: contrast (S-curve mix), y: saturation, z: vignette, w: where the blur starts (screen radius).
    grade: vec4f,
    // rgb: tint added to the shadows; w: blur streak length (uv).
    shadow_tint: vec4f,
    // rgb: tint added to the highlights.
    highlight_tint: vec4f,
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> post: PostUniforms;

struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOut;
    out.position = vec4f(uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

const BLUR_TAPS: i32 = 10;

fn hash(p: vec2f) -> f32 {
    let q = fract(p * vec2f(443.897, 441.423));
    let r = q + dot(q, q.yx + 19.19);
    return fract((r.x + r.y) * r.x);
}

fn luminance(c: vec3f) -> f32 {
    return dot(c, vec3f(0.2126, 0.7152, 0.0722));
}

// Samples the scene with colour fringing: red pushed out, blue pulled in, along the line from the centre.
fn fringed(uv: vec2f, offset: vec2f) -> vec3f {
    let r = textureSampleLevel(scene, scene_sampler, uv + offset, 0.0).r;
    let g = textureSampleLevel(scene, scene_sampler, uv, 0.0).g;
    let b = textureSampleLevel(scene, scene_sampler, uv - offset, 0.0).b;
    return vec3f(r, g, b);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4f {
    let uv = in.uv;
    let from_centre = uv - vec2f(0.5);
    // 0 at the centre, 1 at the edge midpoints, about 1.4 in the corners.
    let radius = length(from_centre * 2.0);
    let fringe = from_centre * radius * post.strength.y;

    // Peripheral blur: a short streak towards the centre, only outside the middle of the screen.
    let edge = smoothstep(post.grade.w, 1.1, radius) * post.strength.x;
    var colour = vec3f(0.0);
    if (edge > 0.001) {
        let step = -from_centre * edge * post.shadow_tint.w / f32(BLUR_TAPS);
        var total = 0.0;
        for (var i = 0; i < BLUR_TAPS; i = i + 1) {
            let w = 1.0 - f32(i) / f32(BLUR_TAPS);
            colour += fringed(uv + step * f32(i), fringe) * w;
            total += w;
        }
        colour /= total;
    } else {
        colour = fringed(uv, fringe);
    }

    // Grade and grain in display space (the scene texture is sRGB, so samples arrive linear).
    colour = pow(max(colour, vec3f(0.0)), vec3f(1.0 / 2.2));

    // The grade, blended in by its strength.
    let grade = post.strength.w;
    if (grade > 0.0) {
        // Filmic S-curve on each channel, keeping black and white where they were.
        let curved = colour * colour * (3.0 - 2.0 * colour);
        var graded = mix(colour, curved, post.grade.x);
        // Teal in the shadows, warm orange in the highlights (by default; the tints are settings).
        let l = luminance(graded);
        graded += post.shadow_tint.rgb * (1.0 - smoothstep(0.0, 0.5, l)) + post.highlight_tint.rgb * smoothstep(0.45, 1.0, l);
        // Saturation, then a soft vignette.
        graded = mix(vec3f(luminance(graded)), graded, post.grade.y);
        graded *= 1.0 - post.grade.z * smoothstep(0.55, 1.45, radius);
        colour = mix(colour, clamp(graded, vec3f(0.0), vec3f(1.0)), grade);
    }

    // Film grain, stronger in the mid-tones, new every frame.
    if (post.strength.z > 0.0) {
        let pixel = floor(uv / vec2f(post.frame.y, post.frame.z));
        let n = hash(pixel + fract(post.frame.x * 0.61) * 1000.0) - 0.5;
        let l = luminance(colour);
        colour += vec3f(n * post.strength.z * (0.4 + 4.0 * l * (1.0 - l)));
    }
    return vec4f(pow(clamp(colour, vec3f(0.0), vec3f(1.0)), vec3f(2.2)), 1.0);
}
