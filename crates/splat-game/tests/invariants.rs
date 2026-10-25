//! Random event sequences (seeded, so failures reproduce) must never break the game machine's invariants.

use splat_game::game::{step_game, GameEvent, GameState, Mode, RacePhase, StartRequest};
use splat_game::record::Outcome;
use splat_game::vehicle::{step_vehicle, PhysicsSample, SampleSummary, VehicleEvent, VehicleState, VehicleTuning};

/// A small, dependency-free generator so the test stays reproducible.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn random_game_event(rng: &mut Lcg) -> GameEvent {
    let asset = if rng.next() % 2 == 0 { "car.body" } else { "track.main" };
    match rng.next() % 26 {
        0 => GameEvent::BootDone,
        1 => GameEvent::AssetsChanged { all_ready: rng.next() % 2 == 0 },
        2 => GameEvent::AssetFailed(asset.into()),
        3 => GameEvent::Fault((rng.next() % 5) as u16),
        4 => GameEvent::UiStart(StartRequest::Practice),
        5 => GameEvent::UiStart(StartRequest::Race { track: "main".into() }),
        6 => GameEvent::UiPause,
        7 => GameEvent::UiResume,
        8 => GameEvent::UiQuit,
        9 => GameEvent::UiRetry(asset.into()),
        10 => GameEvent::UiRestart,
        11 => GameEvent::StepRequested((rng.next() % 4) as u32),
        12 => GameEvent::Reload,
        13 => GameEvent::Reset,
        14 => GameEvent::Wreck,
        15 => GameEvent::CountdownElapsed,
        16 => GameEvent::RaceFinished { time_ms: rng.next() % 100_000 },
        17 => GameEvent::ResultsDue,
        18 => GameEvent::VehicleWrecked,
        19 => GameEvent::WreckTimerElapsed,
        _ => match rng.next() % 3 {
            0 => GameEvent::UiPause,
            1 => GameEvent::UiResume,
            _ => GameEvent::UiStart(StartRequest::Practice),
        },
    }
}

#[test]
fn game_machine_invariants_hold_over_random_sequences() {
    let mut rng = Lcg(0x5EED_CAFE);
    for sequence in 0..10_000u32 {
        let mut state = GameState::default();
        for step in 0..40 {
            let event = random_game_event(&mut rng);
            let before = state.clone();
            let t = step_game(&state, event.clone());
            let after = t.next.clone();

            // I1: practice and race only run with the assets ready.
            if matches!(after.mode, Mode::Practice | Mode::Race(_)) {
                assert!(after.assets_ready, "seq {sequence} step {step}: running without assets after {event:?}");
            }
            // I2: results are entered only from a finished race.
            if after.mode == Mode::Results && before.mode != Mode::Results {
                assert_eq!(before.mode, Mode::Race(RacePhase::Finished), "seq {sequence} step {step}");
            }
            // I3: the engine gate is exactly the paused mode.
            assert_eq!(after.engine_paused(), matches!(after.mode, Mode::Paused(_)));
            // A refused event changes nothing.
            if matches!(t.outcome, Outcome::Rejected(_)) {
                assert_eq!(after, before, "seq {sequence} step {step}: rejected {event:?} changed state");
            }
            state = after;
        }
    }
}

#[test]
fn vehicle_machine_never_leaves_a_loaded_car_without_a_state() {
    let tuning = VehicleTuning::default();
    let mut rng = Lcg(0xBEEF);
    for _ in 0..2_000 {
        let mut state = VehicleState::Unloaded;
        for _ in 0..60 {
            let event = match rng.next() % 9 {
                0 => VehicleEvent::Load,
                1 => VehicleEvent::CarReady,
                2 => VehicleEvent::Release { on_ground: rng.next() % 2 == 0 },
                3 => VehicleEvent::Park,
                4 => VehicleEvent::Reset,
                5 => VehicleEvent::Wreck,
                6 => VehicleEvent::Rebuild,
                7 => VehicleEvent::Unload,
                _ => VehicleEvent::Sample(SampleSummary::from(PhysicsSample {
                    ground_contacts: (rng.next() % 3) as u8,
                    chassis_impact_mps: if rng.next() % 4 == 0 { Some((rng.next() % 30) as f32) } else { None },
                    speed_mps: (rng.next() % 20) as f32 * 0.1,
                    in_bounds: rng.next() % 10 != 0,
                })),
            };
            let t = step_vehicle(&state, event, &tuning);
            // Counters stay bounded: a counter that overflowed would wrap the state silently.
            if let VehicleState::Driving { air_ticks } = t.next {
                assert!(air_ticks <= tuning.k_air);
            }
            state = t.next;
        }
    }
}
