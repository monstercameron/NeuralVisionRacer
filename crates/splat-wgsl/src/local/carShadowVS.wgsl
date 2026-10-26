// Surface-fitted car shadow and clearcoat share the same per-level world sun.
// One packed uniform keeps the existing portable binding budget. Negative receiver.w marks car-only
// clearcoat occlusion; positive receiver.w marks level receivers. Its magnitude is penumbra width.
struct CarShadowData {
    to_proxy: mat4x4f,
    sun: vec4f,
    receiver: vec4f,
    shapes: array<vec4f, 14>,
}
uniform shadow_box: CarShadowData;
var<private> shadowTransmission: f32;

// Per splat slot, the share of its opacity drawn: 1 normally, between 0 and 1 while a streamed level's leaf
// crossfades between two levels of detail (written by the depth-key pass each sort).
var<storage, read> splatFade: array<f32>;
// Per splat slot, the level of detail it holds plus one in the low 4 bits (0: not from a streamed level) and its leaf
// above them (for tools), and a debug switch: when
// `splatDebug[0]` is 1, splats are drawn in their level's colour (0 green, 1 yellow, 2 orange, 3 red), so the
// rendered picture shows where coarse detail is seen; at 2, in their distance from the camera.
var<storage, read> splatLod: array<u32>;
var<storage, read> splatDebug: array<u32>;

#include "wheelMathVS"
// Car scenes use the high splatLod bits for part IDs instead of leaf IDs; their low LOD bits stay zero.
// Level scenes keep their existing layout and always have wheelRig disabled.
// Header (enabled, selection preview, 0, 0), then quaternion/translation pairs for parts 0..8.
// Wheel poses rotate centres and in-plane Gaussian orientation together; axle thickness is unchanged.
var<storage, read> wheelRig: array<vec4f>;
// Optional car-only relighting. Level scenes bind a single disabled normal and zero intensity.
// Light: sun.xyz/intensity, eye.xyz/roughness, linear color.xyz/unused, spare.
var<storage, read> carSurface: array<u32>;
var<storage, read> carLight: array<vec4f>;
var<private> carRadiance: vec3f;

fn wheelPart() -> u32 {
    if (wheelRig[0].x < 0.5) { return 0u; }
    return min(splatLod[splat.index] >> 4u, 8u);
}

fn wheelShDirection(dir: vec3f) -> vec3f {
    let part = wheelPart();
    if (part == 0u) { return dir; }
    let q = wheelRig[part * 2u + 1u];
    return wheelRotate(vec4f(-q.xyz, q.w), dir);
}

fn carSunRadiance(center: vec3f) -> vec3f {
    if (carLight[0].w <= 0.0 || dot(carLight[0].xyz, carLight[0].xyz) < 1e-8) { return vec3f(0.0); }
    let packed = carSurface[splat.index];
    let reliability = f32(packed >> 24u) / 255.0;
    if (reliability <= 0.0) { return vec3f(0.0); }
    var n = normalize(vec3f(f32(packed & 255u), f32((packed >> 8u) & 255u), f32((packed >> 16u) & 255u)) / 127.5 - vec3f(1.0));
    let part = wheelPart();
    if (part > 0u) { n = wheelRotate(wheelRig[part * 2u + 1u], n); }
    let v = normalize(carLight[1].xyz - center);
    let l = normalize(carLight[0].xyz);
    // Gaussian normals are unoriented. Choose the camera-facing hemisphere, then require sun incidence.
    n = select(n, -n, dot(n, v) < 0.0);
    let nl = max(dot(n, l), 0.0);
    if (nl <= 0.0) { return vec3f(0.0); }
    let halfSum = l + v;
    if (dot(halfSum, halfSum) < 1e-6) { return vec3f(0.0); }
    let h = normalize(halfSum);
    let nv = max(dot(n, v), 0.01);
    let nh = max(dot(n, h), 0.0);
    let vh = max(dot(v, h), 0.0);
    let roughness = carLight[1].w;
    let a2 = pow(roughness, 4.0);
    let denominator = nh * nh * (a2 - 1.0) + 1.0;
    let distribution = a2 / max(3.14159265 * denominator * denominator, 1e-5);
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let visibility = nv / (nv * (1.0 - k) + k) * nl / (nl * (1.0 - k) + k);
    let fresnel = 0.04 + 0.96 * pow(1.0 - vh, 5.0);
    // Bounded additive clearcoat, evaluated in linear light. Preserve baked colour and Gaussian coverage.
    let specular = min(distribution * visibility * fresnel / max(4.0 * nv, 0.01), 0.8);
    // Offset by the same reliable normal used by clearcoat. Only the added highlight is occluded;
    // the captured car RGB already contains its own lighting and remains untouched.
    var radiance = carLight[2].xyz * carLight[0].w * reliability * specular;
    // Occlusion matters for visible glints. Preserve tiny contributions directly instead of testing seven
    // casters for almost-black GGX tails on every panel/wheel Gaussian.
    if (uniform.shadow_box.sun.w > 0.0 && max(radiance.x, max(radiance.y, radiance.z)) > 0.002) {
        let proxyNormal = (uniform.shadow_box.to_proxy * vec4f(n, 0.0)).xyz;
        let proxyPoint = (uniform.shadow_box.to_proxy * vec4f(center, 1.0)).xyz;
        let bias = proxyNormal / max(length(proxyNormal), 1e-5) * 0.06;
        radiance *= carShadowVisibility(proxyPoint + bias);
    }
    return radiance;
}

fn lodColour(lod: u32) -> vec3f {
    switch lod {
        case 0u: { return vec3f(0.1, 1.0, 0.2); }
        case 1u: { return vec3f(1.0, 0.95, 0.1); }
        case 2u: { return vec3f(1.0, 0.5, 0.05); }
        default: { return vec3f(1.0, 0.1, 0.1); }
    }
}

// Splats farther than this (squared, in box half-sizes) from the car are not tested.
const SHADOW_REACH_SQ: f32 = 36.0;

fn modifySplatCenter(center: ptr<function, vec3f>) {
    // Centres already include this rigid part pose: the compute deformer writes the shared sort/render buffer.
}

fn modifySplatRotationScale(originalCenter: vec3f, modifiedCenter: vec3f, rotation: ptr<function, vec4f>, scale: ptr<function, vec3f>) {
    let part = wheelPart();
    if (part > 0u) { *rotation = wheelMultiply(wheelRig[part * 2u + 1u], *rotation); }
}

// Analytic soft ray/ellipsoid coverage. Work is bounded to seven proxies close to the car.
fn carShadowVisibility(p: vec3f) -> f32 {
    let sun = uniform.shadow_box.sun;
    if (sun.w <= 0.0 || dot(p, p) > SHADOW_REACH_SQ) { return 1.0; }
    var coverage = 0.0;
    for (var i = 0u; i < 7u; i++) {
        let shape = uniform.shadow_box.shapes[i * 2u];
        if (shape.w < 0.5) { continue; }
        let radius = max(uniform.shadow_box.shapes[i * 2u + 1u].xyz, vec3f(1e-4));
        let q = (p - shape.xyz) / radius;
        // A coarse volume enclosing the surface is not a separate self-occluder. This complements the
        // normal bias and prevents a fitted ellipsoid from extinguishing its own panel highlight.
        if (uniform.shadow_box.receiver.w < 0.0 && dot(q, q) < 1.10) { continue; }
        let d = sun.xyz / radius;
        let t = max(-dot(q, d) / max(dot(d, d), 1e-8), 0.0);
        let closest = q + d * t;
        let width = abs(uniform.shadow_box.receiver.w) * (1.0 + min(t * length(sun.xyz), 6.0) * 0.15);
        coverage = max(coverage, 1.0 - smoothstep(1.0 - width, 1.0 + width, length(closest)));
        if (coverage >= 0.999) { break; }
    }
    return 1.0 - coverage;
}

fn shadowReceiverWeight() -> f32 {
    // Reload only for actual shadow receivers. Keeping quaternion/scale live across every vertex
    // would increase register pressure even on the millions of splats outside the shadow.
    let s = getScale();
    let smallest = min(s.x, min(s.y, s.z));
    let middle = s.x + s.y + s.z - smallest - max(s.x, max(s.y, s.z));
    let confidence = clamp(1.0 - smallest / max(middle, 1e-6), 0.0, 1.0);
    var axis = vec3f(1.0, 0.0, 0.0);
    if (s.y < s.x && s.y <= s.z) { axis = vec3f(0.0, 1.0, 0.0); }
    if (s.z < s.x && s.z < s.y) { axis = vec3f(0.0, 0.0, 1.0); }
    let n = wheelRotate(getRotation().yzwx, axis);
    let l = normalize(uniform.shadow_box.receiver.xyz);
    // Unoriented Gaussian normals: incidence uses the absolute dot product. Ambiguous/round splats and
    // surfaces at grazing sun incidence receive a weaker approximation instead of opaque floating patches.
    let incidence = smoothstep(0.06, 0.40, abs(dot(n, l)));
    return mix(0.65, mix(0.35, 1.0, incidence), confidence);
}

fn modifySplatColor(center: vec3f, color: ptr<function, vec4f>) {
    shadowTransmission = 1.0;
    carRadiance = vec3f(0.0);
    *color = vec4f((*color).rgb, (*color).a * splatFade[splat.index]);
    #ifdef PREPASS_PASS
        // Depth coverage depends on opacity only; RGB lighting, debug tints and box shading have no effect.
        return;
    #endif
    if (wheelRig[0].x > 0.5 && wheelRig[0].y > 0.5) {
        let part = wheelPart();
        var tint = vec3f(0.18);
        if (part > 0u) {
            switch ((part - 1u) % 4u) {
                case 0u: { tint = vec3f(1.0, 0.12, 0.1); }
                case 1u: { tint = vec3f(0.12, 1.0, 0.15); }
                case 2u: { tint = vec3f(0.1, 0.4, 1.0); }
                default: { tint = vec3f(1.0, 0.85, 0.1); }
            }
            if (part > 4u) { tint = vec3f(1.0, 0.1, 1.0); }
        }
        *color = vec4f(tint, (*color).a);
        return;
    }
    let packed = splatLod[splat.index];
    let lod = packed & 15u;
    if (splatDebug[0] == 1u && lod > 0u) {
        *color = vec4f(lodColour(lod - 1u), (*color).a);
        return;
    }
    // Debug mode 2: the splat's distance from the camera in red, 0..128 m, for measuring what is seen how far away.
    if (splatDebug[0] == 2u && lod > 0u) {
        let d = length((uniform.matrix_view * uniform.matrix_model * vec4f(center, 1.0)).xyz);
        *color = vec4f(clamp(d / 128.0, 0.0, 1.0), 0.0, 0.0, (*color).a);
        return;
    }
    #ifndef PREPASS_PASS
        carRadiance = carSunRadiance(center);
    #endif
    if (uniform.shadow_box.sun.w <= 0.0 || uniform.shadow_box.receiver.w <= 0.0) { return; }
    let p = (uniform.shadow_box.to_proxy * vec4f(center, 1.0)).xyz;
    let blocked = 1.0 - carShadowVisibility(p);
    if (blocked <= 0.0) { return; }
    // Apply attenuation after gamma decode in gsplatOutputVS, alongside the linear clearcoat contribution.
    shadowTransmission = 1.0 - uniform.shadow_box.sun.w * blocked * shadowReceiverWeight();
}
