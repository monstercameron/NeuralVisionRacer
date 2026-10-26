use crate::{camera, scene, shadow, sog};
use std::collections::BTreeMap;
// An opaque floor of thin Y-normal Gaussians. No car geometry is drawn: only its light-occlusion proxy.
pub(crate) fn floor() -> sog::Bundle {
    let side = 51u32;
    let count = side * side;
    let mut low = Vec::new();
    let mut high = Vec::new();
    let log = |v: f32| v.signum() * (v.abs() + 1.0).ln();
    let mins = [log(-2.5), -0.1, log(-2.5)];
    let maxs = [log(2.5), 0.1, log(2.5)];
    for x in 0..side {
        for z in 0..side {
            let p = [
                (x as f32 / 50.0 - 0.5) * 5.0,
                0.0,
                (z as f32 / 50.0 - 0.5) * 5.0,
            ];
            for a in 0..3 {
                let code = ((log(p[a]) - mins[a]) / (maxs[a] - mins[a]) * 65535.0).round() as u16;
                low.push(code as u8);
                high.push((code >> 8) as u8);
            }
            low.push(0);
            high.push(0);
        }
    }
    let tex = |rgba| sog::Texture {
        width: count,
        height: 1,
        rgba,
    };
    let mut textures = BTreeMap::new();
    textures.insert("means_l", tex(low));
    textures.insert("means_u", tex(high));
    textures.insert("quats", tex([128, 128, 128, 255].repeat(count as usize)));
    textures.insert("scales", tex([1, 0, 1, 0].repeat(count as usize)));
    textures.insert("sh0", tex([0, 0, 0, 255].repeat(count as usize)));
    let mut scales = vec![0.065f32.ln(); 256];
    scales[0] = 0.001f32.ln();
    sog::complete(
        sog::Meta {
            count,
            means_mins: mins,
            means_maxs: maxs,
            scales_codebook: scales,
            sh0_codebook: vec![0.0; 256],
            sh_n_codebook: vec![0.0; 256],
            sh_n_count: 0,
        },
        textures,
    )
}
#[test]
fn shared_sun_casts_opposite_shadows_without_changing_coverage() {
    let Some((device, queue)) = crate::try_device() else {
        return;
    };
    let pipeline = scene::Pipelines::new(&device, crate::COLOR_FORMAT).unwrap();
    let level = scene::build(&device, &queue, &pipeline, &floor(), 0.5).unwrap();
    let view = scene::View {
        model: camera::IDENTITY,
        view: camera::look_at([0.0, 5.0, 0.01], [0.0; 3], [0.0, 0.0, -1.0]),
        projection: camera::perspective(65f32.to_radians(), 1.0, 0.01, 100.0),
        width: 128,
        height: 128,
        near: 0.01,
        far: 100.0,
    };
    let base = level.render_rgba(&device, &queue, &pipeline, &view, None);
    let proxy = shadow::Proxy {
        bounds: ([-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]),
        ellipsoids: vec![([0.0, 1.0, 0.0], [0.45, 0.5, 0.45])],
    };
    let render = |sun| {
        level.set_uniform(
            &queue,
            "shadow_box",
            &proxy.uniform(&camera::IDENTITY, sun, 0.7, 0.12, false),
        );
        level.render_rgba(&device, &queue, &pipeline, &view, None)
    };
    let left = render([0.8, 1.0, 0.0]);
    let right = render([-0.8, 1.0, 0.0]);
    let centroid = |image: &[u8]| {
        let mut x = 0.0;
        let mut weight = 0.0;
        for (i, (a, b)) in base.chunks_exact(4).zip(image.chunks_exact(4)).enumerate() {
            assert_eq!(a[3], b[3], "shadow must preserve opacity");
            assert!(b[0] <= a[0] + 1, "shadow must not brighten the receiver");
            let w = f32::from(a[0].saturating_sub(b[0]));
            x += (i % 128) as f32 * w;
            weight += w;
        }
        assert!(weight > 1000.0, "shadow must be visible");
        x / weight
    };
    let (a, b) = (centroid(&left), centroid(&right));
    assert!(
        (a - 64.0) * (b - 64.0) < 0.0 && (a - b).abs() > 10.0,
        "opposite sun must move shadow across the caster: {a}, {b}"
    );
    level.set_uniform(
        &queue,
        "shadow_box",
        &vec![0; shadow::UNIFORM_BYTES as usize],
    );
    assert_eq!(
        base,
        level.render_rgba(&device, &queue, &pipeline, &view, None)
    );
}
