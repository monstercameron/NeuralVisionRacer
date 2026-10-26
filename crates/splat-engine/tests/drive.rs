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
fn hard_wall_collision_stays_at_the_crash_site_until_manual_reset() {
    use rapier3d::prelude::*;
    let mut e = engine();
    e.physics.reset_vehicle([20.0, 0.70, 0.0]).unwrap();
    e.physics.world.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(25.0, 2.0, 0.0)),
        ColliderBuilder::cuboid(0.5, 3.0, 10.0),
    );
    let handle = e.physics.vehicle().unwrap().body;
    e.physics.world.bodies[handle].set_linvel(Vector::new(40.0, 0.0, 0.0), true);
    let mut max_impact = 0.0f32;
    for _ in 0..360 {
        e.tick();
        max_impact = max_impact.max(e.physics.sample().unwrap().chassis_impact_mps.unwrap_or(0.0));
        let state = status(&e)["vehicle"]["state"].as_str().unwrap().to_owned();
        assert!(!state.starts_with("Wrecked") && !state.starts_with("Respawning"), "collision changed state to {state}");
        assert!(e.physics.vehicle_pose().unwrap().position[0] > 10.0, "car teleported away from the wall");
    }
    assert!(max_impact > 15.0, "must exercise an impact above the old wreck threshold: {max_impact}");
    assert!(e.execute(splat_engine::Command::Reset).0.to_json_line().contains("\"ok\":true"));
    assert!(e.physics.vehicle_pose().unwrap().position[0].abs() < 0.01, "manual reset still returns to spawn");
}

#[test]
fn tire_marks_follow_real_contacts_and_do_not_connect_a_reset() {
    let mut e=engine();
    assert!(e.physics.tire_marks.marks.is_empty(),"a settled car must not draw stationary tread");
    e.physics.set_controls(Controls{throttle:1.0,..Default::default()});
    for _ in 0..120 {e.tick();}
    let marks=&e.physics.tire_marks.marks;
    assert!(marks.len()>40,"actual driving must deposit four-wheel tread");
    assert!(marks.iter().any(|m|(m.width-0.275).abs()<0.001));
    assert!(marks.iter().any(|m|(m.width-0.335).abs()<0.001));
    for m in marks {
        let length=(0..3).map(|i|(m.end[i]-m.start[i]).powi(2)).sum::<f32>().sqrt();
        assert!((length-0.25).abs()<0.001,"continuous distance spacing: {length}");
        assert!(m.normal[1]>0.9);
    }
    let first=marks[0];
    e.physics.reset_vehicle([500.0,0.7,0.0]).unwrap();
    for _ in 0..15 {e.tick();}
    assert_eq!(e.physics.tire_marks.marks[0].start,first.start,"old tread is fixed in world space");
    assert!(e.physics.tire_marks.marks.iter().all(|m| (0..3).map(|i|(m.end[i]-m.start[i]).powi(2)).sum::<f32>()<0.07),"reset cannot create a cross-map stripe");
    assert_eq!(e.profile_json()["tire_marks"]["wheel_speed_source"],"independent driven rotation from axle torque, loaded grip and engine/gearing; passive wheels roll");
}

#[test]
fn paused_contact_comparisons_do_not_move_the_car_or_chase_camera() {
    let mut e=engine();
    e.set_controls(Controls{throttle:1.0,..Default::default()});
    for _ in 0..90 {e.tick();}
    e.execute(splat_engine::Command::Pause);
    assert!(e.paused());
    e.set_render_alpha(0.0);
    let car=e.render_car_pose();let camera=e.render_camera();let time=e.render_seconds();
    e.set_render_alpha(0.99);
    assert_eq!(car,e.render_car_pose());assert_eq!(camera,e.render_camera());assert_eq!(time,e.render_seconds());
}

#[test]
fn held_s_and_w_each_stop_before_accelerating_in_the_other_direction() {
    let mut e=engine();
    e.set_controls(Controls{throttle:1.0,..Default::default()});
    for _ in 0..90 {e.tick();}
    assert!(e.physics.car_output().unwrap().speed_mps>10.0);
    e.set_controls(Controls{brake:1.0,..Default::default()});
    let mut changed=false;
    for _ in 0..360 {
        e.tick();let out=e.physics.car_output().unwrap();
        if out.gear<0 {assert!(out.speed_mps.abs()<0.001,"reverse selected before a stop: {out:?}");changed=true;break;}
        assert_eq!(out.throttle,0.0,"S must brake forward travel");
    }
    assert!(changed,"held S must automatically become reverse gas after stopping");
    for _ in 0..90 {e.tick();}
    assert!(e.physics.car_output().unwrap().speed_mps < -3.0);
    e.set_controls(Controls{throttle:1.0,..Default::default()});
    changed=false;
    for _ in 0..360 {
        e.tick();let out=e.physics.car_output().unwrap();
        if out.gear>0 {assert!(out.speed_mps.abs()<0.001,"forward selected before a stop: {out:?}");changed=true;break;}
        assert_eq!(out.throttle,0.0,"W must brake reverse travel");
    }
    assert!(changed,"held W must become forward gas after stopping");
    for _ in 0..90 {e.tick();}
    assert!(e.physics.car_output().unwrap().speed_mps>3.0);
}

#[test]
fn handbrake_locks_rear_wheels_leaves_skids_and_releases_without_selecting_reverse() {
    let mut e=engine();
    e.set_controls(Controls{throttle:1.0,..Default::default()});
    for _ in 0..90 {e.tick();}
    let speed=e.physics.vehicle_speed();
    e.set_controls(Controls{handbrake:1.0,..Default::default()});
    for _ in 0..5 {e.tick();}
    let wheels=e.physics.visual_wheels();
    assert!(wheels[0].angular_speed.abs()>1.0 && wheels[1].angular_speed.abs()>1.0,"front tires should keep rolling");
    assert_eq!(wheels[2].angular_speed,0.0);assert_eq!(wheels[3].angular_speed,0.0);
    let out=e.physics.car_output().unwrap();assert!(out.handbrake>0.9 && out.handbrake_rear_n>0.0);
    assert_eq!(out.brake_front_n,0.0);assert_eq!(out.brake,0.0);
    let tires=e.physics.tire_marks.profile_json();
    for i in [2,3] {assert_eq!(tires["wheels"][i]["locked"],true);assert!(tires["wheels"][i]["smear"].as_f64().unwrap()>0.5);}
    assert!(e.physics.vehicle_speed()<speed);
    e.set_controls(Controls::default());e.tick();
    assert!(e.physics.visual_wheels()[2].angular_speed.abs()>1.0,"releasing handbrake must restore wheel rotation");
    e.set_controls(Controls{handbrake:1.0,..Default::default()});
    for _ in 0..360 {e.tick();}
    assert!(e.physics.vehicle_speed()<0.15,"handbrake should hold the stopped car");
    assert!(e.physics.car_output().unwrap().gear>0,"handbrake alone never selects reverse");
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
fn visual_wheels_roll_with_road_speed_and_reverse_but_not_engine_idle() {
    let mut engine=engine();
    let initial=engine.physics.visual_wheels();
    for _ in 0..60 {engine.tick();}
    let idle=engine.physics.visual_wheels();
    assert!(engine.physics.car_output().unwrap().rpm>500.0);
    for i in 0..4 {
        assert!((idle[i].spin-initial[i].spin).abs()<0.05,"idling wheel spins: {:?}",idle[i]);
        assert!(idle[i].angular_speed.abs()<0.05,"idle must not generate wheel blur");
    }
    let geometry=engine.physics.wheel_geometry().unwrap();
    let handle=engine.physics.vehicle().unwrap().body;
    engine.physics.world.bodies.get_mut(handle).unwrap().set_linvel(geometry.forward*20.0,true);
    engine.tick();
    let rolling=engine.physics.visual_wheels();
    let car=engine.physics.car().unwrap();
    for i in 0..4 {
        let radius=if i<2 {car.spec.front_radius_m}else{car.spec.rear_radius_m};
        let expected=20.0/(60.0*radius);
        assert!(((rolling[i].spin-idle[i].spin) as f32-expected).abs()<0.08,"wheel {i}: {:?}",rolling[i]);
        assert!((rolling[i].angular_speed-20.0/radius).abs()<2.0,"wheel blur must follow road speed");
    }
    engine.physics.world.bodies.get_mut(handle).unwrap().set_linvel(-geometry.forward*10.0,true);
    engine.tick();
    let reverse=engine.physics.visual_wheels();
    for i in 0..4 {
        assert!(reverse[i].spin<rolling[i].spin,"reverse wheel {i}");
        assert!(reverse[i].angular_speed<0.0,"reverse blur speed must retain its direction");
    }
}

#[test]
fn visual_phase_interpolates_forward_even_beyond_half_a_revolution_per_tick() {
    let mut engine=engine();
    let before=engine.physics.visual_wheels();
    let geometry=engine.physics.wheel_geometry().unwrap();
    let handle=engine.physics.vehicle().unwrap().body;
    engine.physics.world.bodies.get_mut(handle).unwrap().set_linvel(geometry.forward*100.0,true);
    engine.tick();
    let after=engine.physics.visual_wheels();
    engine.set_render_alpha(0.5);
    let rendered=engine.render_wheels();
    for i in 0..4 {
        assert!(after[i].spin-before[i].spin>std::f64::consts::PI);
        assert!((rendered[i].spin-(before[i].spin+after[i].spin)*0.5).abs()<1e-9);
    }
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

#[test]
fn blocked_chassis_spins_only_driven_wheels_and_builds_two_local_patches() {
    let mut e = engine();
    let body = e.physics.vehicle().unwrap().body;
    let position = e.physics.vehicle_pose().unwrap().position;
    {
        let b = e.physics.world.bodies.get_mut(body).unwrap();
        b.set_linvel(rapier3d::prelude::Vector::ZERO, true);
        b.set_angvel(rapier3d::prelude::Vector::ZERO, true);
        b.set_locked_axes(rapier3d::prelude::LockedAxes::all(), true);
    }
    e.set_controls(Controls { throttle: 1.0, ..Default::default() });
    for _ in 0..120 { e.tick(); }
    let wheels = e.physics.visual_wheels();
    assert_eq!(e.physics.vehicle_pose().unwrap().position, position);
    assert!(e.physics.vehicle_velocity().iter().all(|v| v.abs() < 0.001));
    assert!(wheels[..2].iter().all(|w| w.angular_speed.abs() < 0.05));
    assert!(wheels[2..].iter().all(|w| w.grounded && w.angular_speed > 10.0 && w.spin > 10.0));
    let marks = &e.physics.tire_marks.marks;
    assert_eq!(marks.len(), 2, "one bounded patch per driven tire");
    let strength = marks[0].strength;
    assert!(strength > 0.4 && strength <= 0.8);
    let first = marks[0].start;
    for _ in 0..120 { e.tick(); }
    assert_eq!(e.physics.tire_marks.marks.len(), 2);
    assert_eq!(e.physics.tire_marks.marks[0].start, first);
    assert!(e.physics.tire_marks.marks[0].strength > strength);
    let output = e.physics.car_output().unwrap();
    let limit = output.rpm / output.drive_ratio.abs() * std::f32::consts::TAU / 60.0;
    assert!(e.physics.visual_wheels()[2..].iter().all(|w| w.angular_speed <= limit + 0.01));
    println!("blocked_burnout={}", serde_json::json!({
        "chassis_mps": e.physics.vehicle_velocity(),
        "wheel_rad_s": e.physics.visual_wheels().map(|w| w.angular_speed),
        "rpm": output.rpm, "applied_ratio": output.drive_ratio,
        "patch_count": e.physics.tire_marks.marks.len(),
        "opacity_after_2s": strength, "opacity_after_4s": e.physics.tire_marks.marks[0].strength
    }));
    // A mechanical rear handbrake still wins over the motor.
    e.set_controls(Controls { throttle: 1.0, handbrake: 1.0, ..Default::default() });
    for _ in 0..30 { e.tick(); }
    assert!(e.physics.visual_wheels()[2..].iter().all(|w| w.angular_speed == 0.0));
    let strength = e.physics.tire_marks.marks[0].strength;
    for _ in 0..30 { e.tick(); }
    assert_eq!(e.physics.tire_marks.marks[0].strength, strength);
    e.physics.reset_vehicle([0.0, 0.7, 0.0]).unwrap();
    assert!(e.physics.visual_wheels().iter().all(|w| w.angular_speed == 0.0 && w.spin == 0.0));
}

#[test]
fn blocked_reverse_and_airborne_drive_have_signed_rotation_but_only_ground_paints() {
    for airborne in [false, true] {
        let mut e = engine();
        let body = e.physics.vehicle().unwrap().body;
        if airborne { e.physics.reset_vehicle([0.0, 5.0, 0.0]).unwrap(); }
        {
            let b = e.physics.world.bodies.get_mut(body).unwrap();
            b.set_linvel(rapier3d::prelude::Vector::ZERO, true);
            b.set_angvel(rapier3d::prelude::Vector::ZERO, true);
            b.set_locked_axes(rapier3d::prelude::LockedAxes::all(), true);
        }
        e.set_controls(Controls { brake: 1.0, ..Default::default() });
        for _ in 0..120 { e.tick(); }
        assert!(e.physics.visual_wheels()[2..].iter().all(|w| w.angular_speed < -10.0), "airborne={airborne}: {:?} {:?}", e.physics.visual_wheels(), e.physics.car_output());
        assert_eq!(e.physics.tire_marks.marks.len(), if airborne { 0 } else { 2 });
    }
}

#[test]
fn actual_wheel_dust_freezes_when_paused_and_resets_without_air_emission() {
    let mut e=engine();e.config.level_path="levels/saburi-alley-yazd".into();
    assert!(e.physics.dust.particles.is_empty());
    e.set_controls(Controls{throttle:1.0,..Default::default()});
    for _ in 0..60 {e.tick();}
    assert!(!e.physics.dust.particles.is_empty(),"actual loaded tire contacts must raise dust");
    e.execute(splat_engine::Command::Pause);
    let time=e.physics.dust.time;let count=e.physics.dust.particles.len();
    for _ in 0..30 {e.tick();}
    assert_eq!(time,e.physics.dust.time);assert_eq!(count,e.physics.dust.particles.len());
    e.execute(splat_engine::Command::Resume);
    e.physics.reset_vehicle([0.0,10.0,0.0]).unwrap();
    assert!(e.physics.dust.particles.is_empty());
    for _ in 0..15 {e.tick();}
    assert!(e.physics.dust.particles.is_empty(),"airborne engine revs must not emit dust");
}

#[test]
fn foliage_sensors_cannot_lift_the_car_or_become_wheel_ground() {
    use rapier3d::prelude::*;
    let mut e=engine();
    let before=e.physics.vehicle_pose().unwrap().position;
    e.physics.world.insert(RigidBodyBuilder::fixed().translation(Vector::new(0.0,0.3,0.0)),
        ColliderBuilder::cuboid(8.0,0.15,8.0).sensor(true).user_data(splat_engine::foliage::SENSOR_TAG));
    for _ in 0..120 {e.tick();}
    let after=e.physics.vehicle_pose().unwrap().position;
    assert!((before[1]-after[1]).abs()<0.02,"wheel rays must pass through foliage to the actual road: {before:?} -> {after:?}");
    assert_eq!(e.physics.wheel_states().iter().filter(|(ground,_)|*ground).count(),4);
    assert!(e.physics.sample().unwrap().chassis_impact_mps.unwrap_or(0.0)<0.01);
    assert!(e.physics.foliage.lost_energy_j<0.01,"stationary foliage contact cannot consume energy");
}
