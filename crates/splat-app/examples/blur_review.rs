//! Freeze the real scene/camera and compare production peripheral blur at representative speeds.
#![allow(dead_code)]
#[path = "../src/renderer.rs"] mod renderer;
use splat_engine::{CarLoad,Command,Engine,EngineConfig,Level,LevelFloor,Logger};
use splat_render::{lod::LodMeta,sog};
use std::{path::Path,sync::Arc};
fn main() {
    let out=std::env::args().nth(1).unwrap();let out=Path::new(&out);std::fs::create_dir_all(out).unwrap();
    let mut cfg=EngineConfig::parse(&std::fs::read_to_string("engine.conf").unwrap()).unwrap();
    cfg.level_path="levels/trompetstraat-delft".into();cfg.level_stream=false;cfg.hud_enabled=false;
    cfg.render_debug_colliders=false;cfg.render_debug_lod_boxes=false;cfg.effects_grain=0.0;cfg.effects_dof=0.0;
    let car=sog::load(Path::new(&cfg.splat_path)).unwrap();
    let level=LevelFloor::load(Path::new(&cfg.level_path),cfg.level_floor_clearance_m).unwrap();
    let dir=Path::new(&cfg.level_path);let meta=LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
    let chunks:Vec<_>=meta.chunk_dirs(meta.pick(-1,3_000_000)).iter().map(|p|sog::load_dir(&dir.join(p)).unwrap()).collect();
    let mut e=Engine::new(cfg,None,Arc::new(Logger::open(None,Level::Error,10).unwrap()),CarLoad::Loaded(sog::centres(&car)),Some(level));
    for _ in 0..120 {e.tick();}e.execute(Command::Pause);e.set_render_alpha(1.0);
    let mut r=renderer::Renderer::new(None,Some(&car),&chunks,&e,(1280,720)).unwrap();r.render(&e);
    let mut post=r.frame.post_settings();let view=splat_engine::render::view_for(&e,1280,720);
    let mut report=Vec::new();
    for speed in [0.0_f32,30.0,60.0,100.0] {
        post.blur=splat_engine::render::peripheral_blur(&e.config,speed);r.frame.set_post(post);
        let pixels=pollster::block_on(r.frame.read_rgba(&view));
        image::RgbaImage::from_raw(1280,720,pixels).unwrap().save(out.join(format!("blur-{speed:.0}.png"))).unwrap();
        report.push(serde_json::json!({"speed_kmh":speed,"shader_strength":post.blur,"streak_length":post.blur_length}));
    }
    std::fs::write(out.join("speeds.json"),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}
