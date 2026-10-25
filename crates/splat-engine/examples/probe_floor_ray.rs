//! Casts rays down through the level floor at the road spawn and prints where rapier says the floor is.
//! `cargo run --release --example probe_floor_ray -- <level dir>`
use rapier3d::prelude::*;
use splat_engine::{EngineConfig, LevelFloor, Physics};
use std::path::Path;

fn main() {
    let dir = std::env::args().nth(1).expect("level directory");
    let config = EngineConfig::default();
    let level = LevelFloor::load(Path::new(&dir), config.level_floor_clearance_m).expect("level");
    let spawn = level.road_spawn().expect("road");
    println!("road spawn top (world): {:?}", spawn);
    footprint(&level, spawn);
    let mut physics = Physics::new(&config);
    physics.add_voxel_floor(&level.cells, level.dims, level.origin, level.resolution, &config).expect("floor");
    physics.world.step();
    let pipeline = physics.world.query_pipeline();
    for dy in [0.0f32, -0.1, -0.5] {
        let origin = Vector::new(spawn[0], spawn[1] + 5.0 + dy, spawn[2]);
        let ray = Ray::new(origin, Vector::new(0.0, -1.0, 0.0));
        match pipeline.cast_ray(&ray, 100.0, true) {
            Some((_, toi)) => println!("ray from y={:.2}: floor hit at y={:.3}", origin.y, origin.y - toi),
            None => println!("ray from y={:.2}: no floor hit", origin.y),
        }
    }
    let cell = level.cells.iter().min_by(|a, b| {
        let d = |c: &[u32; 3]| (c[0] as f32 - (spawn[0] - level.origin[0]) / level.resolution).powi(2) + (c[2] as f32 - (spawn[2] - level.origin[2]) / level.resolution).powi(2);
        d(a).total_cmp(&d(b))
    });
    println!("nearest floor cell: {:?}", cell);
}

#[allow(dead_code)]
fn inspect_heightfield(physics: &Physics) {
    for (_, collider) in physics.world.colliders.iter() {
        if let Some(hf) = collider.shape().as_heightfield() {
            let heights = hf.heights();
            println!("heightfield scale {:?}", hf.scale());
            println!("value at (652,1763) = {}", heights[(652, 1763)]);
            println!("value at (652,1762) = {}", heights[(652, 1762)]);
            println!("value at (653,1763) = {}", heights[(653, 1763)]);
            let max = (0..652).map(|i| heights[(i, 0)]).fold(f32::MIN, f32::max);
            println!("row-0 sample max {}", max);
        }
    }
}

#[allow(dead_code)]
fn footprint(level: &LevelFloor, spawn: [f32; 3]) {
    let res = level.resolution;
    let (cx, cz) = ((spawn[0] - level.origin[0]) / res, (spawn[2] - level.origin[2]) / res);
    let radius = 1.0 / res;
    let mut tops: Vec<f32> = Vec::new();
    let mut holes = 0;
    let mut total = 0;
    for dx in -(radius as i32)..=(radius as i32) {
        for dz in -(radius as i32)..=(radius as i32) {
            total += 1;
            let (x, z) = ((cx as i32 + dx) as u32, (cz as i32 + dz) as u32);
            match level.cells.iter().find(|c| c[0] == x && c[2] == z) {
                Some(c) => tops.push((c[1] as f32 + 1.0) * res + level.origin[1]),
                None => holes += 1,
            }
        }
    }
    let min = tops.iter().cloned().fold(f32::MAX, f32::min);
    let max = tops.iter().cloned().fold(f32::MIN, f32::max);
    println!("footprint {total} columns: {holes} holes, floor tops {min:.3}..{max:.3}");
}
