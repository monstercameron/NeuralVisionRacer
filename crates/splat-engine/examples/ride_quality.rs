//! How smoothly the car rides a level: drives a key sequence and measures the chassis's bouncing.
//! `cargo run --release --example ride_quality -- <car.sog> <level dir>`
//!
//! SEQ="W:6" (held keys and seconds, as in probe_drive), CONF=path/to/engine.conf. HOLD=1 steers to keep the starting
//! heading; RIG=m/s instead pushes the car at that speed along its starting heading (turned by ANGLE= degrees, or
//! reversed by RIG_BACK=1), so runs compare on the same road; TRACE=1 prints every tick. Prints the RMS vertical
//! acceleration and pitch/roll rates of the chassis, how often each wheel touches the road, the time with no wheel
//! down, the distance covered and the top speed, up to a quarter second before the hull first hits something.
use splat_car::Controls;
use splat_engine::log::Level;
use splat_engine::{CarLoad, Engine, EngineConfig, LevelFloor, Logger};
use std::path::Path;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let car = splat_render::sog::load(Path::new(&args[0])).expect("car");
    let config = EngineConfig::parse(&std::fs::read_to_string(std::env::var("CONF").unwrap_or("../../engine.conf".into())).expect("engine.conf")).expect("config");
    let level = LevelFloor::load(Path::new(&args[1]), config.level_floor_clearance_m).expect("level");
    let log = Arc::new(Logger::open(None, Level::Warn, 100).unwrap());
    let started = std::time::Instant::now();
    let mut engine = Engine::new(config, None, log, CarLoad::Loaded(splat_render::sog::centres(&car)), Some(level));
    println!("world built in {:.2} s", started.elapsed().as_secs_f32());
    for _ in 0..120 {
        engine.tick();
    }
    let dt = 1.0 / 60.0;
    let (mut ticks, mut accel_sq, mut pitch_sq, mut roll_sq, mut airborne) = (0usize, 0.0f64, 0.0f64, 0.0f64, 0usize);
    let mut wheel_down = [0usize; 4];
    let (mut floor_hits, mut floor_impulse) = (0usize, 0.0f32);
    let (mut distance, mut top_speed) = (0.0f32, 0.0f32);
    let state = |engine: &Engine| {
        let vehicle = engine.physics.vehicle().expect("vehicle");
        let body = &engine.physics.world.bodies[vehicle.body];
        (body.linvel(), body.angvel(), *body.rotation(), body.translation())
    };
    let (mut last_v, _, start_rotation, mut last_p) = state(&engine);
    let forward_of = |r: rapier3d::prelude::Rotation| { let f = r * rapier3d::prelude::Vector::Z; f.x.atan2(f.z) };
    let start_heading = forward_of(start_rotation);
    let hold = std::env::var("HOLD").is_ok();
    let rig: Option<f32> = std::env::var("RIG").ok().and_then(|v| v.parse().ok());
    let rig_dir = {
        let f = start_rotation * rapier3d::prelude::Vector::Z;
        let sign = if std::env::var("RIG_BACK").is_ok_and(|v| !v.is_empty()) { -1.0 } else { 1.0 };
        let a: f32 = std::env::var("ANGLE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0f32).to_radians();
        let (sa, ca) = a.sin_cos();
        rapier3d::prelude::Vector::new(f.x * ca + f.z * sa, 0.0, -f.x * sa + f.z * ca).normalize() * sign
    };
    let mut samples: Vec<(f64, f64, f64, [bool; 4], bool, f32)> = Vec::new();
    let mut blocked_at = None;
    let seq = std::env::var("SEQ").unwrap_or_else(|_| "W:6".into());
    for part in seq.split(',') {
        let (keys, secs) = part.split_once(':').expect("KEYS:seconds");
        let secs: f32 = secs.parse().unwrap();
        let steer = if keys.contains('A') { -1.0 } else if keys.contains('D') { 1.0 } else { 0.0 };
        let mut controls = Controls { throttle: f32::from(u8::from(keys.contains('W'))), brake: f32::from(u8::from(keys.contains('S'))), steer, shift: 0, handbrake: 0.0 };
        for _ in 0..(secs * 60.0) as usize {
            if hold {
                // HOLD: steer to keep the starting heading (positive steer is right).
                let (_, _, r, _) = state(&engine);
                let error = (forward_of(r) - start_heading + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                controls.steer = (error * 3.0).clamp(-1.0, 1.0);
            }
            engine.set_controls(controls);
            if let Some(speed) = rig {
                // RIG=m/s: hold the car at this speed along its starting heading with no yaw, leaving it free to
                // bounce, pitch and roll; every run then crosses the same road.
                let vehicle = engine.physics.vehicle().expect("vehicle");
                let body = engine.physics.world.bodies.get_mut(vehicle.body).unwrap();
                let v = body.linvel();
                body.set_linvel(rapier3d::prelude::Vector::new(rig_dir.x * speed, v.y, rig_dir.z * speed), true);
                let w = body.angvel();
                body.set_angvel(rapier3d::prelude::Vector::new(w.x, 0.0, w.z), true);
            }
            engine.tick();
            let vehicle = engine.physics.vehicle().expect("vehicle");
            if let Some(pair) = engine.physics.world.contact_pairs_with(vehicle.collider).find(|p| p.total_impulse_magnitude() > 50.0) {
                let other = if pair.collider1 == vehicle.collider { pair.collider2 } else { pair.collider1 };
                let flip = if pair.collider1 == vehicle.collider { 1.0 } else { -1.0 };
                let n = pair.manifolds().first().map(|m| m.data.normal * flip);
                let shape = engine.physics.world.colliders[other].shape().shape_type();
                let speed = engine.physics.world.bodies[vehicle.body].linvel().length() * 3.6;
                println!("hull hit {shape:?} at {speed:.0} km/h, normal {n:?}");
                blocked_at.get_or_insert(ticks as f32 * dt);
                break;
            }
            let (v, w, rotation, p) = state(&engine);
            let floor_touch: f32 = engine.physics.world.contact_pairs_with(vehicle.collider).filter(|p| {
                let other = if p.collider1 == vehicle.collider { p.collider2 } else { p.collider1 };
                engine.physics.world.colliders[other].shape().as_heightfield().is_some() || engine.physics.world.colliders[other].shape().as_cuboid().is_some_and(|c| c.half_extents.y < 0.6)
            }).map(|p| p.total_impulse_magnitude()).sum();
            if floor_touch > 0.0 { floor_hits += 1; floor_impulse += floor_touch; }
            // Body-frame rates: x across the car is pitch, z along it is roll (the spec's forward is about ±z).
            let local = rotation.inverse() * w;
            if std::env::var("TRACE").is_ok() {
                let lens: Vec<String> = engine.physics.wheel_states().iter().map(|(on, l)| if *on { format!("{l:.3}") } else { "-----".into() }).collect();
                println!("t {:.3} vy {:+.3} ay {:+6.2} pitch {:+6.1} roll {:+6.1} wheels {}", ticks as f32 * dt, v.y, (v.y - last_v.y) / dt, local.x.to_degrees(), local.z.to_degrees(), lens.join(" "));
            }
            let states = engine.physics.wheel_states();
            let down: [bool; 4] = std::array::from_fn(|i| states.get(i).is_some_and(|s| s.0));
            samples.push((
                f64::from(((v.y - last_v.y) / dt).powi(2)),
                f64::from(local.x.to_degrees().powi(2)),
                f64::from(local.z.to_degrees().powi(2)),
                down,
                down.iter().all(|d| !d),
                (p - last_p).length(),
            ));
            top_speed = top_speed.max(v.length() * 3.6);
            (last_v, last_p) = (v, p);
            ticks += 1;
        }
        if blocked_at.is_some() {
            break;
        }
    }
    // Leave out the last quarter second before the hull touched something: that is the impact, not the ride.
    if blocked_at.is_some() {
        samples.truncate(samples.len().saturating_sub(15));
    }
    ticks = samples.len();
    for (a, pt, r, down, air, d) in &samples {
        accel_sq += a;
        pitch_sq += pt;
        roll_sq += r;
        for (count, on) in wheel_down.iter_mut().zip(down) {
            *count += usize::from(*on);
        }
        airborne += usize::from(*air);
        distance += d;
    }
    if let Some(t) = blocked_at {
        println!("(the hull touched something after {t:.2} s; measured up to there)");
    }
    println!("hull on the floor {floor_hits} ticks, impulse {floor_impulse:.0} N·s");
    let rms = |sum: f64| (sum / ticks.max(1) as f64).sqrt();
    let down: Vec<String> = wheel_down.iter().map(|&n| format!("{:.0}%", 100.0 * n as f32 / ticks.max(1) as f32)).collect();
    println!(
        "{seq}: vertical accel {:.2} m/s² rms, pitch {:.2}°/s rms, roll {:.2}°/s rms, wheels down {} (FL FR RL RR), airborne {:.2} s, {distance:.0} m, top {top_speed:.0} km/h",
        rms(accel_sq), rms(pitch_sq), rms(roll_sq), down.join(" "), airborne as f32 * dt
    );
}
