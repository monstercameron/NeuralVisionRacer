//! Drives the real car down a level at full throttle and prints what the chassis touches.
//! `cargo run --release --example probe_drive -- <car.sog> <level dir> [seconds]`
use splat_car::Controls;
use splat_engine::log::Level;
use splat_engine::{CarLoad, EngineConfig, Engine, LevelFloor, Logger};
use std::path::Path;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let car = splat_render::sog::load(Path::new(&args[0])).expect("car");
    let config = EngineConfig::parse(&std::fs::read_to_string(std::env::var("CONF").unwrap_or("../../engine.conf".into())).expect("engine.conf")).expect("config");
    let level = LevelFloor::load(Path::new(&args[1]), config.level_floor_clearance_m).expect("level");
    let seconds: f32 = args.get(2).map_or(4.0, |s| s.parse().unwrap());
    let log = Arc::new(Logger::open(None, Level::Warn, 100).unwrap());
    let mut engine = Engine::new(config, None, log, CarLoad::Loaded(splat_render::sog::centres(&car)), Some(level));
    for _ in 0..120 {
        engine.tick();
    }
    if let Some(vehicle) = engine.physics.vehicle() {
        let aabb = engine.physics.world.colliders[vehicle.collider].compute_aabb();
        let size = aabb.maxs - aabb.mins;
        println!("chassis hull {:.2} x {:.2} x {:.2} m (x, y, z)", size.x, size.y, size.z);
        let body = &engine.physics.world.bodies[vehicle.body];
        println!("hull box {:?}..{:?}; body translation {:?}; centre of mass {:?}", aabb.mins, aabb.maxs, body.translation(), body.center_of_mass());
    }
    report(&engine, "settled");
    // SEQ="W:2,S:2,WA:1": held keys (W throttle, S brake/reverse, A/D steer) and seconds; default full throttle.
    let seq = std::env::var("SEQ").unwrap_or_else(|_| format!("W:{seconds}"));
    let mut t = 0.0;
    for part in seq.split(',') {
        let (keys, secs) = part.split_once(':').expect("KEYS:seconds");
        let secs: f32 = secs.parse().unwrap();
        let steer = if keys.contains('A') { -1.0 } else if keys.contains('D') { 1.0 } else { 0.0 };
        engine.set_controls(Controls { throttle: f32::from(u8::from(keys.contains('W'))), brake: f32::from(u8::from(keys.contains('S'))), steer, shift: 0, handbrake: 0.0 });
        let trace = std::env::var("TRACE").ok().and_then(|v| {
            let (a, b) = v.split_once(',')?;
            Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?))
        });
        for _ in 0..(secs * 4.0) as usize {
            for i in 0..15 {
                engine.tick();
                let now = t + (i + 1) as f32 / 60.0;
                if trace.is_some_and(|(a, b)| now >= a && now <= b) {
                    trace_tick(&engine, now);
                }
            }
            t += 0.25;
            report(&engine, &format!("{keys} {t:.2} s"));
        }
    }
    if std::env::var("SEQ").is_ok() {
        return;
    }
    // TRACE=from,to (seconds): every tick in that window, the speed, attitude and the chassis's contacts per body.
    let trace = std::env::var("TRACE").ok().and_then(|t| {
        let (a, b) = t.split_once(',')?;
        Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?))
    });
    for step in 1..=(seconds * 4.0) as usize {
        for i in 0..15 {
            engine.tick();
            let t = (step - 1) as f32 / 4.0 + (i + 1) as f32 / 60.0;
            if trace.is_some_and(|(a, b)| t >= a && t <= b) {
                trace_tick(&engine, t);
            }
        }
        report(&engine, &format!("{:.2} s", step as f32 / 4.0));
    }
}

fn trace_tick(engine: &Engine, t: f32) {
    let physics = &engine.physics;
    let Some(vehicle) = physics.vehicle() else { return };
    let body = &physics.world.bodies[vehicle.body];
    let (axis, angle) = body.rotation().to_axis_angle();
    let mut per_body: std::collections::BTreeMap<String, (usize, f32, [f32; 3])> = Default::default();
    for pair in physics.world.contact_pairs_with(vehicle.collider) {
        if !pair.has_any_active_contact() {
            continue;
        }
        let other = if pair.collider1 == vehicle.collider { pair.collider2 } else { pair.collider1 };
        let flip = if pair.collider1 == vehicle.collider { 1.0 } else { -1.0 };
        let key = format!("{:?}", physics.world.colliders[other].parent().map(|h| h.into_raw_parts().0));
        if physics.world.colliders[other].parent().map(|h| h.into_raw_parts().0) == Some(1) {
            let aabb = physics.world.colliders[other].compute_aabb();
            let car = physics.world.colliders[vehicle.collider].compute_aabb();
            println!("    wall box {:?}..{:?}, car hull {:?}..{:?}", aabb.mins, aabb.maxs, car.mins, car.maxs);
        }
        let entry = per_body.entry(key).or_insert((0, 0.0, [0.0; 3]));
        entry.0 += 1;
        entry.1 += pair.total_impulse_magnitude();
        if let Some(m) = pair.manifolds().first() {
            let n = m.data.normal * flip;
            entry.2 = [n.x, n.y, n.z];
        }
    }
    let v = body.linvel();
    println!(
        "  t {t:.3} v ({:.1},{:.1},{:.1}) tilt {:.1}° about ({:.2},{:.2},{:.2}) wheels {:?} contacts {per_body:?}",
        v.x, v.y, v.z, angle.to_degrees(), axis.x, axis.y, axis.z,
        physics.wheel_states().iter().map(|w| w.0 as u8).collect::<Vec<_>>()
    );
}

fn report(engine: &Engine, label: &str) {
    let s = engine.status_json();
    let car = &s["car"];
    let wheels: String = engine.physics.wheel_states().iter().map(|(on, len)| if *on { format!("{:.2} ", len) } else { "---- ".into() }).collect();
    println!("{label}: pos {} contacts {} wheels [{wheels}] | {} km/h gear {} rpm {} spin {}", s["vehicle"]["position"], s["vehicle"]["ground_contacts"], car["speed_kmh"], car["gear"], car["rpm"], car["wheelspin"]);
    let physics = &engine.physics;
    let Some(vehicle) = physics.vehicle() else { return };
    let body = &physics.world.bodies[vehicle.body];
    for pair in physics.world.contact_pairs_with(vehicle.collider) {
        if !pair.has_any_active_contact() {
            continue;
        }
        for manifold in pair.manifolds() {
            let other = if pair.collider1 == vehicle.collider { pair.collider2 } else { pair.collider1 };
            let other_body = physics.world.colliders[other].parent();
            let n = manifold.data.normal;
            let p = manifold.points.first().map(|p| body.position() * p.local_p1);
            println!("    hull touches {other_body:?}: normal ({:.2},{:.2},{:.2}) at {:?} impulse {:.0}", n.x, n.y, n.z, p.map(|p| [p.x, p.y, p.z].map(|v| (v * 100.0).round() / 100.0)), pair.total_impulse_magnitude());
        }
    }
}
