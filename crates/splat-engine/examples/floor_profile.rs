//! Prints the collision floor's height along a straight line, by casting rays down onto the floor boxes.
//! `cargo run --release --example floor_profile -- <level dir> <x> <z from> <z to>`
use rapier3d::prelude::*;
use splat_engine::{EngineConfig, LevelFloor, Physics};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = EngineConfig::default();
    let level = LevelFloor::load(Path::new(&args[0]), config.level_floor_clearance_m).expect("level");
    let x: f32 = args[1].parse().unwrap();
    let (from, to): (f32, f32) = (args[2].parse().unwrap(), args[3].parse().unwrap());
    let mut physics = Physics::new(&config);
    physics.add_voxel_floor(&level.cells, level.dims, level.origin, level.resolution, &config).expect("floor");
    physics.world.step();
    let pipeline = physics.world.query_pipeline();
    let mut z = from;
    let mut line = String::new();
    while z >= to {
        let ray = Ray::new(Vector::new(x, 10.0, z), Vector::new(0.0, -1.0, 0.0));
        let h = pipeline.cast_ray(&ray, 30.0, true).map(|(_, toi)| 10.0 - toi);
        line += &format!("z {z:7.2}: {}\n", h.map_or("hole".into(), |h| format!("{h:6.2} {}", "#".repeat(((h + 3.0) * 20.0).max(0.0) as usize))));
        z -= std::env::var("STEP").ok().and_then(|s| s.parse().ok()).unwrap_or(0.25f32);
    }
    print!("{line}");
}
