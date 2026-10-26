// Soft ground-material dust; all motion uses simulation time, so pause freezes it.
struct View {vp:mat4x4f,right:vec4f,up:vec4f,params:vec4f,viewport:vec4f}
struct Particle {origin_birth:vec4f,velocity_life:vec4f,tint_alpha:vec4f,shape:vec4f,normal:vec4f}
@group(0) @binding(0) var<uniform> view:View;
@group(0) @binding(1) var<storage,read> particles:array<Particle>;
@group(0) @binding(2) var level_depth:texture_depth_2d;
@group(0) @binding(3) var car_depth:texture_depth_2d;
struct Out {@builtin(position) position:vec4f,@location(0) uv:vec2f,@location(1) @interpolate(flat) id:u32}
@vertex fn vs_main(@builtin(vertex_index) v:u32,@builtin(instance_index) id:u32)->Out {
    let corners=array<vec2f,6>(vec2f(-1,-1),vec2f(1,-1),vec2f(-1,1),vec2f(-1,1),vec2f(1,-1),vec2f(1,1));
    let p=particles[id];let age=max(view.params.x-p.origin_birth.w,0.0);let t=clamp(age/p.velocity_life.w,0.0,1.0);
    let position=p.origin_birth.xyz+p.velocity_life.xyz*((1.0-exp(-0.65*age))/0.65)+p.normal.xyz*0.03*age*age;
    let radius=p.shape.x*(1.0+2.4*t);let uv=corners[v];
    var out:Out;out.position=view.vp*vec4f(position+(view.right.xyz*uv.x+view.up.xyz*uv.y)*radius,1);
    out.uv=uv;out.id=id;return out;
}
fn hash(p:vec2f)->f32 {return fract(sin(dot(p,vec2f(127.1,311.7)))*43758.5453);}
fn noise(p:vec2f)->f32 {
    let i=floor(p);let f=fract(p);let u=f*f*(3.0-2.0*f);
    return mix(mix(hash(i),hash(i+vec2f(1,0)),u.x),mix(hash(i+vec2f(0,1)),hash(i+vec2f(1,1)),u.x),u.y);
}
@fragment fn fs_main(input:Out)->@location(0) vec4f {
    let p=particles[input.id];let age=view.params.x-p.origin_birth.w;let t=age/p.velocity_life.w;
    let r2=dot(input.uv,input.uv);if r2>1.0||t<0.0||t>=1.0 {discard;}
    let drift=input.uv*3.1+vec2f(p.shape.y*31.0,age*0.30);
    let cloud=noise(drift)*0.65+noise(drift*2.13+4.7)*0.35;
    let density=exp(-r2*2.7)*(1.0-smoothstep(0.60,1.0,r2))*smoothstep(0.12,0.85,cloud);
    let fade=smoothstep(0.0,0.09,age)*pow(max(1.0-t,0.0),1.25);
    let contribution=p.tint_alpha.w*view.params.y*density*fade;
    if contribution<0.001 {discard;}
    let dimensions=vec2i(textureDimensions(level_depth));let px=clamp(vec2i(input.position.xy),vec2i(0),dimensions-vec2i(1));
    // Opaque Gaussian cores have sub-pixel gaps although their colour tails overlap.
    // A small resolution-scaled conservative car-depth dilation stops dust behind the body leaking
    // through those gaps. It stays soft at the silhouette instead of tinting body panels.
    var car=textureLoad(car_depth,px,0);
    let core_radius=max(1,i32(round(min(view.viewport.x,view.viewport.y)/240.0)));
    for (var y=-1;y<=1;y++) {for (var x=-1;x<=1;x++) {
        let sample_px=clamp(px+vec2i(x,y)*core_radius,vec2i(0),dimensions-vec2i(1));
        car=min(car,textureLoad(car_depth,sample_px,0));
    }}
    let depth=min(textureLoad(level_depth,px,0),car);
    let opaque_z=view.params.w/(depth+view.params.z);
    let particle_z=view.params.w/(input.position.z+view.params.z);
    let soft=clamp((opaque_z-particle_z)/0.30,0.0,1.0);
    let alpha=clamp(contribution*soft,0.0,0.40);
    if alpha<0.001 {discard;}
    return vec4f(p.tint_alpha.rgb*(0.85+cloud*0.15)*alpha,alpha);
}
