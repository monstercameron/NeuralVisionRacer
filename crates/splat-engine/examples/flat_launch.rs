//! Full-throttle launch of a car-sized box on flat ground, printing each wheel's suspension every 0.05 s.
use splat_car::Controls;
use splat_engine::log::Level;
use splat_engine::{CarLoad, EngineConfig, Engine, Logger};
use std::sync::Arc;

fn main() {
    let mut points = Vec::new();
    for i in 0..=19 {
        for j in 0..=13 {
            for k in 0..=45 {
                points.push([i as f32 * 0.1, j as f32 * 0.1, k as f32 * 0.1016]);
            }
        }
    }
    let mut config = EngineConfig::default();
    config.splat_rotate_x_degrees = 0.0;
    config.vehicle_spawn = [0.0, 0.7, 0.0];
    let log = Arc::new(Logger::open(None, Level::Warn, 10).unwrap());
    let mut engine = Engine::new(config, None, log, CarLoad::Loaded(points), None);
    for _ in 0..120 {
        engine.tick();
    }
    engine.set_controls(Controls { throttle: 1.0, ..Default::default() });
    for i in 0..=30 {
        let w: String = engine.physics.wheel_states().iter().map(|(on, len)| if *on { format!("{len:.3} ") } else { "---- ".into() }).collect();
        println!("{:.2} s  [{w}] {} km/h", i as f32 / 20.0, engine.status_json()["car"]["speed_kmh"]);
        for _ in 0..3 {
            engine.tick();
        }
    }
}
