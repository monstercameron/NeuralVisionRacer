//! Estimate a shared sun from baked road brightness vs full-source voxel visibility.
//! Offline diagnostic: fit_sun <level> <report.json>. No source assets are modified.
use splat_engine::{road::RoadProfile, svo::Octree};
use splat_render::{lod::LodMeta, sog};
use std::{collections::BTreeMap, path::Path};
#[derive(Clone, Copy)]
struct Sample {
    p: [f32; 3],
    brightness: f64,
}
fn main() {
    let a: Vec<_> = std::env::args().skip(1).collect();
    let dir = Path::new("levels").join(&a[0]);
    let road = RoadProfile::for_level(&a[0]).unwrap();
    let tree = Octree::load(&dir).unwrap();
    let o = tree.meta.grid_min;
    let step = tree.meta.resolution * 4.;
    let dims = [0, 1, 2].map(|i| ((tree.meta.grid_max[i] - o[i]) / step).ceil() as usize);
    let ix = |c: [usize; 3]| (c[0] * dims[2] + c[2]) * dims[1] + c[1];
    let mut solid = vec![false; dims.iter().product()];
    tree.for_each_occupied(|c| {
        let c = c.map(|v| v as usize / 4);
        solid[ix(c)] = true;
    });
    let meta = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
    let lod = meta.pick(-1, 3_000_000);
    let mut bins: BTreeMap<(i32, i32), Vec<([f32; 3], f64)>> = BTreeMap::new();
    for chunk in meta.chunk_dirs(lod) {
        let b = sog::load_dir(&dir.join(chunk)).unwrap();
        let colours = &b.textures["sh0"].rgba;
        let scales = &b.textures["scales"].rgba;
        for (i, p) in sog::centres(&b).iter().enumerate() {
            let p = [-p[0], -p[1], p[2]];
            let Some(c) = road.centre(p[2]) else {
                continue;
            };
            if (p[0]-c[0]).abs()>road.half_width || (p[1] - c[1]).abs() > 0.18 || p[2] < -62. || p[2] > 72. || colours[i * 4 + 3] < 170 {
                continue;
            }
            if (0..3).any(|axis| b.meta.scales_codebook[scales[i * 4 + axis] as usize].exp() > 0.3)
            {
                continue;
            }
            let rgb = [0, 1, 2].map(|axis| {
                (0.5 + 0.2820948 * b.meta.sh0_codebook[colours[i * 4 + axis] as usize]).max(0.)
            });
            if rgb[1] > rgb[0] * 1.15 && rgb[1] > rgb[2] * 1.15 {
                continue;
            }
            let l = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
            bins.entry(((p[0] * 2.).floor() as i32, (p[2] * 2.).floor() as i32))
                .or_default()
                .push((p, f64::from((l + 0.02).ln())));
        }
    }
    let samples: Vec<_> = bins
        .values_mut()
        .filter(|v| v.len() >= 8)
        .map(|v| {
            v.sort_by(|a, b| a.1.total_cmp(&b.1));
            let m = &v[v.len() / 2];
            Sample {
                p: m.0,
                brightness: m.1,
            }
        })
        .collect();
    let lit = |p: [f32; 3], sun: [f32; 3]| {
        let p = [p[0], p[1] + 0.25, p[2]];
        let mut t = 0.25;
        while t < 100. {
            let q = [0, 1, 2].map(|i| ((p[i] + sun[i] * t - o[i]) / step).floor() as isize);
            if (0..3).any(|i| q[i] < 0 || q[i] >= dims[i] as isize) {
                return true;
            }
            if solid[ix(q.map(|v| v as usize))] {
                return false;
            }
            t += step * 0.70;
        }
        true
    };
    let mean = samples.iter().map(|s| s.brightness).sum::<f64>() / samples.len() as f64;
    let variance = samples
        .iter()
        .map(|s| (s.brightness - mean).powi(2))
        .sum::<f64>();
    let sun = |az: f32, el: f32| {
        let (sa, ca) = az.to_radians().sin_cos();
        let (se, ce) = el.to_radians().sin_cos();
        [sa * ce, se, ca * ce]
    };
    let score = |az: f32, el: f32| {
        let d = sun(az, el);
        let mut count = 0;
        let mut sum = 0.;
        for s in &samples {
            if lit(s.p, d) {
                count += 1;
                sum += s.brightness - mean;
            }
        }
        let fraction = count as f64 / samples.len() as f64;
        if fraction < 0.03 || fraction > 0.97 {
            return (-1., fraction);
        }
        (
            sum / (variance * samples.len() as f64 * fraction * (1. - fraction)).sqrt(),
            fraction,
        )
    };
    let mut candidates = vec![];
    for az in (0..360).step_by(10) {
        for el in (10..86).step_by(5) {
            let (s, f) = score(az as f32, el as f32);
            candidates.push((s, az as f32, el as f32, f));
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (_, az, el, _) = candidates[0];
    for da in -6..=6 {
        for de in -5..=5 {
            let a = az + da as f32 * 2.;
            let e = (el + de as f32 * 2.).clamp(5., 88.);
            let (s, f) = score(a, e);
            candidates.push((s, a, e, f));
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let rows:Vec<_>=candidates.iter().take(12).map(|&(s,a,e,f)|serde_json::json!({"correlation":s,"azimuth_deg":a,"elevation_deg":e,"sun":sun(a,e),"predicted_lit_fraction":f})).collect();
    let previous = score(
        0.75f32.atan2(-0.35).to_degrees(),
        (0.6f32 / (0.75f32.powi(2) + 0.35f32.powi(2)).sqrt())
            .atan()
            .to_degrees(),
    );
    let best = sun(candidates[0].1, candidates[0].2);
    let points: Vec<_> = samples
        .iter()
        .map(|s| serde_json::json!({"p":s.p,"log_brightness":s.brightness,"lit":lit(s.p,best)}))
        .collect();
    let report = serde_json::json!({"samples":samples.len(),"source_lod":lod,"visibility_grid_m":step,"previous_correlation":previous.0,"candidates":rows,"points":points,
 "method":"Offline correlation of median log road luminance against full source voxel visibility; approximate capture-light estimate, affected by reconstruction, material and indirect light"});
    std::fs::write(&a[1], serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!(
        "samples {} previous {:?} best {:?}",
        samples.len(),
        previous,
        rows[0]
    );
}
