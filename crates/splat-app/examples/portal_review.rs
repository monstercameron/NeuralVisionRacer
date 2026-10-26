//! Matched foreground-portal captures using the production renderer: portal_review <output-dir>.
#![allow(dead_code)]
#[path = "../src/renderer.rs"]
mod renderer;
use splat_engine::{CarLoad, Command, Engine, EngineConfig, Level, LevelFloor, Logger};
use splat_render::{lod::LodMeta, sog};
use std::{path::Path, sync::Arc};
fn main() {
    let out = std::env::args().nth(1).unwrap();
    let out = Path::new(&out);
    std::fs::create_dir_all(out).unwrap();
    let mut cfg = EngineConfig::parse(&std::fs::read_to_string("engine.conf").unwrap()).unwrap();
    cfg.level_path = "levels/trompetstraat-delft".into();
    cfg.level_stream = false;
    cfg.hud_enabled = false;
    cfg.render_debug_colliders = false;
    cfg.render_debug_lod_boxes = false;
    cfg.effects_grain = 0.0;
    cfg.effects_dof = 0.0;
    let car = sog::load(Path::new(&cfg.splat_path)).unwrap();
    let level = LevelFloor::load(Path::new(&cfg.level_path), cfg.level_floor_clearance_m).unwrap();
    let dir = Path::new(&cfg.level_path);
    let meta = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
    let chunks: Vec<_> = meta
        .chunk_dirs(meta.pick(-1, 3_000_000))
        .iter()
        .map(|p| sog::load_dir(&dir.join(p)).unwrap())
        .collect();
    let mut e = Engine::new(
        cfg,
        None,
        Arc::new(Logger::open(None, Level::Error, 10).unwrap()),
        CarLoad::Loaded(sog::centres(&car)),
        Some(level),
    );
    for _ in 0..120 {
        e.tick();
    }
    e.execute(Command::Pause);
    e.set_render_alpha(1.0);
    let p = e.physics.vehicle_pose().unwrap().position;
    let mut r = renderer::Renderer::new(None, Some(&car), &chunks, &e, (1280, 720)).unwrap();
    for (label, offset) in [
        ("side", [-7.0, 2.4, 1.8]),
        ("opposite", [7.0, 2.4, -1.8]),
        ("chase", [0.0, 2.6, -6.0]),
    ] {
        e.execute(Command::CameraSet([
            p[0] + offset[0],
            p[1] + offset[1],
            p[2] + offset[2],
            p[0],
            p[1],
            p[2],
        ]));
        for (variant, strength) in [("off", 0.0), ("on", 1.0)] {
            e.config.effects_portal_frame = strength;
            r.screenshot(&e, &out.join(format!("{label}-{variant}.png")))
                .unwrap();
            r.frame.profile_sample_every(1);
            r.frame.reset_profile();
            for _ in 0..24 {
                r.render(&e);
                r.frame.wait_idle();
            }
            std::fs::write(
                out.join(format!("{label}-{variant}-profile.json")),
                serde_json::to_vec_pretty(&r.frame.profile_json()).unwrap(),
            )
            .unwrap();
        }
    }
}
