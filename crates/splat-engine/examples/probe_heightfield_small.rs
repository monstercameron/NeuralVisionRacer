//! Checks rapier's heightfield ray queries on small and large flat grids.
use rapier3d::prelude::*;
use splat_engine::{EngineConfig, Physics};

fn probe(nx: usize, nz: usize, label: &str) {
    let config = EngineConfig::default();
    let mut physics = Physics::new(&config);
    let heights = rapier3d::parry::utils::Array2::from_fn(nx, nz, |_, _| 1.0f32);
    let body = RigidBodyBuilder::fixed().translation(Vector::new(0.0, 0.0, 0.0));
    let collider = ColliderBuilder::heightfield(heights, Vector::new(0.05, 1.0, 0.05));
    physics.world.insert(body, collider);
    physics.world.step();
    let pipeline = physics.world.query_pipeline();
    let ray = Ray::new(Vector::new(nx as f32 * 0.025, 5.0, nz as f32 * 0.025), Vector::new(0.0, -1.0, 0.0));
    match pipeline.cast_ray(&ray, 100.0, true) {
        Some((_, toi)) => println!("{label}: hit at y={:.3} (expect 1.0)", 5.0 - toi),
        None => println!("{label}: no hit"),
    }
}

fn probe_cuboid() {
    let config = EngineConfig::default();
    let mut physics = Physics::new(&config);
    let body = RigidBodyBuilder::fixed().translation(Vector::new(0.0, 0.5, 0.0));
    let collider = ColliderBuilder::cuboid(5.0, 0.5, 5.0);
    physics.world.insert(body, collider);
    physics.world.step();
    let pipeline = physics.world.query_pipeline();
    let ray = Ray::new(Vector::new(0.0, 5.0, 0.0), Vector::new(0.0, -1.0, 0.0));
    match pipeline.cast_ray(&ray, 100.0, true) {
        Some((_, toi)) => println!("cuboid: hit at y={:.3} (expect 1.0)", 5.0 - toi),
        None => println!("cuboid: no hit"),
    }
}

fn main() {
    probe_cuboid();
    probe(10, 10, "10x10");
    probe(200, 200, "200x200");
    probe(1040, 3528, "1040x3528");
}
