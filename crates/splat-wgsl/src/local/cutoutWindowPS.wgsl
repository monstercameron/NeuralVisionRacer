// Level-only pixel aperture. Both colour and depth use the same edge, independent
// of individual Gaussian centres and their distance from the camera.
struct CutoutWindowData {
    focus: vec4f,
    cut: vec4f,
    projection: vec4f,
    world_y: vec4f,
}
uniform cutout_window: CutoutWindowData;

fn cutoutWindowAlpha() -> f32 {
    let focus = uniform.cutout_window.focus;
    if (focus.w <= 0.0) { return 1.0; }
    let cut = uniform.cutout_window.cut;
    let z = 1.0 / pcPosition.w;
    if (z >= focus.w - cut.z) { return 1.0; }
    let projection = uniform.cutout_window.projection;
    let uv = pcPosition.xy * projection.zw;
    let ray = (uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0)) * projection.xy;
    let world_y = dot(uniform.cutout_window.world_y, vec4f(ray * z, -z, 1.0));
    let above_floor = smoothstep(cut.w, cut.w + 0.12, world_y);
    let radius = (focus.z + cut.y / focus.w) * cut.x;
    let feather = 3.0 * projection.y * projection.w;
    let edge = smoothstep(radius - feather, radius, length(ray - focus.xy));
    return mix(1.0, edge, above_floor);
}
