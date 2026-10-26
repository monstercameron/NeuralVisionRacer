//! Controlled full-street drive rig: road_drive <level> <report.json>. RAW=1 compares original collision.
//! Holds horizontal speed/heading along the road; vertical motion, pitch, roll and wheel contacts remain physical.
use rapier3d::prelude::*;
use splat_engine::{road::RoadProfile, CarLoad, Engine, EngineConfig, Level, LevelFloor, Logger};
use std::{path::Path, sync::Arc};
fn main() {
    let a: Vec<_> = std::env::args().skip(1).collect();
    let name = &a[0];
    let mut cfg = EngineConfig::parse(&std::fs::read_to_string("engine.conf").unwrap()).unwrap();
    cfg.level_path = format!("levels/{name}");
    let mut road = RoadProfile::for_level(&cfg.level_path).expect("road profile");
    let from = std::env::var("FROM")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(-72.0);
    let to = std::env::var("TO")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(75.0);
    let offsets = std::env::var("OFFSETS")
        .unwrap_or("-1.25,0,1.25".into())
        .split(',')
        .map(|v| v.parse::<f32>().unwrap())
        .collect::<Vec<_>>();
    // Extend guidance only: collision still loads the actual authored profile. This exposes old end cutoffs.
    if to + 3.0 > road.knots.last().unwrap()[2] {
        let n = road.knots.len();
        let a = road.knots[n - 2];
        let b = road.knots[n - 1];
        let z = to + 3.0;
        road.knots.push([
            b[0] + (b[0] - a[0]) * (z - b[2]) / (b[2] - a[2]),
            b[1] + (b[1] - a[1]) * (z - b[2]) / (b[2] - a[2]),
            z,
        ]);
    }
    if from - 3.0 < road.knots[0][2] {
        let a = road.knots[0];
        let b = road.knots[1];
        let z = from - 3.0;
        road.knots.insert(
            0,
            [
                a[0] + (b[0] - a[0]) * (z - a[2]) / (b[2] - a[2]),
                a[1] + (b[1] - a[1]) * (z - a[2]) / (b[2] - a[2]),
                z,
            ],
        );
    }
    let floor = LevelFloor::load(Path::new(&cfg.level_path), cfg.level_floor_clearance_m).unwrap();
    let bundle = splat_render::sog::load(Path::new(&cfg.splat_path)).unwrap();
    let centres = splat_render::sog::centres(&bundle);
    if std::env::var("RAW").is_ok() {
        cfg.level_path = "baseline".into();
    }
    cfg.vehicle_auto_spawn = false;
    let mut results = vec![];
    for sign in [1.0f32, -1.0] {
        for &offset in &offsets {
            let z = if sign > 0.0 { from } else { to };
            let p = road.centre(z).unwrap();
            cfg.vehicle_spawn = [p[0] + offset, p[1] + 0.68, z];
            let log = Arc::new(Logger::open(None, Level::Error, 20).unwrap());
            let mut e = Engine::new(
                cfg.clone(),
                None,
                log,
                CarLoad::Loaded(centres.clone()),
                Some(floor.clone()),
            );
            let next = road.centre(z + sign * 0.2).unwrap();
            let yaw =
                (next[0] - p[0]).atan2(next[2] - p[2]) - cfg.vehicle_forward_degrees.to_radians();
            e.physics.set_spawn_yaw(yaw);
            e.physics.reset_vehicle(cfg.vehicle_spawn).unwrap();
            for _ in 0..120 {
                e.tick();
            }
            let (mut steps, mut airborne, mut hull_hits, mut max_impact, mut max_error) =
                (0, 0, 0, 0.0f32, 0.0f32);
            let mut wheel_contacts = [0usize; 4];
            let mut hit_events = vec![];
            let mut completed = false;
            for _ in 0..1200 {
                let pos = e.physics.vehicle_pose().unwrap().position;
                if (sign > 0.0 && pos[2] >= to) || (sign < 0.0 && pos[2] <= from) {
                    completed = true;
                    break;
                }
                let Some(p) = road.centre(pos[2]) else { break };
                let Some(next) = road
                    .centre((pos[2] + sign).clamp(road.knots[0][2], road.knots.last().unwrap()[2]))
                else {
                    break;
                };
                let dx = next[0] - p[0];
                let dz = next[2] - p[2];
                let n = dx.hypot(dz);
                let target = (dx).atan2(dz);
                let handle = e.physics.vehicle().unwrap().body;
                let b = &mut e.physics.world.bodies[handle];
                let v = b.linvel();
                b.set_linvel(
                    Vector::new(
                        dx / n * 20.0 + ((p[0] + offset - pos[0]) * 2.0).clamp(-2.0, 2.0),
                        v.y,
                        dz / n * 20.0,
                    ),
                    true,
                );
                let f = *b.rotation() * Vector::new(0.0, 0.0, -1.0);
                let heading = f.x.atan2(f.z);
                let err = (target - heading + std::f32::consts::PI)
                    .rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                let w = b.angvel();
                b.set_angvel(Vector::new(w.x, err * 6.0, w.z), true);
                e.tick();
                steps += 1;
                let states = e.physics.wheel_states();
                for (i, (down, _)) in states.iter().enumerate() {
                    wheel_contacts[i] += usize::from(*down);
                }
                if states.iter().all(|w| !w.0) {
                    airborne += 1;
                }
                let s = e.physics.sample().unwrap();
                let impact = s.chassis_impact_mps.unwrap_or(0.0);
                max_impact = max_impact.max(impact);
                if impact > 0.05 {
                    hull_hits += 1;
                    if hit_events.len() < 8 {
                        hit_events.push(serde_json::json!({"position":e.physics.vehicle_pose().unwrap().position,"impact":impact,
                          "hull":format!("{:?}",e.physics.world.colliders[e.physics.vehicle().unwrap().collider].compute_aabb()),
                          "contacts":e.physics.world.contact_pairs_with(e.physics.vehicle().unwrap().collider).filter(|p|p.has_any_active_contact()).map(|p|{
                            let h=e.physics.vehicle().unwrap().collider;let other=if p.collider1==h{p.collider2}else{p.collider1};
                            serde_json::json!({"shape":format!("{:?}",e.physics.world.colliders[other].shape().shape_type()),"aabb":format!("{:?}",e.physics.world.colliders[other].compute_aabb()),"normal":format!("{:?}",p.manifolds().first().map(|m|m.data.normal))})}).collect::<Vec<_>>()}));
                    }
                }
                max_error = max_error
                    .max((e.physics.vehicle_pose().unwrap().position[1] - (p[1] + 0.68)).abs());
                if !s.in_bounds {
                    break;
                }
            }
            results.push(serde_json::json!({"direction":sign,"offset_m":offset,"completed":completed,"seconds":steps as f32/60.,"airborne_ticks":airborne,"hull_hit_ticks":hull_hits,"hit_events":hit_events,"max_impact_mps":max_impact,"max_height_error_m":max_error,"wheel_contact_fraction":wheel_contacts.map(|n|n as f32/steps.max(1) as f32),"end":e.physics.vehicle_pose().unwrap().position,"road":e.physics.road_profile()}));
        }
    }
    std::fs::write(&a[1], serde_json::to_vec_pretty(&results).unwrap()).unwrap();
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}
