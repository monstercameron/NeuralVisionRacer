//! Casts horizontal rays at car height from the road spawn and prints where the level's wall boxes stop them.
//! `cargo run --release --example probe_walls -- <level dir>`
use rapier3d::prelude::*;
use splat_engine::{EngineConfig, LevelFloor, Physics};
use std::path::Path;

fn main() {
    let dir = std::env::args().nth(1).expect("level directory");
    let config = EngineConfig::default();
    let level = LevelFloor::load(Path::new(&dir), config.level_floor_clearance_m).expect("level");
    let spawn = level.road_spawn().unwrap_or(config.vehicle_spawn);
    let wall_columns = level.walls.iter().filter(|w| w.is_some()).count();
    let mut physics = Physics::new(&config);
    physics.set_level(&level, &config);
    physics.stream_collision(spawn);
    let boxes = physics.world.colliders.len();
    println!("road spawn {spawn:?}; {wall_columns} candidate wall columns of {:.2} m -> {boxes} streamed floor/obstacle colliders", level.block_m);
    let at: Vec<f32> = std::env::var("AT").unwrap_or("0.3,-8".into()).split(",").map(|v| v.parse().unwrap()).collect();
    wall_map(&level, [at[0], spawn[1], at[1]], 3.0);
    physics.world.step();
    let pipeline = physics.world.query_pipeline();
    for height in [0.5f32, 1.0] {
        for (name, dir) in [("+x", [1.0, 0.0, 0.0]), ("-x", [-1.0, 0.0, 0.0]), ("+z", [0.0, 0.0, 1.0]), ("-z", [0.0, 0.0, -1.0])] {
            let origin = Vector::new(spawn[0], spawn[1] + height, spawn[2]);
            let ray = Ray::new(origin, Vector::new(dir[0], dir[1], dir[2]));
            match pipeline.cast_ray(&ray, config.level_collision_radius_m, true) {
                Some((_, toi)) => println!("{height:.1} m up, {name}: wall at {toi:.2} m"),
                None => println!("{height:.1} m up, {name}: open"),
            }
        }
    }
}

/// Prints the wall columns around `centre` (world x, z) as a map: `#` wall, `.` open. Rows run along z.
#[allow(dead_code)]
pub fn wall_map(level: &LevelFloor, centre: [f32; 3], half_m: f32) {
    let b = level.block_m;
    let (cx, cz) = (((centre[0] - level.origin[0]) / b) as i64, ((centre[2] - level.origin[2]) / b) as i64);
    let r = (half_m / b) as i64;
    for dz in -r..=r {
        let row: String = (-r..=r)
            .map(|dx| {
                let (x, z) = (cx + dx, cz + dz);
                if x < 0 || z < 0 || x as usize >= level.wall_dims[0] || z as usize >= level.wall_dims[1] {
                    return ' ';
                }
                let here = dx == 0 && dz == 0;
                match (level.walls[x as usize * level.wall_dims[1] + z as usize], here) {
                    (_, true) => 'X',
                    (Some(_), _) => '#',
                    (None, _) => '.',
                }
            })
            .collect();
        println!("z {:>7.2} {row}", level.origin[2] + (cz + dz) as f32 * b);
    }
}
