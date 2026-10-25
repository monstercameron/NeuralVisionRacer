// Order-preserving depth keys for the radix sort. Ascending keys follow ascending view-space z, so the
// farthest splat (most negative z, since the camera looks down -z) sorts first: the back-to-front order
// that premultiplied "over" blending needs.
//
// Splats whose centre is behind the camera, or well outside the view, and empty slots get the largest key, so they
// sort after every visible splat; `visible` counts the rest, and `drawArgs` turns that count into the instanced draw's arguments, so
// the draws only shade the visible prefix of the order.
//
// The cutout (the level only): splats between the camera and the car, within the car's outline on screen, are culled
// the same way, so walls and posts open a hole and the car stays in view. The edge dissolves over a band (a per-splat
// hash decides), and splats below `floor_y` in world space (the road) are never cut.
//
// Crossfades (a streamed level changing a leaf's level of detail): `fades` lists slot ranges, sorted by start, each
// with the share of its opacity to draw. Every splat's share goes into `fade` (1 outside the ranges), which the splat
// shaders multiply into its opacity, so a new range fades in smoothly over the old one, which then fades out. A
// splat at share 0 is left out of the draw.

struct DepthUniforms {
    view_model: mat4x4f,
    clip: mat4x4f,
    // Splat space to world space, for the cutout's floor test.
    cut_model: mat4x4f,
    // xyz: the car's centre in view space; w: how far a splat reaches past its centre (metres).
    cut_target: vec4f,
    // x: hole radius (metres at the car), y: world height kept below, z: depth margin in front of the car,
    // w: the soft edge's outer radius as a multiple of the hole's (0 when the cutout is off).
    cut_params: vec4f,
    count: u32,
    fade_count: u32,
};

@group(0) @binding(0) var<uniform> params: DepthUniforms;
@group(0) @binding(1) var<storage, read> centers: array<f32>;
@group(0) @binding(2) var<storage, read_write> keys: array<u32>;
@group(0) @binding(3) var<storage, read_write> visible: atomic<u32>;
// x: first slot, y: one past the last, z: the share drawn (f32 bits).
@group(0) @binding(4) var<storage, read> fades: array<vec4u>;
// Per splat: the share of its opacity drawn.
@group(0) @binding(5) var<storage, read_write> fade: array<f32>;

// A centre this far outside the view (in units of the half-screen) can still reach into it with a large splat.
const CULL_MARGIN: f32 = 1.5;


fn hash01(i: u32) -> f32 {
    var x = i * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    x = (x >> 22u) ^ x;
    return f32(x) / 4294967295.0;
}

// True when the splat at view-space `p` (model-space `c`, index `i`) hides the car and should be cut.
fn in_cutout(p: vec3f, c: vec3f, i: u32) -> bool {
    let cut = params.cut_params;
    let car = params.cut_target.xyz;
    if (cut.w <= 1.0 || car.z >= -0.01) {
        return false;
    }
    // Only what is between the camera and the car (closer than the car, less a margin) and in front of the camera.
    if (p.z <= car.z + cut.z || p.z >= 0.0) {
        return false;
    }
    // The road stays.
    if ((params.cut_model * vec4f(c, 1.0)).y < cut.y) {
        return false;
    }
    // Angular distance from the car's centre on screen, against the car's angular radius.
    let d = length(p.xy / -p.z - car.xy / -car.z);
    // Near the camera a splat covers a large angle, so a splat whose centre is just outside the hole would still
    // cover it; the hole is widened by the splat's reach seen from its depth.
    let r = cut.x / -car.z + params.cut_target.w / -p.z;
    if (d <= r) {
        return true;
    }
    let t = (d - r) / (r * (cut.w - 1.0));
    return t < 1.0 && hash01(i) > t;
}

// The share of splat `i`'s opacity drawn now: its fading range's share, or 1.
fn fade_share(i: u32) -> f32 {
    // The last range starting at or before i.
    var lo = 0u;
    var hi = params.fade_count;
    while (lo < hi) {
        let mid = (lo + hi) / 2u;
        if (fades[mid].x <= i) {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    if (lo == 0u) {
        return 1.0;
    }
    let f = fades[lo - 1u];
    return select(1.0, bitcast<f32>(f.z), i < f.y);
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if (i >= params.count) {
        return;
    }
    // An empty slot (released by a streamed level) has a NaN centre. Comparisons with NaN are all false, so the
    // tests below would let it through and its stale splat would still be drawn; the exponent bits catch it.
    if ((bitcast<u32>(centers[i * 3u]) & 0x7f800000u) == 0x7f800000u) {
        fade[i] = 0.0;
        keys[i] = 0xffffffffu;
        return;
    }
    let share = fade_share(i);
    fade[i] = share;
    let c = vec3f(centers[i * 3u], centers[i * 3u + 1u], centers[i * 3u + 2u]);
    let p = params.clip * vec4f(c, 1.0);
    if (p.w <= 0.0 || abs(p.x) > p.w * CULL_MARGIN || abs(p.y) > p.w * CULL_MARGIN || share <= 0.0) {
        keys[i] = 0xffffffffu;
        return;
    }
    let v = (params.view_model * vec4f(c, 1.0)).xyz;
    if (in_cutout(v, c, i)) {
        keys[i] = 0xffffffffu;
        return;
    }
    atomicAdd(&visible, 1u);
    let bits = bitcast<u32>(v.z);
    // Flip the sign bit for positives, and every bit for negatives, so unsigned order equals float order.
    keys[i] = select(bits | 0x80000000u, ~bits, (bits & 0x80000000u) != 0u);
}
