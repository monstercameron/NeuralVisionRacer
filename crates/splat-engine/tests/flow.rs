//! The driver runs the machines end to end on synthetic data, so the flow is tested without the car file.

use splat_engine::log::Level;
use splat_engine::{CarLoad, Command, EngineConfig, Engine, Logger};
use std::sync::Arc;

fn cube(side: usize, spacing: f32) -> Vec<[f32; 3]> {
    let mut points = Vec::new();
    for x in 0..side {
        for y in 0..side {
            for z in 0..side {
                points.push([x as f32 * spacing, y as f32 * spacing, z as f32 * spacing]);
            }
        }
    }
    points
}

fn engine(car: CarLoad, start: &str) -> Engine {
    let mut config = EngineConfig::default();
    config.game_start_mode = start.into();
    config.splat_rotate_x_degrees = 0.0;
    config.physics_voxel_size = 0.1;
    config.vehicle_spawn = [0.0, 0.5, 0.0];
    let log = Arc::new(Logger::open(None, Level::Trace, 500).unwrap());
    Engine::new(config, None, log, car, None)
}

fn run(engine: &mut Engine, command: Command) -> String {
    engine.execute(command).0.to_json_line()
}

#[test]
fn startup_reaches_practice_with_the_car_released() {
    let engine = engine(CarLoad::Loaded(cube(4, 0.1)), "practice");
    let status = engine.status_json();
    assert_eq!(status["game"]["mode"], "Practice");
    assert_eq!(status["assets"]["car.body"], "Ready");
    assert!(status["vehicle"]["state"].as_str().unwrap().starts_with("Airborne") || status["vehicle"]["state"].as_str().unwrap().starts_with("Driving"));
}

#[test]
fn a_car_that_fails_to_load_leaves_the_game_in_an_error_state() {
    let engine = engine(CarLoad::Failed("bad file".into()), "practice");
    assert_eq!(engine.status_json()["game"]["mode"], "Error(AssetFailed(car.body))");
    assert!(engine.status_json()["vehicle"].is_null());
}

#[test]
fn no_car_configured_still_reaches_the_menu() {
    let engine = engine(CarLoad::NotConfigured, "menu");
    assert_eq!(engine.status_json()["game"]["mode"], "Menu");
}

#[test]
fn pause_refuses_reset_and_step_runs_exactly_n_ticks() {
    let mut engine = engine(CarLoad::Loaded(cube(4, 0.1)), "practice");
    assert!(run(&mut engine, Command::Pause).contains("\"ok\":true"));
    let refused = run(&mut engine, Command::Reset);
    assert!(refused.contains("\"ok\":false") && refused.contains("rejected: Paused"), "{refused}");
    let before = engine.tick;
    engine.execute(Command::Step(7));
    assert_eq!(engine.tick, before + 7);
    assert!(engine.paused());
}

#[test]
fn a_wreck_respawns_after_the_timer_even_while_stepping() {
    let mut engine = engine(CarLoad::Loaded(cube(4, 0.1)), "practice");
    engine.execute(Command::Step(0));
    engine.execute(Command::Wreck);
    assert_eq!(engine.status_json()["vehicle"]["state"], "Wrecked");
    engine.execute(Command::Pause);
    engine.execute(Command::Step(200));
    let state = engine.status_json()["vehicle"]["state"].as_str().unwrap().to_string();
    assert!(!state.starts_with("Wrecked"), "still {state}");
}

#[test]
fn race_start_from_the_menu_counts_down_and_refuses_reset_meanwhile() {
    let mut engine = engine(CarLoad::Loaded(cube(4, 0.1)), "menu");
    run(&mut engine, Command::Mode("race".into()));
    assert_eq!(engine.status_json()["game"]["mode"], "Race(Countdown)");
    let refused = run(&mut engine, Command::Reset);
    assert!(refused.contains("rejected: WrongPhase"), "{refused}");
    engine.execute(Command::Step(0));
    for _ in 0..(3 * 60 + 5) {
        engine.tick();
    }
    assert_eq!(engine.status_json()["game"]["mode"], "Race(Running)");
}

#[test]
fn every_transition_is_logged_with_its_text() {
    let log = Arc::new(Logger::open(None, Level::Trace, 500).unwrap());
    let mut config = EngineConfig::default();
    config.game_start_mode = "practice".into();
    config.physics_voxel_size = 0.1;
    let _engine = Engine::new(config, None, log.clone(), CarLoad::Loaded(cube(4, 0.1)), None);
    let messages: Vec<String> = log.tail(500, Level::Trace).into_iter().map(|r| r.message).collect();
    assert!(messages.iter().any(|m| m == "Boot --BootDone--> Loading [applied]"), "{messages:#?}");
    assert!(messages.iter().any(|m| m.contains("Menu --UiStart--> Practice [applied]")), "{messages:#?}");
}

#[test]
fn the_scene_has_the_car_floor_and_camera_as_entities() {
    let mut engine = engine(CarLoad::Loaded(cube(4, 0.1)), "practice");
    engine.tick();
    let entities = run(&mut engine, Command::Entities);
    assert!(entities.contains("\"name\":\"car\"") && entities.contains("\"name\":\"camera\""), "{entities}");
    assert!(entities.contains("\"Vehicle\""), "the car entity should carry its vehicle component: {entities}");
}

/// The rotation of entity `id`, read from the engine's own `entity` reply.
fn rotation_of(engine: &mut Engine, id: &str) -> Vec<f64> {
    let reply: serde_json::Value = serde_json::from_str(&run(engine, Command::Inspect(id.into()))).expect("json");
    reply["data"]["transform"]["rotation_xyzw"].as_array().expect("rotation").iter().map(|v| v.as_f64().unwrap()).collect()
}

#[test]
fn an_attached_spin_turns_its_entity_each_tick_and_detach_stops_it() {
    let mut engine = engine(CarLoad::NotConfigured, "menu");
    let spawned: serde_json::Value = serde_json::from_str(&run(&mut engine, Command::SpawnMarker([0.0; 3]))).expect("json");
    let id = spawned["data"]["id"].as_str().expect("id").to_string();
    let attached = run(&mut engine, Command::Attach { id: id.clone(), behaviour: "spin".into(), args: vec![180.0] });
    assert!(attached.contains("\"ok\":true"), "{attached}");
    let start = rotation_of(&mut engine, &id);
    for _ in 0..60 {
        engine.tick();
    }
    let turned = rotation_of(&mut engine, &id);
    assert_ne!(start, turned, "a spin behaviour should turn the entity");
    run(&mut engine, Command::Detach(id.clone()));
    let stopped = rotation_of(&mut engine, &id);
    engine.tick();
    assert_eq!(stopped, rotation_of(&mut engine, &id), "detach stops the spin");
}
