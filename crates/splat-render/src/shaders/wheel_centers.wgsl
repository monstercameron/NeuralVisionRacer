// Only selected car-part centres are updated. Original SOG data stays immutable.
@group(0) @binding(0) var<storage, read> original: array<f32>;
@group(0) @binding(1) var<storage, read_write> animated: array<f32>;
@group(0) @binding(2) var<storage, read> parts: array<u32>;
@group(0) @binding(3) var<storage, read> rig: array<vec4f>;
@group(0) @binding(4) var<storage, read> selected: array<u32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= arrayLength(&selected)) { return; }
    let i = selected[id.x];
    let part = parts[i] >> 4u;
    let p = vec3f(original[i * 3u], original[i * 3u + 1u], original[i * 3u + 2u]);
    let q = rig[part * 2u + 1u];
    let t = rig[part * 2u + 2u].xyz;
    let moved = select(p, wheelRotate(q, p) + t, rig[0].x >= 0.5);
    animated[i * 3u] = moved.x;
    animated[i * 3u + 1u] = moved.y;
    animated[i * 3u + 2u] = moved.z;
}
