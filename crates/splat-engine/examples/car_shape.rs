//! Prints how the car splat is distributed: per 5 cm height slice, the splat count and the x/z extent (5–95%).
//! `cargo run --release --example car_shape -- <car.sog>` (after the engine's rotation and length scaling)
use splat_engine::engine::car_scale;
use splat_render::camera;
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("car.sog");
    let bundle = splat_render::sog::load(Path::new(&path)).expect("car");
    let rotate = camera::rotation_x(std::f32::consts::PI);
    let rotated: Vec<[f32; 3]> = splat_render::sog::centres(&bundle).iter().map(|c| camera::transform_point(&rotate, *c)).collect();
    let square = splat_engine::car_fit::align(&rotated);
    let rotated: Vec<[f32; 3]> = rotated.iter().map(|c| camera::transform_point(&square, *c)).collect();
    let scale = car_scale(&rotated, 4.57);
    let pts: Vec<[f32; 3]> = rotated.iter().map(|c| c.map(|v| v * scale)).collect();
    let mut ys: Vec<f32> = pts.iter().map(|p| p[1]).collect();
    ys.sort_by(f32::total_cmp);
    let lo = ys[0];
    println!("scale {scale:.3}; y from {lo:.2} to {:.2}; percentiles 0.2/1/5%: {:.3} {:.3} {:.3}", ys[ys.len() - 1], ys[ys.len() / 500], ys[ys.len() / 100], ys[ys.len() / 20]);
    let base = ys[ys.len() / 500];
    for slice in 0..32 {
        let (a, b) = (base + slice as f32 * 0.05 - 0.1, base + slice as f32 * 0.05 - 0.05);
        let mut xs: Vec<f32> = pts.iter().filter(|p| p[1] >= a && p[1] < b).map(|p| p[0]).collect();
        let mut zs: Vec<f32> = pts.iter().filter(|p| p[1] >= a && p[1] < b).map(|p| p[2]).collect();
        if xs.len() < 10 {
            println!("{a:6.2}..{b:6.2}: {}", xs.len());
            continue;
        }
        xs.sort_by(f32::total_cmp);
        zs.sort_by(f32::total_cmp);
        let q = |v: &[f32], f: f32| v[((v.len() - 1) as f32 * f) as usize];
        println!("{a:6.2}..{b:6.2}: {:6} splats  x {:6.2}..{:5.2}  z {:6.2}..{:5.2}", xs.len(), q(&xs, 0.05), q(&xs, 0.95), q(&zs, 0.05), q(&zs, 0.95));
    }
}
