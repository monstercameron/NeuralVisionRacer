// Copies the offscreen colour target to the window surface. Texel-exact, so no filtering is needed.

@group(0) @binding(0) var source: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4f,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOutput;
    out.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    return textureLoad(source, vec2i(in.position.xy), 0);
}
