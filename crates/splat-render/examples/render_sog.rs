//! Renders one frame of a SOG bundle, sorted back-to-front on the CPU, to a PNG.
//!
//! `cargo run --example render_sog -- <in.sog> <out.png> [width height [eye_xyz target_xyz]]`
//!
//! Without an explicit camera, the view mirrors the spike viewer's framing: the splat is rotated 180° about X,
//! then the camera looks at the bounds centre from 1.8× the bounds size along (1, 0.35, 1).

use splat_render::camera;
use splat_render::{scene, sog, try_device};
use std::path::Path;
use std::time::Instant;

fn parse_vec3(args: &[String]) -> [f32; 3] {
    [0, 1, 2].map(|i| args[i].parse().expect("camera coordinate"))
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / len, v[1] / len, v[2] / len]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: render_sog <in.sog> <out.png> [width height [eye_xyz target_xyz]]");
        std::process::exit(2);
    }
    let input = Path::new(&args[0]);
    let output = &args[1];
    let width: u32 = args.get(2).map_or(960, |s| s.parse().expect("width"));
    let height: u32 = args.get(3).map_or(540, |s| s.parse().expect("height"));
    let explicit_camera = (args.len() >= 10).then(|| (parse_vec3(&args[4..7]), parse_vec3(&args[7..10])));

    let started = Instant::now();
    let bundle = sog::load(input).expect("load SOG bundle");
    println!("loaded {} splats in {:?}", bundle.meta.count, started.elapsed());

    let (device, queue) = try_device().expect("no GPU adapter");
    let pipelines = scene::Pipelines::new(&device, splat_render::COLOR_FORMAT).expect("build pipelines");
    let scene = scene::build(&device, &queue, &pipelines, &bundle, 0.5).expect("build gsplat scene");

    let model = camera::rotation_x(std::f32::consts::PI);
    let (eye, target) = match explicit_camera {
        Some(pair) => pair,
        None => {
            let lo = bundle.meta.means_mins.map(sog::unlog);
            let hi = bundle.meta.means_maxs.map(sog::unlog);
            let centre = [0, 1, 2].map(|i| (lo[i] + hi[i]) * 0.5);
            let size = [0, 1, 2].map(|i| hi[i] - lo[i]);
            let diagonal = (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
            let target = camera::transform_point(&model, centre);
            let dir = normalize([1.0, 0.35, 1.0]);
            let eye = [0, 1, 2].map(|i| target[i] + dir[i] * diagonal * 1.8);
            (eye, target)
        }
    };
    println!("camera eye {eye:?} target {target:?}");

    let view = scene::View {
        model,
        view: camera::look_at(eye, target, [0.0, 1.0, 0.0]),
        projection: camera::perspective(50f32.to_radians(), width as f32 / height as f32, 0.01, 1000.0),
        width,
        height,
        near: 0.01,
        far: 1000.0,
    };

    let started = Instant::now();
    let rgba = scene.render_rgba(&device, &queue, &pipelines, &view, None);
    println!("rendered {width}x{height} in {:?}", started.elapsed());

    image::RgbaImage::from_raw(width, height, rgba)
        .expect("image buffer size")
        .save(output)
        .expect("write PNG");
    println!("wrote {output}");
}
