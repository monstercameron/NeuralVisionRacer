// Shared rigid-part math for the visible Gaussian, covariance, SH direction and sort centres.
fn wheelRotate(q: vec4f, v: vec3f) -> vec3f {
    return v + 2.0 * cross(q.xyz, cross(q.xyz, v) + q.w * v);
}

fn wheelMultiply(a: vec4f, b: vec4f) -> vec4f {
    return vec4f(a.w * b.xyz + b.w * a.xyz + cross(a.xyz, b.xyz), a.w * b.w - dot(a.xyz, b.xyz));
}
