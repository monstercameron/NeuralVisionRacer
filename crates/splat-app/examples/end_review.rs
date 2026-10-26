//! Render the corrected Delft terminal shoulder using the actual physics collision overlay.
#![allow(dead_code)]
#[path = "../src/renderer.rs"]
mod renderer;
use splat_engine::{
    road::RoadProfile, CarLoad, Command, Engine, EngineConfig, Level, LevelFloor, Logger,
};
use splat_render::{lod::LodMeta, sog};
use std::{path::Path, sync::Arc};
fn main() {
    let out = std::env::args().nth(1).expect("output directory");
    let out = Path::new(&out);
    std::fs::create_dir_all(out).unwrap();
    let mut cfg = EngineConfig::parse(&std::fs::read_to_string("engine.conf").unwrap()).unwrap();
    cfg.level_path = "levels/trompetstraat-delft".into();
    cfg.level_stream = false;
    cfg.hud_enabled = false;
    cfg.effects_grain = 0.;
    let car = sog::load(Path::new(&cfg.splat_path)).unwrap();
    let level = LevelFloor::load(Path::new(&cfg.level_path), cfg.level_floor_clearance_m).unwrap();
    let meta =
        LodMeta::parse(&std::fs::read(Path::new(&cfg.level_path).join("lod-meta.json")).unwrap())
            .unwrap();
    let lod = meta.pick(-1, 3_000_000);
    let chunks: Vec<_> = meta
        .chunk_dirs(lod)
        .iter()
        .map(|p| sog::load_dir(&Path::new(&cfg.level_path).join(p)).unwrap())
        .collect();
    let road = RoadProfile::for_level(&cfg.level_path).unwrap();
    let p = road.centre(74.).unwrap();
    let mut e = Engine::new(
        cfg,
        None,
        Arc::new(Logger::open(None, Level::Error, 30).unwrap()),
        CarLoad::Loaded(sog::centres(&car)),
        Some(level),
    );
    e.physics.set_spawn_yaw(0.);
    e.physics
        .reset_vehicle([p[0] - 1.8, p[1] + 0.68, p[2]])
        .unwrap();
    for _ in 0..120 {
        e.tick();
    }
    e.execute(Command::Pause);
    e.set_render_alpha(1.);
    let p = e.physics.vehicle_pose().unwrap().position;
    e.execute(Command::CameraSet([
        p[0] - 3.,
        p[1] + 5.,
        p[2] - 5.,
        p[0],
        p[1],
        p[2] + 1.,
    ]));
    let mut r = renderer::Renderer::new(None, Some(&car), &chunks, &e, (1280, 720)).unwrap();
    e.config.render_debug_colliders = true;
    r.screenshot(&e, &out.join("end-collision-grid.png"))
        .unwrap();
    e.config.render_debug_colliders = false;
    r.screenshot(&e, &out.join("end-clean.png")).unwrap();
}
