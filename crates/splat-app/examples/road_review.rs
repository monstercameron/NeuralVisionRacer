//! Matched full-engine Delft shadow-intensity captures: road_review <level> <output-dir> <label>.
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
    if label=="before" {cfg.effects_shadow *= 0.75 / 1.35;}
    let bundle=sog::load(Path::new(&cfg.splat_path)).unwrap();
    let floor=LevelFloor::load(Path::new(&cfg.level_path),cfg.level_floor_clearance_m).unwrap();
    let meta=LodMeta::parse(&std::fs::read(Path::new(&cfg.level_path).join("lod-meta.json")).unwrap()).unwrap();
    let lod=meta.pick(-1,3_000_000);
    let chunks:Vec<_>=meta.chunk_dirs(lod).iter().map(|c|sog::load_dir(&Path::new(&cfg.level_path).join(c)).unwrap()).collect();
    let log=Arc::new(Logger::open(None,Level::Error,50).unwrap());
    let mut engine=Engine::new(cfg,None,log,CarLoad::Loaded(sog::centres(&bundle)),Some(floor));
    for _ in 0..120 {engine.tick();} engine.set_render_alpha(1.0); engine.execute(Command::Pause);
    if label.starts_with("alignment") {
        let road=splat_engine::road::RoadProfile::for_level(&engine.config.level_path).unwrap();
        let p=road.centre(-11.0).unwrap();engine.physics.set_spawn_yaw(std::f32::consts::PI);
        engine.physics.reset_vehicle([p[0],p[1]+0.68,p[2]]).unwrap();
        engine.execute(Command::Resume);for _ in 0..120{engine.tick();}engine.set_render_alpha(1.0);engine.execute(Command::Pause);
    }
    let p=engine.physics.vehicle_pose().unwrap().position;
    let body=engine.physics.vehicle().unwrap().body;
    let forward=*engine.physics.world.bodies[body].rotation()*engine.physics.wheel_geometry().unwrap().forward;
    let right=[-forward.z,0.0,forward.x];
    engine.execute(Command::CameraSet([p[0]-forward.x*5.8+right[0]*2.0,p[1]+2.6,
        p[2]-forward.z*5.8+right[2]*2.0,p[0]+forward.x*0.7,p[1]+0.2,p[2]+forward.z*0.7]));
    if label.starts_with("alignment") {engine.execute(Command::CameraSet([p[0]+3.0,p[1]+8.0,p[2]-4.5,p[0],p[1]-0.2,p[2]+0.5]));}
    let mut renderer=renderer::Renderer::new(None,Some(&bundle),&chunks,&engine,(1280,720)).unwrap();
    renderer.screenshot(&engine,&out.join(format!("{name}-{label}.png"))).unwrap();
    if label=="alignment-before" {
        // Change only the sun direction after the common production-frame setup.
        let old=[0.73367399,0.5869392,-0.34238118];
        let mut light=splat_engine::lighting::for_level(&engine.config);light.sun=old;
        light.intensity*=engine.config.effects_car_specular;
        light.roughness*=engine.config.effects_car_roughness/0.32;
        renderer.frame.set_car_light(Some(light));renderer.frame.set_shadow(Some((0.7425,old)));
        let view=splat_engine::render::view_for(&engine,1280,720);
        let pixels=pollster::block_on(renderer.frame.read_rgba(&view));
        image::RgbaImage::from_raw(1280,720,pixels).unwrap().save(out.join(format!("{name}-{label}.png"))).unwrap();
    }

    println!("{name} {label}: lod {lod}");
}
