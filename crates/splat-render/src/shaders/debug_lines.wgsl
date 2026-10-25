// Debug lines over the finished frame (the streamed level's octree leaves, coloured by level of detail). Positions
// are in the level's splat space; `clip` takes them to clip space. No depth test: the boxes show through walls.

struct LineUniforms {
    clip: mat4x4f,
};

@group(0) @binding(0) var<uniform> lines: LineUniforms;

struct VertexIn {
    @location(0) position: vec3f,
    @location(1) colour: vec4f,
};

struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) colour: vec4f,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = lines.clip * vec4f(in.position, 1.0);
    out.colour = in.colour;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4f {
    // Premultiplied, as the frame blends.
    return vec4f(in.colour.rgb * in.colour.a, in.colour.a);
}
