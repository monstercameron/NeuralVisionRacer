//! Approximate surface normals for car relighting. A Gaussian's thinnest axis is a normal estimate,
//! not a measured material normal. Average unoriented normal tensors in a small spatial neighbourhood;
//! tensors keep opposite quaternion signs from cancelling. Round/large/faint splats receive no gloss.
use crate::{sog, wheel};
use std::collections::HashMap;

type Cell = [i32; 3];
type Tensor = [f32; 6];
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn normalize(n: [f32; 3]) -> [f32; 3] {
    let l = dot(n, n).sqrt().max(1e-12);
    n.map(|v| v / l)
}
fn outer(n: [f32; 3], w: f32) -> Tensor {
    [
        n[0] * n[0] * w,
        n[1] * n[1] * w,
        n[2] * n[2] * w,
        n[0] * n[1] * w,
        n[0] * n[2] * w,
        n[1] * n[2] * w,
    ]
}
fn principal(t: Tensor, mut n: [f32; 3]) -> [f32; 3] {
    for _ in 0..5 {
        n = normalize([
            t[0] * n[0] + t[3] * n[1] + t[4] * n[2],
            t[3] * n[0] + t[1] * n[1] + t[5] * n[2],
            t[4] * n[0] + t[5] * n[1] + t[2] * n[2],
        ]);
    }
    n
}

fn gaussian(q: &[u8], scales: [f32; 3], radius: f32, opacity: u8) -> ([f32; 3], f32) {
    let mut sorted = [0usize, 1, 2];
    sorted.sort_by(|a, b| scales[*a].total_cmp(&scales[*b]));
    let confidence = (1.0 - scales[sorted[0]] / scales[sorted[1]].max(1e-12)).clamp(0.0, 1.0);
    if confidence < 0.25 || scales[sorted[2]] > radius * 3.0 || opacity < 40 {
        return ([0.0, 1.0, 0.0], 0.0);
    }
    // Match PlayCanvas SOG v2: the omitted (largest, positive) component is q.a - 252.
    let abc = [0, 1, 2].map(|i| (q[i] as f32 / 255.0 - 0.5) * std::f32::consts::SQRT_2);
    let d = (1.0 - dot(abc, abc)).max(0.0).sqrt();
    let mode = q[3].saturating_sub(252) as usize;
    let mut quat = [0.0; 4];
    let mut j = 0;
    for (i, v) in quat.iter_mut().enumerate() {
        if i == mode {
            *v = d;
        } else {
            *v = abc[j];
            j += 1;
        }
    }
    let mut axis = [0.0; 3];
    axis[sorted[0]] = 1.0;
    (normalize(wheel::rotate(quat, axis)), confidence)
}

/// Packed model-space normal (RGB) and reliability (A), one u32 per car splat. Only car scenes allocate this.
pub fn normals(bundle: &sog::Bundle) -> Vec<u32> {
    let points = sog::centres(bundle);
    if points.is_empty() {
        return vec![0];
    }
    // Robust extent excludes reconstruction floaters and gives roughly a 3 cm neighbourhood on a 5 m car.
    let mut ext = [0.0f32; 3];
    for axis in 0..3 {
        let mut v: Vec<f32> = points
            .iter()
            .map(|p| p[axis])
            .filter(|x| x.is_finite())
            .collect();
        v.sort_by(f32::total_cmp);
        if !v.is_empty() {
            ext[axis] = v[v.len() * 995 / 1000] - v[v.len() * 5 / 1000];
        }
    }
    let radius = ext.into_iter().fold(0.0, f32::max).max(0.001) / 160.0;
    let key = |p: [f32; 3]| p.map(|x| (x / radius).floor() as i32);
    let scales = &bundle.textures["scales"].rgba;
    let quats = &bundle.textures["quats"].rgba;
    let alpha = &bundle.textures["sh0"].rgba;
    let raw: Vec<_> = (0..points.len())
        .map(|i| {
            gaussian(
                &quats[i * 4..i * 4 + 4],
                [0, 1, 2].map(|a| bundle.meta.scales_codebook[scales[i * 4 + a] as usize].exp()),
                radius,
                alpha[i * 4 + 3],
            )
        })
        .collect();
    let mut grid: HashMap<Cell, Tensor> = HashMap::new();
    for (p, (n, w)) in points.iter().zip(&raw) {
        if *w == 0.0 || !p.iter().all(|x| x.is_finite()) {
            continue;
        }
        let t = outer(*n, *w);
        let slot = grid.entry(key(*p)).or_default();
        for i in 0..6 {
            slot[i] += t[i];
        }
    }
    points
        .iter()
        .zip(raw)
        .map(|(p, (n, w))| {
            if w == 0.0 {
                return 0;
            }
            let k = key(*p);
            let mut tensor = [0.0; 6];
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(t) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                            // Keep panel creases: neighbour normals must be roughly parallel to this Gaussian.
                            let similarity = (t[0] * n[0] * n[0]
                                + t[1] * n[1] * n[1]
                                + t[2] * n[2] * n[2]
                                + 2.0
                                    * (t[3] * n[0] * n[1]
                                        + t[4] * n[0] * n[2]
                                        + t[5] * n[1] * n[2]))
                                / (t[0] + t[1] + t[2]).max(1e-12);
                            if similarity > 0.5 {
                                let weight = similarity / (1 + dx * dx + dy * dy + dz * dz) as f32;
                                for i in 0..6 {
                                    tensor[i] += t[i] * weight;
                                }
                            }
                        }
                    }
                }
            }
            let smooth = principal(tensor, n);
            let byte = |v: f32| ((v * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u32;
            byte(smooth[0])
                | byte(smooth[1]) << 8
                | byte(smooth[2]) << 16
                | ((w * 255.0).round() as u32) << 24
        })
        .collect()
}

/// A directional clearcoat contribution. The baked RGB remains the base appearance.
#[derive(Debug, Clone, Copy)]
pub struct CarLight {
    /// Direction from the car towards the sun, fixed in world space.
    pub sun: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub roughness: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opposite_normal_signs_do_not_cancel() {
        let a = outer([0.0, 1.0, 0.0], 1.0);
        let b = outer([0.0, -1.0, 0.0], 1.0);
        let sum = std::array::from_fn(|i| a[i] + b[i]);
        assert!(dot(principal(sum, [0.01, 1.0, 0.01]), [0.0, 1.0, 0.0]) > 0.999);
    }
    #[test]
    fn only_surface_like_splats_receive_gloss() {
        let q = [128, 128, 128, 255];
        let (n, w) = gaussian(&q, [0.01, 0.1, 0.1], 1.0, 255);
        assert!(n[0].abs() > 0.99 && w > 0.8);
        assert_eq!(gaussian(&q, [0.1; 3], 1.0, 255).1, 0.0);
        assert_eq!(gaussian(&q, [0.01, 5.0, 5.0], 1.0, 255).1, 0.0);
        assert_eq!(gaussian(&q, [0.01, 0.1, 0.1], 1.0, 20).1, 0.0);
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;
    use crate::{camera, scene};
    use std::collections::BTreeMap;

    #[test]
    fn highlight_moves_with_world_sun_and_preserves_coverage() {
        let Some((device, queue)) = crate::try_device() else {
            return;
        };
        let side = 41;
        let count = side * side;
        let mut low = Vec::new();
        let mut high = Vec::new();
        let log = |v: f32| v.signum() * (1.0 + v.abs()).ln();
        let mins = [log(-0.1), log(-1.01), log(-1.01)];
        let maxs = mins.map(|v| -v);
        for y in 0..side {
            for z in 0..side {
                for (a, v) in [0.0, (y as f32 - 20.0) / 20.0, (z as f32 - 20.0) / 20.0]
                    .into_iter()
                    .enumerate()
                {
                    let code = ((log(v) - mins[a]) / (maxs[a] - mins[a]) * 65535.0).round() as u16;
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
        textures.insert("scales", tex([0, 1, 1, 0].repeat(count as usize)));
        textures.insert("sh0", tex([0, 0, 0, 255].repeat(count as usize)));
        let mut scales = vec![0.02f32.ln(); 256];
        scales[0] = 0.0005f32.ln();
        let bundle = sog::complete(
            sog::Meta {
                count,
                means_mins: mins,
                means_maxs: maxs,
                scales_codebook: scales,
                sh0_codebook: vec![-0.5; 256],
                sh_n_codebook: vec![0.0; 256],
                sh_n_count: 0,
            },
            textures,
        );
        let pipelines = scene::Pipelines::new(&device, crate::COLOR_FORMAT).unwrap();
        let car = scene::build_car(&device, &queue, &pipelines, &bundle, 0.5).unwrap();
        let view = scene::View {
            model: camera::IDENTITY,
            view: camera::look_at([3.0, 0.0, 0.0], [0.0; 3], [0.0, 1.0, 0.0]),
            projection: camera::perspective(50f32.to_radians(), 1.0, 0.01, 100.0),
            width: 128,
            height: 128,
            near: 0.01,
            far: 100.0,
        };
        let light = CarLight {
            sun: [1.0, 0.0, 0.0],
            color: [1.0; 3],
            intensity: 1.0,
            roughness: 0.32,
        };
        car.set_car_light(&queue, None, &view);
        let base = car.render_rgba(&device, &queue, &pipelines, &view, None);
        car.set_car_light(&queue, Some(light), &view);
        let lit = car.render_rgba(&device, &queue, &pipelines, &view, None);
        assert!(base
            .chunks_exact(4)
            .zip(lit.chunks_exact(4))
            .any(|(a, b)| b[0] > a[0] + 10));
        for (a, b) in base.chunks_exact(4).zip(lit.chunks_exact(4)) {
            assert_eq!(a[3], b[3], "relighting must not change opacity");
            if a[..3] == [0, 0, 0] {
                assert_eq!(a, b, "background must stay unchanged");
            }
        }
        // The same fitted occluders suppress only added clearcoat, preserving captured base RGB/coverage.
        let proxy=crate::shadow::Proxy{bounds:([-1.0;3],[1.0;3]),ellipsoids:vec![([0.6,0.0,0.0],[0.2,0.8,0.8])]};
        car.set_uniform(&queue,"shadow_box",&proxy.uniform(&camera::IDENTITY,light.sun,0.55,0.12,true));
        let occluded=car.render_rgba(&device,&queue,&pipelines,&view,None);
        let gain=|image:&[u8]|base.iter().zip(image).map(|(a,b)|u64::from(b.saturating_sub(*a))).sum::<u64>();
        assert!(gain(&occluded)<gain(&lit)*8/10,"proxy must suppress the added highlight");
        car.set_car_light(&queue,None,&view);
        assert_eq!(base,car.render_rgba(&device,&queue,&pipelines,&view,None),"self-shadow must preserve baked base colours");
        car.set_uniform(&queue,"shadow_box",&vec![0;crate::shadow::UNIFORM_BYTES as usize]);

        // Turn car and camera together: baked appearance holds, while the same world sun becomes grazing.
        let rotation = camera::quat_to_mat4(wheel::axis_angle(
            [0.0, 1.0, 0.0],
            std::f32::consts::FRAC_PI_2,
        ));
        let turned = scene::View {
            model: rotation,
            view: camera::look_at(
                camera::transform_point(&rotation, [3.0, 0.0, 0.0]),
                [0.0; 3],
                [0.0, 1.0, 0.0],
            ),
            ..view
        };
        car.set_car_light(&queue, None, &turned);
        let turned_base = car.render_rgba(&device, &queue, &pipelines, &turned, None);
        car.set_car_light(&queue, Some(light), &turned);
        let grazing = car.render_rgba(&device, &queue, &pipelines, &turned, None);
        let delta = |a: &[u8], b: &[u8]| {
            a.iter()
                .zip(b)
                .map(|(x, y)| u64::from(x.abs_diff(*y)))
                .sum::<u64>()
        };
        assert!(
            delta(&turned_base, &grazing) < delta(&base, &lit) / 10,
            "sun must remain anchored to world rather than rotating with car"
        );
    }
}
