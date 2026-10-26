//! Reproducible full-engine benchmark (release). No FPS cap or readback inside timed frames.
//! perf_probe <level-name> <output-dir> [frames=90] [stream=0|1] [width=1280] [height=720]
//! Writes raw frame samples plus before/after screenshots; warm-up and loading are excluded.
#![allow(dead_code)]
#[path = "../src/renderer.rs"]
mod renderer;
use splat_engine::{CarLoad, Command, Engine, EngineConfig, Level, LevelFloor, Logger};
use splat_render::{lod::LodMeta, sog};
use std::{path::Path, sync::Arc, time::Instant};
fn main() {
    let a: Vec<_> = std::env::args().skip(1).collect();
    let name = a.first().expect("level name");
    let out = Path::new(a.get(1).expect("output dir"));
    std::fs::create_dir_all(out).unwrap();
    let frames: usize = a.get(2).map_or(90, |s| s.parse().unwrap());
    let streaming = a.get(3).is_some_and(|s| s == "1");
    let width = a.get(4).map_or(1280, |s| s.parse().unwrap());
    let height = a.get(5).map_or(720, |s| s.parse().unwrap());
    let mut cfg = EngineConfig::parse(&std::fs::read_to_string("engine.conf").unwrap()).unwrap();
    cfg.level_path = format!("levels/{name}");
    cfg.level_stream = streaming;
    cfg.log_file.clear();
    cfg.render_debug_lod_boxes = false;
    cfg.render_debug_colliders = false;
    cfg.effects_grain = 0.0;
    cfg.window_width = width;
    cfg.window_height = height;
    cfg.window_vsync = false;
    let bundle = sog::load(Path::new(&cfg.splat_path)).unwrap();
    let floor = LevelFloor::load(Path::new(&cfg.level_path), cfg.level_floor_clearance_m).ok();
    let chunks = if streaming {
        Vec::new()
    } else {
        let dir = Path::new(&cfg.level_path);
        let m = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
        m.chunk_dirs(1)
            .iter()
            .map(|c| sog::load_dir(&dir.join(c)).unwrap())
            .collect::<Vec<_>>()
    };
    let log = Arc::new(Logger::open(None, Level::Error, 50).unwrap());
    let mut engine = Engine::new(
        cfg,
        None,
        log,
        CarLoad::Loaded(sog::centres(&bundle)),
        floor,
    );
    for _ in 0..180 {
        engine.tick();
    }
    engine.set_render_alpha(1.0);
    engine.execute(Command::Pause);
    let p = engine.physics.vehicle_pose().unwrap().position;
    let camera = |engine: &mut Engine, angle: f32| {
        let (s, c) = angle.sin_cos();
        engine.execute(Command::CameraSet([
            p[0] + 4.6 * c,
            p[1] + std::env::var("CAMERA_HEIGHT").ok().and_then(|s|s.parse::<f32>().ok()).unwrap_or(1.5),
            p[2] + 4.6 * s,
            p[0],
            p[1],
            p[2],
        ]));
    };
    camera(&mut engine, -0.60);
    let setup = Instant::now();
    let mut renderer =
        renderer::Renderer::new(None, Some(&bundle), &chunks, &engine, (width, height)).unwrap();
    if let Ok(n)=std::env::var("GPU_SAMPLE_EVERY"){renderer.frame.profile_sample_every(n.parse().expect("GPU_SAMPLE_EVERY integer"));}
    renderer
        .screenshot(&engine, &out.join("reference.png"))
        .unwrap();
    renderer.frame.wait_idle();
    let base = engine.config.clone();
    let mut results = Vec::new();
    let mut csv = String::from("variant,mode,frame,cpu_ms,frame_ms\n");
    let variants = std::env::var("VARIANTS")
        .unwrap_or_else(|_| "base,no_hud,no_dof,no_specular,no_clarity,no_shadow".into());
    for variant in variants.split(',') {
        engine.config = base.clone();
        match variant {
            "no_hud" => engine.config.hud_enabled = false,
            "no_dof" => engine.config.effects_dof = 0.0,
            "no_specular" => engine.config.effects_car_specular = 0.0,
            "no_clarity" => {
                engine.config.effects_car_sharpen = 0.0;
                engine.config.effects_car_clarity = 0.0;
            }
            "no_shadow" => engine.config.effects_shadow = 0.0,
            "base" => {}
            other => panic!("unknown variant {other}"),
        }
        for mode in ["still", "orbit"] {
            camera(&mut engine, -0.60);
            for _ in 0..15 {
                renderer.render(&engine);
            }
            renderer.frame.wait_idle();
            let mut wall = Vec::new();
            let mut cpu = Vec::new();
            for i in 0..frames {
                if mode == "orbit" {
                    camera(&mut engine, -0.60 + i as f32 * 0.002);
                }
                let start = Instant::now();
                renderer.render(&engine);
                let encoded = start.elapsed().as_secs_f64() * 1000.0;
                renderer.frame.wait_idle();
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                cpu.push(encoded);
                wall.push(ms);
                csv.push_str(&format!("{variant},{mode},{i},{encoded:.6},{ms:.6}\n"));
            }
            let mean = wall.iter().sum::<f64>() / frames as f64;
            wall.sort_by(f64::total_cmp);
            cpu.sort_by(f64::total_cmp);
            let result = serde_json::json!({"variant":variant,"mode":mode,"mean_frame_ms":mean,"median_frame_ms":wall[frames/2],"p95_frame_ms":wall[(frames*95/100).min(frames-1)],"median_cpu_ms":cpu[frames/2],"fps":1000.0/mean,"gpu_ms":renderer.frame.gpu_ms(),"timing_source":renderer.frame.timing_source(),"pending_frames":renderer.frame.pending_frames()});
            println!("{result}");
            std::fs::write(out.join(format!("profile-{variant}-{mode}.json")),serde_json::to_vec_pretty(&renderer.profile_json(&engine)).unwrap()).unwrap();
            renderer.screenshot(&engine,&out.join(format!("image-{variant}-{mode}.png"))).unwrap();
            results.push(result);
        }
    }
    engine.config = base;
    camera(&mut engine, -0.60);
    renderer
        .screenshot(&engine, &out.join("verified.png"))
        .unwrap();
    // Isolate steady-state physics from renderer/streaming. No change to tick rate or physics settings.
    engine.execute(Command::Resume);
    let start = Instant::now();
    for _ in 0..600 {
        engine.tick();
    }
    let tick_ms = start.elapsed().as_secs_f64() * 1000.0 / 600.0;
    std::fs::write(out.join("frames.csv"), csv).unwrap();
    std::fs::write(out.join("results.json"),serde_json::to_string_pretty(&serde_json::json!({"level":name,"size":[width,height],"streaming":streaming,"frames_per_case":frames,"setup_plus_benchmark_s":setup.elapsed().as_secs_f64(),"steady_tick_ms":tick_ms,"stream_stats":format!("{:?}",renderer.stream_stats()),"results":results})).unwrap()).unwrap();
}
