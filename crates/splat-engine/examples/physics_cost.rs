//! Measures the level's collision: how many colliders it builds and what a physics tick costs with the car on it.
//! `cargo run --release --example physics_cost -- <car.sog> <level dir>`
use splat_engine::log::Level;
use splat_engine::{CarLoad, EngineConfig, Engine, LevelFloor, Logger};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let car = splat_render::sog::load(Path::new(&args[0])).expect("car");
    let config = EngineConfig::parse(&std::fs::read_to_string("../../engine.conf").expect("engine.conf")).expect("config");
    let started = Instant::now();
    let level = LevelFloor::load(Path::new(&args[1]), config.level_floor_clearance_m).expect("level");
    let loaded = started.elapsed();
    let log = Arc::new(Logger::open(None, Level::Warn, 10).unwrap());
    let started = Instant::now();
    let mut engine = Engine::new(config, None, log, CarLoad::Loaded(splat_render::sog::centres(&car)), Some(level));
    let built = started.elapsed();
    println!("voxels to floor and walls: {loaded:?}; engine and colliders: {built:?}; colliders: {}", engine.physics.world.colliders.len());
    for _ in 0..60 {
        engine.tick();
    }
    let started = Instant::now();
    for _ in 0..600 {
        engine.tick();
    }
    println!("tick: {:.3} ms", started.elapsed().as_secs_f64() * 1000.0 / 600.0);
}
