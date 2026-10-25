// Splat Racer's replacement for the engine's `gsplatModifyVS` hook: level-of-detail crossfades and the car's shadow
// on the level.
//
// The car is an oriented box. The top three rows of `shadow_box` map splat (model) space into the box's own space,
// where the box is [-1, 1] on every axis. Its fourth row, unused by an affine map, carries the direction towards the
// sun in that space (xyz) and the shadow's strength (w; 0 turns it off, as for the car's own splats). One uniform
// rather than three, because WebGPU allows only 12 uniform buffers per stage.
//
// A splat whose ray towards the sun passes through the box is in the car's shadow and is darkened. Four slightly
// different box sizes are tested, so the shadow's edge is soft.

uniform shadow_box: mat4x4f;

// Per splat slot, the share of its opacity drawn: 1 normally, between 0 and 1 while a streamed level's leaf
// crossfades between two levels of detail (written by the depth-key pass each sort).
var<storage, read> splatFade: array<f32>;
// Per splat slot, the level of detail it holds plus one in the low 4 bits (0: not from a streamed level) and its leaf
// above them (for tools), and a debug switch: when
// `splatDebug[0]` is 1, splats are drawn in their level's colour (0 green, 1 yellow, 2 orange, 3 red), so the
// rendered picture shows where coarse detail is seen; at 2, in their distance from the camera.
var<storage, read> splatLod: array<u32>;
var<storage, read> splatDebug: array<u32>;

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
}

fn modifySplatRotationScale(originalCenter: vec3f, modifiedCenter: vec3f, rotation: ptr<function, vec4f>, scale: ptr<function, vec3f>) {
}

// Whether the ray from `origin` along `dir` meets the box [-s, s]³ ahead of it (slab test).
fn rayHitsBox(origin: vec3f, dir: vec3f, s: f32) -> bool {
    let inv = 1.0 / dir;
    let t0 = (vec3f(-s) - origin) * inv;
    let t1 = (vec3f(s) - origin) * inv;
    let near = max(max(min(t0.x, t1.x), min(t0.y, t1.y)), min(t0.z, t1.z));
    let far = min(min(max(t0.x, t1.x), max(t0.y, t1.y)), max(t0.z, t1.z));
    return far >= max(near, 0.0);
}

fn modifySplatColor(center: vec3f, color: ptr<function, vec4f>) {
    *color = vec4f((*color).rgb, (*color).a * splatFade[splat.index]);
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
    let m = uniform.shadow_box;
    let strength = m[3][3];
    if (strength <= 0.0) {
        return;
    }
    let p = (m * vec4f(center, 1.0)).xyz;
    // Far from the car (in box units) nothing is shadowed; skip the tests.
    if (dot(p, p) > SHADOW_REACH_SQ) {
        return;
    }
    let sun = vec3f(m[0][3], m[1][3], m[2][3]);
    var hits = 0.0;
    if (rayHitsBox(p, sun, 0.80)) { hits += 1.0; }
    if (rayHitsBox(p, sun, 0.92)) { hits += 1.0; }
    if (rayHitsBox(p, sun, 1.04)) { hits += 1.0; }
    if (rayHitsBox(p, sun, 1.16)) { hits += 1.0; }
    let shade = 1.0 - strength * hits * 0.25;
    *color = vec4f((*color).rgb * shade, (*color).a);
}
