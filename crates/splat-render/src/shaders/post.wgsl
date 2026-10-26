// Screen effects over the finished scene, before the HUD:
//
// - peripheral blur: samples along the line to the screen centre, weighted towards the edges, so speed smears the
//   periphery while the middle (the car) stays sharp;
// - colour fringing: red and blue sampled slightly outwards and inwards, growing towards the edges (chromatic
//   aberration);
// - the grade: luminance contrast, a soft highlight shoulder, warm light/cool shadows,
//   exposure/balance, highlight desaturation, gamut compression and lens falloff;
// - film grain: luminance-weighted noise that changes every frame.
//
// Every effect scales with its uniform; all zero is a plain copy.

struct PostUniforms {
    // x: peripheral blur (0..1), y: fringing (uv units at the corner), z: grain, w: grade (0..1).
    strength: vec4f,
    // x: time in seconds, y: 1 / width, z: 1 / height, w: highlight shoulder.
    frame: vec4f,
    // x: luminance contrast, y: saturation, z: vignette, w: where the blur starts (screen radius).
    grade: vec4f,
    // rgb: tint added to the shadows; w: blur streak length (uv).
    shadow_tint: vec4f,
    // rgb: tint added to the highlights.
    highlight_tint: vec4f,
    // x: car sharpening, y: wider local contrast, z: portal strength, w: rim width at 1080p.
    car: vec4f,
    // x: exposure stops, y: warm/cool, z: magenta/green, w: neutral black floor.
    film: vec4f,
    // Projected car centre/radii (uv); a soft region fills holes in the core depth mask.
    car_region: vec4f,
    // xy: angular car centre, z: angular aperture radius, w: car view depth.
    portal_target: vec4f,
    // Soft radius multiple, near-splat reach, foreground depth margin, preserved world floor height.
    portal_cut: vec4f,
    // Inverse projection xy scale and projection z coefficients for depth reconstruction.
    portal_projection: vec4f,
    // Row Y of inverse view: reconstruct the preserved road height.
    portal_world_y: vec4f,
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> post: PostUniforms;
@group(0) @binding(3) var car_depth: texture_depth_2d;
@group(0) @binding(4) var level_depth: texture_depth_2d;

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

const BLUR_TAPS: i32 = 12;

fn hash(p: vec2f) -> f32 {
    let q = fract(p * vec2f(443.897, 441.423));
    let r = q + dot(q, q.yx + 19.19);
    return fract((r.x + r.y) * r.x);
}

fn luminance(c: vec3f) -> f32 {
    return dot(c, vec3f(0.2126, 0.7152, 0.0722));
}

// Explicit sRGB working-space conversions: the texture sampler and output target
// already handle transfer functions, so the grade must not apply gamma twice.
fn linearToDisplay(c: vec3f) -> vec3f {
    let positive = max(c, vec3f(0.0));
    return select(positive * 12.92, 1.055 * pow(positive, vec3f(1.0 / 2.4)) - 0.055,
                  positive > vec3f(0.0031308));
}
fn displayToLinear(c: vec3f) -> vec3f {
    let positive = max(c, vec3f(0.0));
    return select(positive / 12.92, pow((positive + 0.055) / 1.055, vec3f(2.4)),
                  positive > vec3f(0.04045));
}

fn visibleCar(px: vec2i) -> bool {
    let size = vec2i(textureDimensions(car_depth));
    let p = clamp(px, vec2i(0), size - vec2i(1));
    let car = textureLoad(car_depth, p, 0);
    return car < 1.0 && car < textureLoad(level_depth, p, 0);
}

fn displaySample(uv: vec2f) -> vec3f {
    return linearToDisplay(textureSampleLevel(scene, scene_sampler, uv, 0.0).rgb);
}

// Interior-only unsharp mask. Erode the depth mask to avoid halos at silhouettes and occluders.
fn clarifyCar(uv: vec2f, colour: vec3f, motion: f32) -> vec3f {
    if (post.car.x <= 0.0 && post.car.y <= 0.0) { return colour; }
    let px = vec2i(uv * vec2f(textureDimensions(car_depth)));
    if (!visibleCar(px)) { return colour; }
    let radius = 3;
    if (!visibleCar(px + vec2i(radius, 0)) || !visibleCar(px - vec2i(radius, 0)) ||
        !visibleCar(px + vec2i(0, radius)) || !visibleCar(px - vec2i(0, radius))) { return colour; }
    let texel = post.frame.yz;
    let n = displaySample(uv - vec2f(0.0, texel.y));
    let s = displaySample(uv + vec2f(0.0, texel.y));
    let e = displaySample(uv + vec2f(texel.x, 0.0));
    let w = displaySample(uv - vec2f(texel.x, 0.0));
    let average = (n + s + e + w) * 0.25;
    let lo = min(colour, min(min(n, s), min(e, w)));
    let hi = max(colour, max(max(n, s), max(e, w)));
    let range = max(max(hi.x - lo.x, hi.y - lo.y), hi.z - lo.z);
    let detail = clamp((colour - average) * post.car.x / (1.0 + 3.0 * range), vec3f(-0.04), vec3f(0.04));
    let wide = (displaySample(uv + vec2f(3.0 * texel.x, 0.0)) + displaySample(uv - vec2f(3.0 * texel.x, 0.0)) +
                displaySample(uv + vec2f(0.0, 3.0 * texel.y)) + displaySample(uv - vec2f(0.0, 3.0 * texel.y))) * 0.25;
    let local = clamp((colour - wide) * post.car.y, vec3f(-0.025), vec3f(0.025));
    // Do not sharpen against intentional speed blur.
    return clamp(colour + (detail + local) * (1.0 - clamp(motion * 4.0, 0.0, 1.0)), vec3f(0.0), vec3f(1.0));
}

// Compress chroma toward the same luminance until every channel is inside the SDR gamut.
// This avoids clipped orange/teal channels changing hue at strong grade settings.
fn fitGamut(c: vec3f) -> vec3f {
    let y = clamp(luminance(c), 0.0, 1.0);
    let chroma = c - vec3f(y);
    var scale = 1.0;
    for (var i = 0; i < 3; i++) {
        if (chroma[i] > 0.00001) { scale = min(scale, (1.0 - y) / chroma[i]); }
        if (chroma[i] < -0.00001) { scale = min(scale, -y / chroma[i]); }
    }
    return vec3f(y) + chroma * clamp(scale, 0.0, 1.0);
}

fn filmGrade(c: vec3f, radius: f32) -> vec3f {
    let original_y = luminance(c);
    let protected_white = 1.0 - smoothstep(0.80, 1.0, original_y);
    // Exposure and white balance in linear light; contrast and split tones in display space.
    let balance = vec3f(0.20 * post.film.y + 0.10 * post.film.z,
                       -0.10 * post.film.z, -0.20 * post.film.y + 0.10 * post.film.z);
    let exposed = linearToDisplay(displayToLinear(c) * exp2(vec3f(post.film.x) + balance * protected_white));
    let y = max(luminance(exposed), 0.0);
    let x = clamp(y, 0.0, 1.0);
    // Luminance-only sigmoid: stronger midtone separation without three different hue curves.
    let power = 1.0 + clamp(post.grade.x, 0.0, 1.0) * 0.75;
    let lo = pow(x, power);
    let hi = pow(1.0 - x, power);
    let sigmoid = lo / max(lo + hi, 0.00001);
    // A near-linear toe preserves shaded asphalt and tire marks instead of crushing
    // them into black. The stronger sigmoid takes over through the midtones.
    let contrast = mix(x * (1.0 - 0.22 * clamp(post.grade.x, 0.0, 1.0)), sigmoid,
                       smoothstep(0.04, 0.35, x));
    // Compress display-referred highlights smoothly. This cannot recover detail already
    // clipped into the scene's RGBA8 target; it prevents the grade from clipping more.
    let shoulder = 0.65 + 0.35 * (1.0 - exp(-max(contrast - 0.65, 0.0) / 0.35));
    var tone = mix(contrast, min(contrast, shoulder), post.frame.w);
    tone += post.film.w * (1.0 - smoothstep(0.0, 0.35, tone));
    var graded = exposed * (tone / max(y, 0.00001));
    if (y < 0.00001) { graded = vec3f(post.film.w); }
    // Balanced split tones: no colour added to true blacks or protected white highlights.
    let shadows = smoothstep(0.015, 0.10, tone) * (1.0 - smoothstep(0.20, 0.58, tone));
    let lights = smoothstep(0.40, 0.82, tone) * protected_white;
    let cool = post.shadow_tint.rgb - vec3f(luminance(post.shadow_tint.rgb));
    let warm = post.highlight_tint.rgb - vec3f(luminance(post.highlight_tint.rgb));
    graded += cool * shadows + warm * lights;
    let l = luminance(graded);
    let saturation = clamp(post.grade.y, 0.0, 2.0) * (1.0 - 0.30 * smoothstep(0.65, 1.0, tone));
    graded = vec3f(l) + (graded - vec3f(l)) * saturation;
    // Rounded lens falloff, independent of viewport aspect ratio, inside the existing pass.
    let falloff = 1.0 - clamp(post.grade.z, 0.0, 0.65) * smoothstep(0.55, 1.45, radius);
    return fitGamut(graded) * falloff;
}

// Samples the scene with colour fringing: red pushed out, blue pulled in, along the line from the centre.
fn fringed(uv: vec2f, offset: vec2f) -> vec3f {
    if (post.strength.y == 0.0) { return textureSampleLevel(scene, scene_sampler, uv, 0.0).rgb; }
    let r = textureSampleLevel(scene, scene_sampler, uv + offset, 0.0).r;
    let g = textureSampleLevel(scene, scene_sampler, uv, 0.0).g;
    let b = textureSampleLevel(scene, scene_sampler, uv - offset, 0.0).b;
    return vec3f(r, g, b);
}

// A recessed dark reveal, champagne inner lip and cool outer hairline. The rim is
// attached to foreground material at the aperture: no ring on open sky, road or car.
fn framePortal(uv: vec2f, colour: vec3f) -> vec3f {
    if (post.car.z <= 0.0 || post.car.w <= 0.0 || post.portal_target.w <= 0.0) { return colour; }
    let ray = (uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0)) * post.portal_projection.xy;
    let delta = ray - post.portal_target.xy;
    let normal = delta / max(length(delta), 0.0001);
    let radius = (post.portal_target.z + post.portal_cut.y / post.portal_target.w) * post.portal_cut.x;
    let focal_px = 0.5 / (post.frame.z * post.portal_projection.y);
    let distance = (length(delta) - radius) * focal_px;
    let scale = 1.0 / (post.frame.z * 1080.0);
    let width = post.car.w * scale;
    let aa = max(0.8, scale);
    let band = smoothstep(-aa, aa, distance + width * 0.12)
             * (1.0 - smoothstep(width - aa, width + aa, distance));
    if (band <= 0.0) { return colour; }
    let size = vec2f(textureDimensions(level_depth));
    let px = vec2i(uv * size);
    if (visibleCar(px)) { return colour; }
    // Read just outside the lip, where foreground coverage is retained. This
    // bridges subpixel holes in splats while keeping open street unframed.
    let probe_ray = post.portal_target.xy + normal * (radius + (width + 3.0 * scale) / focal_px);
    let probe_uv = probe_ray / post.portal_projection.xy * vec2f(0.5, -0.5) + vec2f(0.5);
    let probe = clamp(vec2i(probe_uv * size), vec2i(0), vec2i(size) - vec2i(1));
    // A short tangential gather closes tiny holes in Gaussian depth coverage.
    let tangent = vec2i(round(vec2f(-normal.y, -normal.x) * 3.0 * scale));
    let limit = vec2i(size) - vec2i(1);
    let depth = min(textureLoad(level_depth, probe, 0),
        min(textureLoad(level_depth, clamp(probe + tangent, vec2i(0), limit), 0),
            textureLoad(level_depth, clamp(probe - tangent, vec2i(0), limit), 0)));
    if (depth >= 1.0) { return colour; }
    let z = post.portal_projection.w / (depth + post.portal_projection.z);
    if (z >= post.portal_target.w - post.portal_cut.z) { return colour; }
    let world_y = dot(post.portal_world_y, vec4f(ray * z, -z, 1.0));
    let floor_mask = smoothstep(post.portal_cut.w, post.portal_cut.w + 0.12, world_y);
    let light = clamp(dot(normal, normalize(vec2f(-0.6, 0.8))) * 0.5 + 0.5, 0.0, 1.0);
    let bevel = distance / max(width, 1.0);
    var metal = mix(vec3f(0.035, 0.049, 0.058), vec3f(0.16, 0.20, 0.22), light);
    metal *= 0.65 + 0.35 * smoothstep(0.2, 0.8, bevel);
    let lip = 1.0 - smoothstep(0.6 * scale, 1.6 * scale, abs(distance - width * 0.19));
    metal = mix(metal, mix(vec3f(0.30, 0.25, 0.18), vec3f(0.85, 0.77, 0.57), light), lip * 0.85);
    let seam = 1.0 - smoothstep(0.4 * scale, 1.2 * scale, abs(distance - width * 0.82));
    metal = mix(metal, vec3f(0.35, 0.47, 0.52), seam * (0.25 + 0.30 * light));
    // Recessed fastener slots at eight evenly spaced points, within the bevel.
    let angle = atan2(normal.y, normal.x);
    let slot = smoothstep(0.997, 0.9995, cos(angle * 8.0))
             * smoothstep(0.35, 0.48, bevel) * (1.0 - smoothstep(0.62, 0.74, bevel));
    metal = mix(metal, vec3f(0.025, 0.031, 0.035), slot * 0.85);
    return mix(colour, metal, band * floor_mask * post.car.z * 0.88);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4f {
    let uv = in.uv;
    let from_centre = uv - vec2f(0.5);
    // 0 at the centre, 1 at the edge midpoints, about 1.4 in the corners.
    let radius = length(from_centre * 2.0);
    let fringe = from_centre * radius * post.strength.y;

    // Peripheral blur: a short streak towards the centre, only outside the middle of the screen.
    let px = vec2i(uv * vec2f(textureDimensions(car_depth)));
    // The speed cue belongs to the environment. Protect the visible car, including
    // a small silhouette margin, even when the chase camera puts it near an edge.
    var protect_car = false;
    if (post.strength.x > 0.0) {
        protect_car = visibleCar(px) || visibleCar(px + vec2i(3, 0)) || visibleCar(px - vec2i(3, 0)) ||
                      visibleCar(px + vec2i(0, 3)) || visibleCar(px - vec2i(0, 3));
    }
    var environment = 1.0;
    if (post.car_region.z > 0.0 && post.car_region.w > 0.0) {
        let d = abs((uv - post.car_region.xy) / post.car_region.zw);
        let squared = d * d;
        let rounded_box = sqrt(sqrt(dot(squared, squared)));
        environment = smoothstep(0.90, 1.12, rounded_box);
    }
    let edge = select(smoothstep(post.grade.w, 1.1, radius) * post.strength.x * environment, 0.0, protect_car);
    var colour = vec3f(0.0);
    if (edge > 0.001) {
        let step = -from_centre * edge * post.shadow_tint.w / f32(BLUR_TAPS);
        var total = 0.0;
        for (var i = 0; i < BLUR_TAPS; i = i + 1) {
            let w = 1.0 - f32(i) / f32(BLUR_TAPS);
            let sample_uv = clamp(uv + step * f32(i), vec2f(0.0), vec2f(1.0));
            // Do not drag bright car paint out into the road's radial streaks.
            if (!visibleCar(vec2i(sample_uv * vec2f(textureDimensions(car_depth))))) {
                colour += fringed(sample_uv, fringe) * w;
                total += w;
            }
        }
        colour /= max(total, 0.00001);
    } else {
        colour = fringed(uv, fringe);
    }

    // Grade and grain in display space (the scene texture is sRGB, so samples arrive linear).
    colour = linearToDisplay(colour);
    colour = clarifyCar(uv, colour, edge);

    // The grade, blended in by its strength.
    let grade = post.strength.w;
    if (grade > 0.0) {
        colour = mix(colour, filmGrade(colour, radius), clamp(grade, 0.0, 1.0));
    }

    colour = framePortal(uv, colour);

    // Film grain, stronger in the mid-tones, new every frame.
    if (post.strength.z > 0.0) {
        let pixel = floor(uv / vec2f(post.frame.y, post.frame.z));
        let n = hash(pixel + fract(post.frame.x * 0.61) * 1000.0) - 0.5;
        let l = luminance(colour);
        colour += vec3f(n * post.strength.z * (0.4 + 4.0 * l * (1.0 - l)));
    }
    return vec4f(displayToLinear(clamp(colour, vec3f(0.0), vec3f(1.0))), 1.0);
}
