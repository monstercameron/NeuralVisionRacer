//! Vehicle machine (one car). Rows V1–V28 of `documents/state-machines.md`.
//!
//! Samples come from the physics world once per tick. The machine decides what the car is doing (driving,
//! airborne, crashed, wrecked, respawning) and returns effects for the driver to apply to the rigid body.

use crate::record::{Outcome, Reject};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleState {
    Unloaded,
    Loading,
    /// Body exists and is frozen at its spawn pose.
    Parked,
    /// At least one ground contact. `air_ticks` counts consecutive samples with none.
    Driving { air_ticks: u8 },
    Airborne,
    /// An impact between the crash and wreck thresholds. `hold` counts samples since the impact.
    Crashed { hold: u16 },
    Wrecked,
    /// Teleported to the checkpoint; `settle` counts samples until the car has stayed slow long enough.
    Respawning { settle: u16 },
}

/// What the physics world reported for one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsSample {
    /// Contact pairs with ground that are touching this tick. Becomes the wheel count with a raycast controller.
    pub ground_contacts: u8,
    /// Closing speed of the largest impact this tick (sum of contact impulses over mass), if any contact.
    pub chassis_impact_mps: Option<f32>,
    pub speed_mps: f32,
    pub in_bounds: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleEvent {
    Load,
    CarReady,
    CarFailed,
    /// `on_ground` is the last sample's contact state, so Parked → Driving or Airborne is decided here.
    Release { on_ground: bool },
    Park,
    Unload,
    Rebuild,
    Sample(SampleSummary),
    Respawn,
    Reset,
    Wreck,
}

/// A sample, reduced to what the machine uses. Keeps `VehicleEvent` `Copy` and `Eq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleSummary {
    pub ground_contacts: u8,
    /// Impact speed in milli-metres per second, or `None`. Integer so the event is `Eq`.
    pub impact_mmps: Option<u32>,
    /// Speed in milli-metres per second.
    pub speed_mmps: u32,
    pub in_bounds: bool,
}

impl From<PhysicsSample> for SampleSummary {
    fn from(s: PhysicsSample) -> Self {
        Self {
            ground_contacts: s.ground_contacts,
            impact_mmps: s.chassis_impact_mps.map(|v| (v.max(0.0) * 1000.0) as u32),
            speed_mmps: (s.speed_mps.max(0.0) * 1000.0) as u32,
            in_bounds: s.in_bounds,
        }
    }
}

impl VehicleEvent {
    pub fn tag(&self) -> &'static str {
        match self {
            VehicleEvent::Load => "Load",
            VehicleEvent::CarReady => "CarReady",
            VehicleEvent::CarFailed => "CarFailed",
            VehicleEvent::Release { .. } => "Release",
            VehicleEvent::Park => "Park",
            VehicleEvent::Unload => "Unload",
            VehicleEvent::Rebuild => "Rebuild",
            VehicleEvent::Sample(_) => "Sample",
            VehicleEvent::Respawn => "Respawn",
            VehicleEvent::Reset => "Reset",
            VehicleEvent::Wreck => "Wreck",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleNotice {
    CarFailed,
    Wrecked,
    OutOfBounds,
}

/// Work the driver performs on the rigid body and the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleEffect {
    /// Hold the body still (true) or let it move (false).
    Freeze(bool),
    /// Move the body to the checkpoint (the spawn pose for now) with zero velocity.
    Teleport,
    SpawnBody,
    DespawnBody,
    Notify(VehicleNotice),
}

/// Thresholds and counters. Placeholders until they are measured in the driving gate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehicleTuning {
    /// Samples without ground contact before Driving becomes Airborne (K_AIR).
    pub k_air: u8,
    /// Impact speed that enters Crashed (CRASH_MPS), in m/s.
    pub crash_mps: f32,
    /// Impact speed that enters Wrecked (WRECK_MPS), in m/s.
    pub wreck_mps: f32,
    /// Samples spent in Crashed before control returns (CRASH_HOLD).
    pub crash_hold: u16,
    /// Samples the car must stay slow before Respawning ends (SETTLE).
    pub settle: u16,
    /// Speed below which the car counts as settled, in m/s.
    pub settle_speed_mps: f32,
}

impl Default for VehicleTuning {
    fn default() -> Self {
        Self { k_air: 6, crash_mps: 8.0, wreck_mps: 15.0, crash_hold: 30, settle: 30, settle_speed_mps: 0.5 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VehicleTransition {
    pub next: VehicleState,
    pub effects: Vec<VehicleEffect>,
    pub outcome: Outcome,
}

/// Short state name for logs and status: `Driving(2)`, `Crashed(5)`, and so on.
pub fn state_tag(state: &VehicleState) -> String {
    match state {
        VehicleState::Unloaded => "Unloaded".into(),
        VehicleState::Loading => "Loading".into(),
        VehicleState::Parked => "Parked".into(),
        VehicleState::Driving { air_ticks } => format!("Driving({air_ticks})"),
        VehicleState::Airborne => "Airborne".into(),
        VehicleState::Crashed { hold } => format!("Crashed({hold})"),
        VehicleState::Wrecked => "Wrecked".into(),
        VehicleState::Respawning { settle } => format!("Respawning({settle})"),
    }
}

/// The name a state has in the tables (no counters), used to decide whether a step changed the tag.
fn kind(state: &VehicleState) -> u8 {
    match state {
        VehicleState::Unloaded => 0,
        VehicleState::Loading => 1,
        VehicleState::Parked => 2,
        VehicleState::Driving { .. } => 3,
        VehicleState::Airborne => 4,
        VehicleState::Crashed { .. } => 5,
        VehicleState::Wrecked => 6,
        VehicleState::Respawning { .. } => 7,
    }
}

/// One step of the vehicle machine.
pub fn step_vehicle(state: &VehicleState, event: VehicleEvent, tuning: &VehicleTuning) -> VehicleTransition {
    use VehicleEffect as F;
    use VehicleEvent as Ev;
    use VehicleNotice as N;
    use VehicleState::*;

    let stay = |effects: Vec<VehicleEffect>| VehicleTransition { next: *state, effects, outcome: Outcome::Ignored };
    let rejected = |reason: Reject| VehicleTransition { next: *state, effects: vec![], outcome: Outcome::Rejected(reason) };
    // A transition is `Applied` when the tag changes. Same tag with effects is `Unchanged`. Counter-only is `Unchanged`.
    let go = |next: VehicleState, effects: Vec<VehicleEffect>| {
        let outcome = if kind(&next) != kind(state) { Outcome::Applied } else { Outcome::Unchanged };
        VehicleTransition { next, effects, outcome }
    };
    let counter = |next: VehicleState| VehicleTransition { next, effects: vec![], outcome: Outcome::Unchanged };
    let teleport_to_checkpoint = || vec![F::Teleport];
    let is_live = matches!(state, Driving { .. } | Airborne | Crashed { .. } | Wrecked | Respawning { .. });

    match (state, event) {
        // V1–V3: loading the car.
        (Unloaded, Ev::Load) => go(Loading, vec![]),
        (Loading, Ev::CarReady) => go(Parked, vec![F::SpawnBody, F::Freeze(true)]),
        (Loading, Ev::CarFailed) => go(Unloaded, vec![F::Notify(N::CarFailed)]),

        // V4–V5: release from the frozen pose.
        (Parked, Ev::Release { on_ground: true }) => go(Driving { air_ticks: 0 }, vec![F::Freeze(false)]),
        (Parked, Ev::Release { on_ground: false }) => go(Airborne, vec![F::Freeze(false)]),

        // V10–V12: impacts. Impacts take precedence over everything else in these states.
        (Driving { .. } | Airborne | Crashed { .. }, Ev::Sample(s)) if impact_mps(&s) >= tuning.wreck_mps => {
            go(Wrecked, vec![F::Notify(N::Wrecked)])
        }
        (Driving { .. } | Airborne, Ev::Sample(s)) if impact_mps(&s) >= tuning.crash_mps => go(Crashed { hold: 0 }, vec![]),

        // V13–V15: the crash hold, then control returns.
        (Crashed { hold }, Ev::Sample(s)) if s.in_bounds => {
            let n = hold.saturating_add(1);
            if n < tuning.crash_hold {
                counter(Crashed { hold: n })
            } else if s.ground_contacts >= 1 {
                go(Driving { air_ticks: 0 }, vec![])
            } else {
                go(Airborne, vec![])
            }
        }

        // V16: leaving the play area respawns the car at the checkpoint.
        (Driving { .. } | Airborne | Crashed { .. }, Ev::Sample(s)) if !s.in_bounds => {
            let mut effects = vec![F::Notify(N::OutOfBounds)];
            effects.extend(teleport_to_checkpoint());
            go(Respawning { settle: 0 }, effects)
        }

        // V6–V9: contact and air tracking.
        (Driving { air_ticks }, Ev::Sample(s)) if s.ground_contacts == 0 => {
            let n = air_ticks.saturating_add(1);
            if n >= tuning.k_air {
                go(Airborne, vec![])
            } else {
                counter(Driving { air_ticks: n })
            }
        }
        (Driving { .. }, Ev::Sample(_)) => counter(Driving { air_ticks: 0 }),
        (Airborne, Ev::Sample(s)) if s.ground_contacts >= 1 => go(Driving { air_ticks: 0 }, vec![]),

        // V18–V20: settling after a respawn.
        (Respawning { settle }, Ev::Sample(s)) => {
            let slow = s.speed_mmps < (tuning.settle_speed_mps * 1000.0) as u32;
            let n = settle.saturating_add(1);
            if slow && n >= tuning.settle {
                if s.ground_contacts >= 1 {
                    go(Driving { air_ticks: 0 }, vec![])
                } else {
                    go(Airborne, vec![])
                }
            } else {
                counter(Respawning { settle: n })
            }
        }

        // V17: a wrecked car respawns on request.
        (Wrecked, Ev::Respawn | Ev::Reset) => {
            let mut effects = teleport_to_checkpoint();
            effects.push(F::Freeze(false));
            go(Respawning { settle: 0 }, effects)
        }

        // V22–V23, V25: resets.
        (Driving { .. } | Airborne | Crashed { .. }, Ev::Reset) => go(Respawning { settle: 0 }, teleport_to_checkpoint()),
        (Parked, Ev::Reset) => go(Parked, teleport_to_checkpoint()),
        (Respawning { .. }, Ev::Reset) => rejected(Reject::AlreadyRespawning),

        // V24: development wreck.
        (Driving { .. } | Airborne | Crashed { .. } | Parked, Ev::Wreck) => go(Wrecked, vec![F::Notify(N::Wrecked)]),

        // V21: park from any live state.
        (_, Ev::Park) if is_live => go(Parked, vec![F::Freeze(true)]),

        // V27: unload from any loaded state.
        (Parked | Driving { .. } | Airborne | Crashed { .. } | Wrecked | Respawning { .. }, Ev::Unload) => {
            go(Unloaded, vec![F::DespawnBody])
        }

        // V28: rebuild the body at the spawn pose, frozen until released.
        (Parked | Driving { .. } | Airborne | Crashed { .. } | Wrecked | Respawning { .. }, Ev::Rebuild) => {
            go(Parked, vec![F::DespawnBody, F::SpawnBody, F::Freeze(true), F::Teleport])
        }

        // V26: commands before the car exists.
        (Unloaded | Loading, Ev::Reset | Ev::Wreck | Ev::Respawn) => rejected(Reject::NotLoaded),

        _ => stay(vec![]),
    }
}

fn impact_mps(sample: &SampleSummary) -> f32 {
    sample.impact_mmps.map_or(0.0, |v| v as f32 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning() -> VehicleTuning {
        VehicleTuning::default()
    }

    fn sample(contacts: u8, impact_mps: Option<f32>, speed_mps: f32, in_bounds: bool) -> VehicleEvent {
        VehicleEvent::Sample(SampleSummary {
            ground_contacts: contacts,
            impact_mmps: impact_mps.map(|v| (v * 1000.0) as u32),
            speed_mmps: (speed_mps * 1000.0) as u32,
            in_bounds,
        })
    }

    fn air() -> VehicleEvent {
        sample(0, None, 5.0, true)
    }

    fn ground() -> VehicleEvent {
        sample(1, None, 5.0, true)
    }

    fn run(state: VehicleState, events: &[VehicleEvent]) -> VehicleState {
        events.iter().fold(state, |s, e| step_vehicle(&s, *e, &tuning()).next)
    }

    #[test]
    fn v1_to_v2_load_then_ready_parks_the_car_at_spawn() {
        let t = step_vehicle(&VehicleState::Unloaded, VehicleEvent::Load, &tuning());
        assert_eq!(t.next, VehicleState::Loading);
        let t = step_vehicle(&VehicleState::Loading, VehicleEvent::CarReady, &tuning());
        assert_eq!(t.next, VehicleState::Parked);
        assert_eq!(t.effects, vec![VehicleEffect::SpawnBody, VehicleEffect::Freeze(true)]);
    }

    #[test]
    fn v3_a_failed_load_returns_to_unloaded_and_tells_the_game() {
        let t = step_vehicle(&VehicleState::Loading, VehicleEvent::CarFailed, &tuning());
        assert_eq!(t.next, VehicleState::Unloaded);
        assert_eq!(t.effects, vec![VehicleEffect::Notify(VehicleNotice::CarFailed)]);
    }

    #[test]
    fn v4_v5_release_chooses_driving_or_airborne_from_the_last_contact() {
        let t = step_vehicle(&VehicleState::Parked, VehicleEvent::Release { on_ground: true }, &tuning());
        assert_eq!(t.next, VehicleState::Driving { air_ticks: 0 });
        let t = step_vehicle(&VehicleState::Parked, VehicleEvent::Release { on_ground: false }, &tuning());
        assert_eq!(t.next, VehicleState::Airborne);
    }

    #[test]
    fn v6_hysteresis_k_air_minus_one_stays_driving_and_k_air_becomes_airborne() {
        let t = tuning();
        let mut s = VehicleState::Driving { air_ticks: 0 };
        for _ in 0..(t.k_air - 1) {
            s = step_vehicle(&s, air(), &t).next;
        }
        assert!(matches!(s, VehicleState::Driving { .. }), "still driving after {} air samples: {s:?}", t.k_air - 1);
        s = step_vehicle(&s, air(), &t).next;
        assert_eq!(s, VehicleState::Airborne);
    }

    #[test]
    fn v8_a_contact_in_the_middle_resets_the_air_counter() {
        let s = run(VehicleState::Driving { air_ticks: 0 }, &[air(), air(), air(), ground(), air(), air()]);
        assert_eq!(s, VehicleState::Driving { air_ticks: 2 });
    }

    #[test]
    fn v9_contact_brings_an_airborne_car_back_to_driving() {
        assert_eq!(run(VehicleState::Airborne, &[ground()]), VehicleState::Driving { air_ticks: 0 });
    }

    #[test]
    fn v11_an_impact_exactly_at_the_crash_threshold_crashes_and_just_under_does_not() {
        let t = tuning();
        let at = step_vehicle(&VehicleState::Driving { air_ticks: 0 }, sample(1, Some(t.crash_mps), 4.0, true), &t);
        assert_eq!(at.next, VehicleState::Crashed { hold: 0 });
        let under = step_vehicle(&VehicleState::Driving { air_ticks: 0 }, sample(1, Some(t.crash_mps - 0.01), 4.0, true), &t);
        assert_eq!(under.next, VehicleState::Driving { air_ticks: 0 });
    }

    #[test]
    fn v10_an_impact_at_the_wreck_threshold_wrecks_and_notifies_the_game() {
        let t = tuning();
        let r = step_vehicle(&VehicleState::Driving { air_ticks: 0 }, sample(1, Some(t.wreck_mps), 0.0, true), &t);
        assert_eq!(r.next, VehicleState::Wrecked);
        assert_eq!(r.effects, vec![VehicleEffect::Notify(VehicleNotice::Wrecked)]);
    }

    #[test]
    fn v13_v14_crash_hold_then_control_returns_on_ground() {
        let t = tuning();
        let mut s = VehicleState::Crashed { hold: 0 };
        for _ in 0..(t.crash_hold - 1) {
            s = step_vehicle(&s, ground(), &t).next;
        }
        assert_eq!(s, VehicleState::Crashed { hold: t.crash_hold - 1 });
        s = step_vehicle(&s, ground(), &t).next;
        assert_eq!(s, VehicleState::Driving { air_ticks: 0 });
    }

    #[test]
    fn v15_crash_hold_ending_in_the_air_becomes_airborne() {
        let t = tuning();
        let s = step_vehicle(&VehicleState::Crashed { hold: t.crash_hold - 1 }, air(), &t).next;
        assert_eq!(s, VehicleState::Airborne);
    }

    #[test]
    fn v16_leaving_the_play_area_respawns_at_the_checkpoint() {
        let t = tuning();
        let r = step_vehicle(&VehicleState::Driving { air_ticks: 0 }, sample(1, None, 9.0, false), &t);
        assert_eq!(r.next, VehicleState::Respawning { settle: 0 });
        assert!(r.effects.contains(&VehicleEffect::Teleport));
        assert!(r.effects.contains(&VehicleEffect::Notify(VehicleNotice::OutOfBounds)));
    }

    #[test]
    fn v18_settle_needs_slow_speed_for_the_full_count() {
        let t = tuning();
        let slow = sample(1, None, 0.49, true);
        let fast = sample(1, None, 0.5, true);
        let mut s = VehicleState::Respawning { settle: 0 };
        for _ in 0..t.settle {
            s = step_vehicle(&s, slow, &t).next;
        }
        assert_eq!(s, VehicleState::Driving { air_ticks: 0 });

        // Speed exactly 0.5 does not count as settled, so the car stays respawning.
        let mut s = VehicleState::Respawning { settle: 0 };
        for _ in 0..t.settle {
            s = step_vehicle(&s, fast, &t).next;
        }
        assert!(matches!(s, VehicleState::Respawning { .. }));
    }

    #[test]
    fn v17_a_wrecked_car_respawns_on_request_or_reset() {
        let r = step_vehicle(&VehicleState::Wrecked, VehicleEvent::Respawn, &tuning());
        assert_eq!(r.next, VehicleState::Respawning { settle: 0 });
        assert!(r.effects.contains(&VehicleEffect::Teleport));
    }

    #[test]
    fn v22_v23_reset_while_driving_respawns_and_while_parked_stays_parked() {
        let r = step_vehicle(&VehicleState::Driving { air_ticks: 0 }, VehicleEvent::Reset, &tuning());
        assert_eq!(r.next, VehicleState::Respawning { settle: 0 });
        let r = step_vehicle(&VehicleState::Parked, VehicleEvent::Reset, &tuning());
        assert_eq!(r.next, VehicleState::Parked);
        assert_eq!(r.effects, vec![VehicleEffect::Teleport]);
    }

    #[test]
    fn v25_reset_during_a_respawn_is_refused() {
        let r = step_vehicle(&VehicleState::Respawning { settle: 3 }, VehicleEvent::Reset, &tuning());
        assert_eq!(r.outcome, Outcome::Rejected(Reject::AlreadyRespawning));
    }

    #[test]
    fn v21_park_freezes_any_live_state() {
        let r = step_vehicle(&VehicleState::Airborne, VehicleEvent::Park, &tuning());
        assert_eq!(r.next, VehicleState::Parked);
        assert_eq!(r.effects, vec![VehicleEffect::Freeze(true)]);
    }

    #[test]
    fn v26_commands_before_the_car_exists_are_refused() {
        let r = step_vehicle(&VehicleState::Loading, VehicleEvent::Reset, &tuning());
        assert_eq!(r.outcome, Outcome::Rejected(Reject::NotLoaded));
    }

    #[test]
    fn v28_rebuild_puts_the_car_back_at_spawn_frozen() {
        let r = step_vehicle(&VehicleState::Wrecked, VehicleEvent::Rebuild, &tuning());
        assert_eq!(r.next, VehicleState::Parked);
        assert!(r.effects.contains(&VehicleEffect::Freeze(true)));
        assert!(r.effects.contains(&VehicleEffect::Teleport));
    }

    #[test]
    fn samples_in_states_that_do_not_use_them_are_ignored() {
        let r = step_vehicle(&VehicleState::Parked, ground(), &tuning());
        assert_eq!(r.outcome, Outcome::Ignored);
        assert_eq!(r.next, VehicleState::Parked);
    }

    #[test]
    fn v7_and_v8_counter_steps_are_unchanged_with_no_effects() {
        let r = step_vehicle(&VehicleState::Driving { air_ticks: 0 }, air(), &tuning());
        assert_eq!(r.outcome, Outcome::Unchanged);
        assert!(r.effects.is_empty());
    }
}
