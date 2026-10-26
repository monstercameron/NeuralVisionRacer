// Depth of field: everything nearer than the focus distance stays sharp (the car, the road ahead), and the scene
// softens beyond it, up to `radius` pixels of blur `range` metres further on. Far away the streamed level is at its
// coarsest levels of detail, and the blur hides their blobs and seams.
//
// Each pixel's distance is the nearer of the level's and the car's depth prepasses, turned back into metres with the
// projection. The blur gathers a golden-angle spiral of taps around the pixel, but only from taps at least about as
// far as the pixel itself, so a sharp foreground (the car, a post) never smears into the soft background behind it.

struct DofUniforms {
    // x: strength 0..1, y: focus distance (m), z: range to full blur (m), w: full blur radius (pixels).
    params: vec4f,
    // x, y: projection[10], projection[14] (view distance = y / (depth + x)), z: 1 / width, w: 1 / height.
    proj: vec4f,
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var depth_level: texture_depth_2d;
@group(0) @binding(3) var depth_car: texture_depth_2d;
@group(0) @binding(4) var<uniform> dof: DofUniforms;

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

const TAPS: i32 = 16;
const GOLDEN_ANGLE: f32 = 2.39996323;
// Nothing written (the sky): treated as very far.
const SKY_M: f32 = 1.0e5;

fn depth_to_m(d: f32) -> f32 {
    if (d >= 1.0) {
        return SKY_M;
    }
    return dof.proj.y / (d + dof.proj.x);
}

fn distance_at(px: vec2i) -> f32 {
    let size = vec2i(textureDimensions(depth_level));
    let p = clamp(px, vec2i(0), size - vec2i(1));
    return depth_to_m(min(textureLoad(depth_level, p, 0), textureLoad(depth_car, p, 0)));
}

// The car's depth prepass only covers its nearly opaque splats; its soft outline would read as the distance behind it
// and smear into a halo. Its depth is grown by `CAR_GROW` pixels (the nearest within a small cross), so the whole
// outline counts as the car and stays sharp.
const CAR_GROW: i32 = 4;

fn centre_distance(px: vec2i) -> f32 {
    let size = vec2i(textureDimensions(depth_car)) - vec2i(1);
    var car = textureLoad(depth_car, clamp(px, vec2i(0), size), 0);
    for (var k = 1; k <= 2; k = k + 1) {
        let o = CAR_GROW * k / 2;
        car = min(car, textureLoad(depth_car, clamp(px + vec2i(o, 0), vec2i(0), size), 0));
        car = min(car, textureLoad(depth_car, clamp(px - vec2i(o, 0), vec2i(0), size), 0));
        car = min(car, textureLoad(depth_car, clamp(px + vec2i(0, o), vec2i(0), size), 0));
        car = min(car, textureLoad(depth_car, clamp(px - vec2i(0, o), vec2i(0), size), 0));
    }
    return min(distance_at(px), depth_to_m(car));
}

// Blur radius in pixels for a distance.
fn blur_px(distance: f32) -> f32 {
    let p = dof.params;
    return p.x * p.w * smoothstep(p.y, p.y + max(p.z, 0.01), distance);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4f {
    let base = textureSampleLevel(scene, scene_sampler, in.uv, 0.0);
    let px = vec2i(in.position.xy);
    // Growing the car mask can only bring the focus distance nearer. A pixel that is already sharp
    // stays sharp, so skip the eight extra car-depth taps across the road and other in-focus areas.
    if (blur_px(distance_at(px)) < 0.5) { return base; }
    let here = centre_distance(px);
    let radius = blur_px(here);
    if (radius < 0.5) {
        return base;
    }
    let texel = dof.proj.zw;
    var sum = base;
    var total = 1.0;
    for (var i = 0; i < TAPS; i = i + 1) {
        // Evenly spread over the disc: radius grows with the square root of the tap index.
        let r = sqrt((f32(i) + 0.5) / f32(TAPS)) * radius;
        let a = f32(i) * GOLDEN_ANGLE;
        let offset = vec2f(cos(a), sin(a)) * r;
        let there = distance_at(px + vec2i(round(offset)));
        // Only gather from what is at least about as far: the sharp foreground stays out of the blur.
        if (there >= here * 0.85) {
            sum += textureSampleLevel(scene, scene_sampler, in.uv + offset * texel, 0.0);
            total += 1.0;
        }
    }
    return sum / total;
}
