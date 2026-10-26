//! Matched full-engine film-grade captures: grade_review <level> <output-dir> <label>.
#![allow(dead_code)]
#[path = "../src/renderer.rs"]
mod renderer;
use splat_engine::{CarLoad, Command, Engine, EngineConfig, Level, LevelFloor, Logger};
use splat_render::{lod::LodMeta, sog};
use std::{path::Path,sync::Arc};
fn main() {
    let args:Vec<_>=std::env::args().skip(1).collect();
    let name=&args[0]; let out=Path::new(&args[1]); let label=&args[2];std::fs::create_dir_all(out).unwrap();
    let mut cfg=EngineConfig::parse(&std::fs::read_to_string("engine.conf").unwrap()).unwrap();
    cfg.level_path=format!("levels/{name}"); cfg.level_stream=false;
    cfg.hud_enabled=false; cfg.effects_grain=0.0; cfg.window_vsync=false;
    if label=="off" {cfg.effects_grade=0.0;}
    let bundle=sog::load(Path::new(&cfg.splat_path)).unwrap();
    let floor=LevelFloor::load(Path::new(&cfg.level_path),cfg.level_floor_clearance_m).unwrap();
    let meta=LodMeta::parse(&std::fs::read(Path::new(&cfg.level_path).join("lod-meta.json")).unwrap()).unwrap();
    let lod=meta.pick(-1,3_000_000);
    let chunks:Vec<_>=meta.chunk_dirs(lod).iter().map(|c|sog::load_dir(&Path::new(&cfg.level_path).join(c)).unwrap()).collect();
    let log=Arc::new(Logger::open(None,Level::Error,50).unwrap());
    let mut engine=Engine::new(cfg,None,log,CarLoad::Loaded(sog::centres(&bundle)),Some(floor));
    for _ in 0..120 {engine.tick();} engine.set_render_alpha(1.0); engine.execute(Command::Pause);
    let p=engine.physics.vehicle_pose().unwrap().position;
    let body=engine.physics.vehicle().unwrap().body;
    let forward=*engine.physics.world.bodies[body].rotation()*engine.physics.wheel_geometry().unwrap().forward;
    let right=[-forward.z,0.0,forward.x];
    engine.execute(Command::CameraSet([p[0]-forward.x*5.8+right[0]*2.0,p[1]+1.6,
        p[2]-forward.z*5.8+right[2]*2.0,p[0]+forward.x*0.7,p[1]+0.2,p[2]+forward.z*0.7]));
    let mut renderer=renderer::Renderer::new(None,Some(&bundle),&chunks,&engine,(1280,720)).unwrap();
    renderer.screenshot(&engine,&out.join(format!("{name}-{label}.png"))).unwrap();
    println!("{name} {label}: lod {lod}");
}
