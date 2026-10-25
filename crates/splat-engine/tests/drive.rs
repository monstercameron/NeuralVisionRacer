//! The car on a flat ground plane: a car-sized box of points carried by the GT3 RS's wheels and mechanics.

use splat_car::Controls;
use splat_engine::log::Level;
use splat_engine::{CarLoad, EngineConfig, Engine, Logger};
use std::sync::Arc;

/// Points filling a 1.9 × 1.3 × 4.57 m box (x across, y up, z along), so the car scale stays 1.
fn car_box() -> Vec<[f32; 3]> {
    let mut points = Vec::new();
    for i in 0..=19 {
        for j in 0..=13 {
            for k in 0..=45 {
                points.push([i as f32 * 0.1, j as f32 * 0.1, k as f32 * 0.1016]);
            }
        }
    }
    points
}

fn engine() -> Engine {
    let mut config = EngineConfig::default();
    config.splat_rotate_x_degrees = 0.0;
    config.physics_ground_half_extent = 2000.0;
    // The box's centroid is 0.65 m above its base; drop it from 5 cm.
    config.vehicle_spawn = [0.0, 0.70, 0.0];
    let log = Arc::new(Logger::open(None, Level::Info, 500).unwrap());
    let mut engine = Engine::new(config, None, log, CarLoad::Loaded(car_box()), None);
    for _ in 0..120 {
        engine.tick();
    }
    engine
}

fn status(engine: &Engine) -> serde_json::Value {
    engine.status_json()
}

fn speed_kmh(engine: &Engine) -> f64 {
    status(engine)["car"]["speed_kmh"].as_f64().unwrap()
}

#[test]
fn it_settles_on_four_wheels() {
    let engine = engine();
    let s = status(&engine);
    assert_eq!(s["vehicle"]["ground_contacts"], 4, "{s}");
    let y = s["vehicle"]["position"][1].as_f64().unwrap();
    eprintln!("settled: ride height {y:.3} m, {}", s["vehicle"]);
    assert!((0.55..0.75).contains(&y), "ride height {y}");
}

#[test]
fn full_throttle_accelerates_through_the_gears_close_to_the_spec() {
    let mut engine = engine();
    engine.set_controls(Controls { throttle: 1.0, ..Default::default() });
    let mut hundred = None;
    let mut highest_gear = 1;
    for tick in 1..=60 * 12 {
        engine.tick();
        if hundred.is_none() && speed_kmh(&engine) >= 100.0 {
            hundred = Some(tick as f32 / 60.0);
        }
        if let Some(g) = status(&engine)["car"]["gear"].as_str().and_then(|g| g.parse::<i32>().ok()) {
            highest_gear = highest_gear.max(g);
        }
    }
    let hundred = hundred.expect("reached 100 km/h");
    eprintln!("0-100 km/h {hundred:.2} s, gear {highest_gear}, after 12 s: {}", status(&engine)["car"]);
    // The flat-road model gives 3.2 s; rapier's tyres and suspension add some loss.
    assert!((2.9..=4.2).contains(&hundred), "0-100 km/h in {hundred:.2} s");
    assert!(highest_gear >= 4, "shifted up to gear {highest_gear}");
    let s = status(&engine);
    assert_eq!(s["vehicle"]["ground_contacts"], 4, "still on its wheels: {s}");
}

#[test]
fn brakes_stop_the_car_and_steering_turns_it() {
    let mut engine = engine();
    engine.set_controls(Controls { throttle: 1.0, ..Default::default() });
    for _ in 0..120 {
        engine.tick();
    }
    let fast = speed_kmh(&engine);
    assert!(fast > 50.0, "{fast}");
    engine.set_controls(Controls { brake: 1.0, ..Default::default() });
    let mut ticks = 0;
    while speed_kmh(&engine).abs() > 1.0 && ticks < 600 {
        engine.tick();
        ticks += 1;
    }
    let seconds = ticks as f32 / 60.0;
    eprintln!("braked from {fast:.0} km/h in {seconds:.2} s");
    assert!(seconds < 4.0, "stopped from {fast:.0} km/h in {seconds:.2} s");

    // Steer right while rolling: the car's heading turns right (clockwise seen from above).
    let heading = |e: &Engine| {
        let q = &status(e)["vehicle"]["rotation_xyzw"];
        let (y, w) = (q[1].as_f64().unwrap(), q[3].as_f64().unwrap());
        2.0 * y.atan2(w)
    };
    engine.set_controls(Controls { throttle: 0.4, ..Default::default() });
    for _ in 0..60 {
        engine.tick();
    }
    let before = heading(&engine);
    engine.set_controls(Controls { throttle: 0.4, steer: 1.0, ..Default::default() });
    for _ in 0..90 {
        engine.tick();
    }
    let turned = heading(&engine) - before;
    eprintln!("steering right for 1.5 s turned the heading {turned:.3} rad");
    // A right turn is a negative rotation about +Y.
    assert!(turned < -0.2, "heading change {turned:.3} rad");
}

#[test]
fn flip_puts_a_car_on_its_roof_back_on_its_wheels() {
    let mut engine = engine();
    // Roll it onto its roof, a little above the ground, and let it land.
    let vehicle = engine.physics.vehicle().unwrap();
    let body = engine.physics.world.bodies.get_mut(vehicle.body).unwrap();
    let p = body.translation();
    body.set_position(rapier3d::prelude::Pose::new(p + rapier3d::prelude::Vector::new(0.0, 1.0, 0.0), rapier3d::prelude::Vector::new(0.0, 0.0, std::f32::consts::PI)), true);
    for _ in 0..180 {
        engine.tick();
    }
    assert_eq!(engine.physics.wheel_states().iter().filter(|w| w.0).count(), 0, "on its roof: {}", status(&engine)["vehicle"]);
    engine.execute(splat_engine::console::Command::Flip);
    for _ in 0..120 {
        engine.tick();
    }
    let s = status(&engine);
    assert_eq!(engine.physics.wheel_states().iter().filter(|w| w.0).count(), 4, "back on its wheels: {s}");
    let up = engine.physics.world.bodies[vehicle.body].rotation() * rapier3d::prelude::Vector::Y;
    assert!(up.y > 0.99, "upright: {up:?}");
}

/// Rolls the car (no throttle) into a long wall at `angle_degrees` and 15 m/s; returns (speed along the wall after
/// one second, total speed, how far it got along the wall).
fn wall_glance(angle_degrees: f32) -> (f32, f32, f32) {
    wall_glance_with(angle_degrees, true, |_| {})
}

fn wall_glance_with(angle_degrees: f32, with_wall: bool, tweak: impl FnOnce(&mut Engine)) -> (f32, f32, f32) {
    use rapier3d::prelude::*;
    let mut engine = engine();
    tweak(&mut engine);
    let config_friction = engine.config.physics_friction;
    // A wall across the car's path, 6 m ahead along +z, as the levels build them (a box, the level's friction).
    if with_wall {
        let wall = RigidBodyBuilder::fixed().translation(Vector::new(0.0, 1.0, 8.0));
        let collider = ColliderBuilder::cuboid(60.0, 2.0, 0.5).friction(config_friction).restitution(engine.config.physics_restitution);
        engine.physics.world.insert(wall, collider);
    }
    let vehicle = engine.physics.vehicle().unwrap();
    let a = angle_degrees.to_radians();
    let direction = Vector::new(a.sin(), 0.0, a.cos());
    let body = engine.physics.world.bodies.get_mut(vehicle.body).unwrap();
    // Face and move along the direction (the test car's body points along +z).
    let p = body.translation();
    body.set_position(Pose::new(p, Vector::new(0.0, a, 0.0)), true);
    body.set_linvel(direction * 15.0, true);
    let start = body.translation();
    for _ in 0..60 {
        engine.tick();
    }
    let body = &engine.physics.world.bodies[vehicle.body];
    let v = body.linvel();
    (v.x, v.length(), body.translation().x - start.x)
}

#[test]
fn a_car_glancing_off_a_wall_slides_along_it() {
    // At 30° into the wall, 7.5 m/s of the 15 runs along it; a car that slides keeps most of that, one that sticks
    // stops dead against the wall.
    let (along, speed, travelled) = wall_glance(30.0);
    eprintln!("glancing at 30°: {along:.2} m/s along the wall, {speed:.2} m/s in all, {travelled:.2} m along it");
    assert!(along > 4.0, "slides along the wall: {along:.2} m/s");
}

#[test]
#[ignore]
fn wall_glance_breakdown() {
    let (along, speed, _) = wall_glance_with(30.0, false, |_| {});
    eprintln!("no wall: {along:.2} along x, {speed:.2} in all");
    let (along, speed, _) = wall_glance_with(30.0, true, |e| {
        let v = e.physics.vehicle().unwrap();
        e.physics.world.colliders.get_mut(v.collider).unwrap().set_friction(0.0);
    });
    eprintln!("hull friction 0: {along:.2} along the wall, {speed:.2} in all");
    for angle in [10.0, 30.0, 60.0, 85.0] {
        let (along, speed, travelled) = wall_glance(angle);
        eprintln!("at {angle}°: {along:.2} m/s along the wall, {speed:.2} m/s in all, {travelled:.2} m along it");
    }
}
