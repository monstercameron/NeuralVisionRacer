//! Renders a streamed level, optionally with the car, through the same `Frame` the game uses, to a PNG.
//!
//! `cargo run --release --example render_level -- <level dir> <out.png> <lod> <eye xyz> <target xyz> [car.sog car xyz scale]`
//!
//! The car is rotated 180° about X (as `splat.rotate_x_degrees`), scaled, and centred on `car xyz`.
//!
//! Benchmark: `FRAMES=n` renders `n` frames after the PNG one and prints the mean milliseconds per frame. `MOVE=1`
//! turns the camera a little every frame (a moving camera re-sorts every scene; a still one need not).

use splat_render::frame::{body_model, Frame};
use splat_render::lod::LodMeta;
use splat_render::{camera, sog};
use std::path::Path;
use std::time::Instant;

fn vec3(args: &[String]) -> [f32; 3] {
    [0, 1, 2].map(|i| args[i].parse().expect("coordinate"))
}

/// 180° about Z is 180° about X then 180° about Y.
fn rotation_y(angle: f32) -> camera::Mat4 {
    let (s, c) = angle.sin_cos();
    [c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 9 {
        eprintln!("usage: render_level <level dir> <out.png> <lod> <eye xyz> <target xyz> [car.sog car_xyz scale]");
        std::process::exit(2);
    }
    let dir = Path::new(&args[0]);
    let lod: i32 = args[2].parse().expect("lod");
    let (eye, target) = (vec3(&args[3..6]), vec3(&args[6..9]));
    let (width, height) = std::env::var("SIZE").ok().and_then(|s| { let (w, h) = s.split_once("x")?; Some((w.parse().ok()?, h.parse().ok()?)) }).unwrap_or((1280, 720));

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let car = args.get(9).map(|path| sog::load(Path::new(path)).expect("load car"));
    let mut frame = pollster::block_on(Frame::new(&instance, None, (width, height), false, car.as_ref())).expect("frame");
    let meta = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).expect("lod-meta.json")).expect("parse");
    let lod = meta.pick(lod, 3_000_000);
    // STREAM=1: LOD streaming around the camera instead of one whole LOD (the `lod` argument is then ignored).
    let mut stream = None;
    if std::env::var("STREAM").is_ok() {
        let started = Instant::now();
        let metas: Vec<Option<Vec<u8>>> = meta.filenames.iter().map(|f| std::fs::read(dir.join(f)).ok()).collect();
        // SH_LODS, CAPACITY and LOD_M override the streaming settings.
        let env = |k: &str| std::env::var(k).ok();
        let defaults = splat_render::stream::StreamSettings::default();
        let settings = splat_render::stream::StreamSettings {
            sh_lods: env("SH_LODS").and_then(|v| v.parse().ok()).unwrap_or(defaults.sh_lods),
            capacity: env("CAPACITY").and_then(|v| v.parse().ok()).unwrap_or(defaults.capacity),
            lod_distance_m: env("LOD_M").and_then(|v| v.parse().ok()).unwrap_or(defaults.lod_distance_m),
            ..defaults
        };
        let mut s = splat_render::stream::LevelStream::new(meta.clone(), &metas, settings).expect("stream");
        frame.start_level_stream(&mut s).expect("pool");
        println!("stream ready in {:?}", started.elapsed());
        stream = Some(s);
    } else {
        let started = Instant::now();
        let mut chunks: Vec<sog::Bundle> = meta.chunk_dirs(lod).iter().map(|c| sog::load_dir(&dir.join(c)).expect("load chunk")).collect();
        // Diagnostic ground slice: hide roofs, trees and reconstructed sky above this world Y, without editing
        // the source files. Use LEVEL_ROT=z with SLICE_Y=<ceiling> for overhead baked-shadow inspection.
        if let Some(ceiling) = std::env::var("SLICE_Y").ok().and_then(|v| v.parse::<f32>().ok()) {
            assert_eq!(std::env::var("LEVEL_ROT").as_deref(), Ok("z"));
            for chunk in &mut chunks {
                let max_radius = std::env::var("SLICE_RADIUS").ok().and_then(|v|v.parse::<f32>().ok()).unwrap_or(f32::INFINITY);
                let floor = std::env::var("SLICE_MIN_Y").ok().and_then(|v|v.parse::<f32>().ok()).unwrap_or(f32::NEG_INFINITY);
                for (i, p) in sog::centres(chunk).iter().enumerate() {
                    let big = (0..3).any(|a|chunk.meta.scales_codebook[chunk.textures["scales"].rgba[i*4+a] as usize].exp() > max_radius);
                    if -p[1] > ceiling || -p[1] < floor || big { chunk.textures.get_mut("sh0").unwrap().rgba[i * 4 + 3] = 0; }
                }
            }
        }
        let loaded = started.elapsed();
        let note = frame.set_level(&chunks).expect("upload level");
        println!("level lod {lod}: {} chunks loaded in {loaded:?}, merged and uploaded in {:?} {note:?}", chunks.len(), started.elapsed() - loaded);
    }
    // LEVEL_ROT=x|z turns the level 180° about that axis.
    match std::env::var("LEVEL_ROT").as_deref() {
        Ok("x") => frame.set_level_model(camera::rotation_x(std::f32::consts::PI)),
        Ok("z") => frame.set_level_model(camera::mul(&camera::rotation_x(std::f32::consts::PI), &rotation_y(std::f32::consts::PI))),
        _ => {}
    }

    let model = match (&car, args.get(10..13), args.get(13)) {
        (Some(car), Some(at), Some(scale)) => {
            let scale: f32 = scale.parse().expect("scale");
            let base = camera::mul(&camera::scale(scale), &camera::rotation_x(std::f32::consts::PI));
            let centres: Vec<[f32; 3]> = sog::centres(car).iter().map(|c| camera::transform_point(&base, *c)).collect();
            body_model(vec3(at), [0.0, 0.0, 0.0, 1.0], sog::mean_centre(&centres), base)
        }
        _ => camera::IDENTITY,
    };
    // CUT=radius: cut a hole through the level around the car (the `car xyz` argument).
    if let (Some(radius), Some(at)) = (std::env::var("CUT").ok().and_then(|r| r.parse::<f32>().ok()), args.get(10..13)) {
        let at = vec3(at);
        frame.set_cutout(Some(splat_render::sort::Cutout { target: at, radius, floor_y: at[1] - 0.35, depth_margin: 0.5, soft: 1.35, splat_reach: 0.25 }));
    }
    let view = splat_render::scene::View {
        model,
        view: camera::look_at(eye, target, [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), width as f32 / height as f32, 0.05, 2000.0),
        width,
        height,
        near: 0.05,
        far: 2000.0,
    };
    // Streaming: run steps until every leaf has its wanted LOD, loading requested chunks synchronously.
    if let Some(s) = stream.as_mut() {
        let started = Instant::now();
        let mut steps = 0;
        loop {
            // A 60 Hz clock, so crossfades and the keep delay play out as they would live.
            frame.stream_level(s, &view, steps as f32 / 60.0);
            let requests = s.requests();
            for (file, chunk) in &requests {
                match sog::load_dir(&dir.join(chunk)) {
                    Ok(bundle) => s.insert(*file, bundle),
                    Err(_) => s.failed(*file),
                }
            }
            let stats = s.stats();
            steps += 1;
            if (requests.is_empty() && stats.leaves_pending == 0 && stats.leaves_empty == 0 && stats.chunks_in_flight == 0) || steps > 400 {
                println!("streamed in {:?} over {steps} steps: {stats:?}", started.elapsed());
                // Let the last crossfades finish before the picture.
                frame.stream_level(s, &view, steps as f32 / 60.0 + 10.0);
                break;
            }
        }
    }
    let pixels = pollster::block_on(frame.read_rgba(&view));
    image::RgbaImage::from_raw(width, height, pixels).expect("size").save(&args[1]).expect("write PNG");
    println!("wrote {}", args[1]);

    if let Some(frames) = std::env::var("FRAMES").ok().and_then(|f| f.parse::<u32>().ok()).filter(|f| *f > 0) {
        let moving = std::env::var("MOVE").is_ok_and(|m| m == "1");
        let frame_view = |i: u32| {
            let mut v = view;
            if moving {
                let a = i as f32 * 0.004;
                v.view = camera::look_at([eye[0] + a.sin() * 0.5, eye[1], eye[2] + a.cos() * 0.1], target, [0.0, 1.0, 0.0]);
            }
            v
        };
        let mut tick = 0u32;
        let mut step = |frame: &mut Frame, v: &splat_render::scene::View| {
            tick += 1;
            if let Some(s) = stream.as_mut() {
                frame.stream_level(s, v, 20.0 + tick as f32 / 60.0);
            }
            frame.render(v);
        };
        for i in 0..5 {
            step(&mut frame, &frame_view(i));
        }
        frame.wait_idle();
        let started = Instant::now();
        for i in 0..frames {
            step(&mut frame, &frame_view(i + 5));
        }
        frame.wait_idle();
        let ms = started.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
        println!("{frames} frames{}: {ms:.2} ms/frame ({:.1} fps)", if moving { " (moving camera)" } else { " (still camera)" }, 1000.0 / ms);
    }
}
