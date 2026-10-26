// Contact-anchored rubber deposition. Project onto opaque Gaussian core depth, never the car or sky.
struct ViewData { inverse_vp: mat4x4f, params: vec4f }
struct Segment { start: vec4f, end: vec4f, normal: vec4f, tire: vec4f, phase: vec4f, rect: vec4f }
@group(0) @binding(0) var<uniform> view: ViewData;
@group(0) @binding(1) var<storage, read> segments: array<Segment>;
@group(0) @binding(2) var level_depth: texture_depth_2d;
@group(0) @binding(3) var car_depth: texture_depth_2d;
struct Out { @builtin(position) position: vec4f, @location(0) @interpolate(flat) id: u32 }
@vertex fn vs_main(@builtin(vertex_index) v: u32, @builtin(instance_index) id: u32) -> Out {
    let corners=array<vec2f,4>(vec2f(0,0),vec2f(1,0),vec2f(0,1),vec2f(1,1));
    var output:Out;output.position=vec4f(mix(segments[id].rect.xy,segments[id].rect.zw,corners[v]),0,1);output.id=id;return output;
}
@fragment fn fs_main(input:Out)->@location(0) vec4f {
    let pixel=vec2i(input.position.xy);
    let depth=textureLoad(level_depth,pixel,0);
    let car=textureLoad(car_depth,pixel,0);
    let ndc=vec2f(input.position.x/view.params.z*2.0-1.0,1.0-input.position.y/view.params.w*2.0);
    let h=view.inverse_vp*vec4f(ndc,depth,1);
    let core_world=h.xyz/h.w;
    let s=segments[input.id];
    // Gaussian core depth is a billboard approximation. Use it for visibility/height validation,
    // then intersect the camera ray with the actual contact plane for continuous tread UVs.
    let near_h=view.inverse_vp*vec4f(ndc,0,1);
    let near_world=near_h.xyz/near_h.w;
    let ray=core_world-near_world;
    let denominator=dot(ray,s.normal.xyz);
    let safe_denominator=select(-max(abs(denominator),0.00001),max(abs(denominator),0.00001),denominator>=0.0);
    let world=near_world+ray*(dot(s.start.xyz-near_world,s.normal.xyz)/safe_denominator);
    let raw_delta=s.end.xyz-s.start.xyz;
    let delta=raw_delta-s.normal.xyz*dot(raw_delta,s.normal.xyz);
    let length=max(length(delta),0.00001);
    let tangent=delta/length;
    let across=normalize(cross(s.normal.xyz,tangent));
    let offset=world-s.start.xyz;
    let along=dot(offset,tangent);
    let t=clamp(along/length,0.0,1.0);
    let lateral=dot(offset,across)/(s.tire.w*0.5);
    let height=abs(dot(core_world-s.start.xyz,s.normal.xyz));
    // Phase is captured at deposition. Existing tracks never animate with the car's later RPM.
    let wheel_phase=mix(s.phase.x,s.phase.y,t)*10.0;
    let tire_side=normalize(cross(s.tire.xyz,s.normal.xyz));
    let tread_lateral=dot(world-mix(s.start.xyz,s.end.xyz,t),tire_side)/(s.tire.w*0.5);
    let transverse=wheel_phase+abs(tread_lateral)*0.22;
    let antialias=max(fwidth(transverse),0.025);
    let blocks=smoothstep(0.065-antialias,0.065+antialias,abs(fract(transverse)-0.5));
    let rib_coord=(tread_lateral*0.5+0.5)*4.0;
    let rib_aa=max(fwidth(rib_coord),0.015);
    let ribs=smoothstep(0.08-rib_aa,0.08+rib_aa,abs(fract(rib_coord)-0.5));
    let pattern=mix(blocks*ribs,0.90,s.end.w);
    let side_fade=1.0-smoothstep(0.72,1.0,abs(lateral));
    let height_fade=1.0-smoothstep(s.phase.z*0.80,s.phase.z,height);
    let end_fade=smoothstep(-0.012,0.006,along)*(1.0-smoothstep(length-0.006,length+0.012,along));
    let age=max(view.params.x-s.normal.w,0.0);
    let fade=1.0-smoothstep(70.0,120.0,age);
    let alpha=clamp(s.start.w*view.params.y*pattern*side_fade*height_fade*end_fade*fade,0.0,0.88);
    if depth>=0.9999999 || car<depth-0.000001 || height>s.phase.z || abs(denominator)<0.00001 || abs(lateral)>1.0 || along < -0.012 || along>length+0.012 || alpha<0.001 { discard; }
    return vec4f(0,0,0,alpha);
}
