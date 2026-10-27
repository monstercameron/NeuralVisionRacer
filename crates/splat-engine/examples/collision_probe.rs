//! CPU-only collision streaming probe; no renderer, GPU adapter or graphics backend is created.
//! collision_probe <level-folder> <output-dir> [samples=6000]
use rapier3d::prelude::PhysicsWorld;
use splat_engine::{collision::LevelCollision, EngineConfig, LevelFloor};
use std::{path::Path, time::Instant};
fn mix(hash: &mut u64, value: u64) {
    *hash = (*hash ^ value).wrapping_mul(1_099_511_628_211);
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let level = &args[0];
    let out = Path::new(&args[1]);
    std::fs::create_dir_all(out).unwrap();
    let count: usize = args.get(2).map_or(6000, |s| s.parse().unwrap());
    let mut config = EngineConfig::parse(&std::fs::read_to_string("engine.conf").unwrap()).unwrap();
    config.level_path = format!("levels/{level}");
    let floor = LevelFloor::load(
        Path::new(&config.level_path),
        config.level_floor_clearance_m,
    )
    .unwrap();
    let road = splat_engine::road::RoadProfile::for_level(&config.level_path);
    let point = |z: f32| {
        road.as_ref().and_then(|r| r.centre(z)).unwrap_or([
            floor.origin[0] + floor.dims[0] as f32 * floor.resolution * 0.5,
            0.0,
            floor.origin[2] + floor.dims[2] as f32 * floor.resolution * 0.5 + z,
        ])
    };
    let mut results = Vec::new();
    let mut csv = String::from("case,sample,ns,built,dropped,tiles,bodies,colliders\n");
    for case in ["stationary", "jitter", "road_trace", "teleports"] {
        let mut world = PhysicsWorld::new();
        let mut collision = LevelCollision::new(&floor, &config);
        collision.update(&mut world, point(-50.0));
        let mut times = Vec::with_capacity(count);
        let mut fingerprint = 14_695_981_039_346_656_037;
        let (mut builds, mut drops) = (0, 0);
        for i in 0..count {
            let p = match case {
                "stationary" => point(-50.0),
                "jitter" => {
                    let mut p = point(-50.0);
                    p[0] += (i as f32 * 0.031).sin() * 0.001;
                    p[2] += (i as f32 * 0.047).cos() * 0.001;
                    p
                }
                "road_trace" => {
                    point(-5.0 - 65.0 * (i as f32 / count as f32 * std::f32::consts::TAU).cos())
                }
                _ => point(if (i / 53) % 2 == 0 { -65.0 } else { 55.0 }),
            };
            let start = Instant::now();
            let (built, dropped) = collision.update(&mut world, p);
            let ns = start.elapsed().as_nanos() as f64;
            times.push(ns);
            builds += built;
            drops += dropped;
            for value in [
                built,
                dropped,
                collision.tiles(),
                world.bodies.len(),
                world.colliders.len(),
            ] {
                mix(&mut fingerprint, value as u64);
            }
            csv.push_str(&format!(
                "{case},{i},{ns},{built},{dropped},{},{},{}\n",
                collision.tiles(),
                world.bodies.len(),
                world.colliders.len()
            ));
        }
        let mean = times.iter().sum::<f64>() / count as f64;
        times.sort_by(f64::total_cmp);
        let mut geometry = 14_695_981_039_346_656_037;
        for (a, b, kind) in collision.debug_material_segments(&world, 4) {
            for v in a.into_iter().chain(b) {
                mix(&mut geometry, u64::from(v.to_bits()));
            }
            mix(&mut geometry, u64::from(kind));
        }
        results.push(serde_json::json!({"case":case,"samples":count,"mean_ns":mean,"p50_ns":times[count/2],"p95_ns":times[(count*95/100).min(count-1)],"built":builds,"dropped":drops,"trace_fingerprint":format!("{fingerprint:016x}"),"final_geometry_fingerprint":format!("{geometry:016x}"),"search_work":collision.search_profile(),"final_tiles":collision.tiles(),"bodies":world.bodies.len(),"colliders":world.colliders.len()}));
    }
    let report = serde_json::json!({"level":level,"scope":"CPU-only collision tile updates; excludes floor preprocessing, rendering and physics stepping","tile_m":config.level_collision_tile_m,"build_radius_m":config.level_collision_radius_m,"results":results});
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    std::fs::write(
        out.join("results.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    std::fs::write(out.join("samples.csv"), csv).unwrap();
}
