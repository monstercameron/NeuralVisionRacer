//! Builds the level's voxel floor in rapier and reports the cost. `cargo run --release --example probe_floor_physics -- <level dir>`
use splat_engine::svo::Octree;
use splat_engine::{EngineConfig, Physics};
use std::path::Path;
use std::time::Instant;

fn main() {
    let dir = std::env::args().nth(1).expect("level directory");
    let tree = Octree::load(Path::new(&dir)).expect("load voxel collision");
    let config = EngineConfig::default();
    let clearance = (config.level_floor_clearance_m / tree.meta.resolution).ceil() as u32;
    let started = Instant::now();
    let cells = tree.floor_cells(clearance);
    println!("{} floor cells in {:?}", cells.len(), started.elapsed());
    let mut physics = Physics::new(&config);
    let started = Instant::now();
    physics.add_voxel_floor(&cells, splat_engine::svo::grid_cells(&tree.meta), tree.meta.grid_min, tree.meta.resolution, &config).expect("floor");
    println!("rapier floor built in {:?}", started.elapsed());
    let started = Instant::now();
    for _ in 0..60 {
        physics.step();
    }
    println!("60 steps with the floor in {:?}", started.elapsed());
}
