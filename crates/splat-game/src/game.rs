//! Game-mode machine (session level). Rows G1–G34 of `documents/state-machines.md`.
//!
//! The machine never touches the vehicle or the engine. It returns effects, and the driver turns them into
//! vehicle events, timers and engine changes. `Paused` is the single source for "the engine must not tick".

use crate::record::{Outcome, Reject};

/// Mode of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Boot,
    Loading,
    Menu,
    Practice,
    Race(RacePhase),
    Paused(ResumePoint),
    Results,
    Error(ErrorKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RacePhase {
    Countdown,
    Running,
    Finished,
}

/// Where a pause came from, so resume returns there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePoint {
    Practice,
    RaceCountdown,
    RaceRunning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    AssetFailed(String),
    Fault(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartRequest {
    Practice,
    Race { track: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameState {
    pub mode: Mode,
    /// Mirrors the asset registry's `all_ready`. The driver keeps them equal.
    pub assets_ready: bool,
}

impl Default for GameState {
    fn default() -> Self {
        Self { mode: Mode::Boot, assets_ready: false }
    }
}

impl GameState {
    /// The engine must not tick while the game is paused. Derived, so nothing else can set it.
    pub fn engine_paused(&self) -> bool {
        matches!(self.mode, Mode::Paused(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    BootDone,
    AssetsChanged { all_ready: bool },
    AssetFailed(String),
    Fault(u16),
    UiStart(StartRequest),
    UiPause,
    UiResume,
    UiQuit,
    UiRetry(String),
    UiRestart,
    StepRequested(u32),
    Reload,
    /// The console `reset` verb, or `Command(Reset)` in the design.
    Reset,
    /// Development-only wreck, `Command(Wreck)`.
    Wreck,
    CountdownElapsed,
    RaceFinished { time_ms: u64 },
    ResultsDue,
    VehicleWrecked,
    WreckTimerElapsed,
}

impl GameEvent {
    pub fn tag(&self) -> &'static str {
        match self {
            GameEvent::BootDone => "BootDone",
            GameEvent::AssetsChanged { .. } => "AssetsChanged",
            GameEvent::AssetFailed(_) => "AssetFailed",
            GameEvent::Fault(_) => "Fault",
            GameEvent::UiStart(_) => "UiStart",
            GameEvent::UiPause => "UiPause",
            GameEvent::UiResume => "UiResume",
            GameEvent::UiQuit => "UiQuit",
            GameEvent::UiRetry(_) => "UiRetry",
            GameEvent::UiRestart => "UiRestart",
            GameEvent::StepRequested(_) => "StepRequested",
            GameEvent::Reload => "Reload",
            GameEvent::Reset => "Reset",
            GameEvent::Wreck => "Wreck",
            GameEvent::CountdownElapsed => "CountdownElapsed",
            GameEvent::RaceFinished { .. } => "RaceFinished",
            GameEvent::ResultsDue => "ResultsDue",
            GameEvent::VehicleWrecked => "VehicleWrecked",
            GameEvent::WreckTimerElapsed => "WreckTimerElapsed",
        }
    }
}

/// Work the driver performs after a game transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEffect {
    /// Start (or restart) loading the assets; the driver feeds the asset machines.
    RequestAssets,
    /// Release every asset the game holds (used on fatal faults).
    ReleaseAssets,
    VehicleRelease,
    VehiclePark,
    VehicleUnload,
    VehicleRebuild,
    VehicleReset,
    VehicleWreck,
    VehicleRespawn,
    StartCountdown,
    CancelCountdown,
    StartWreckTimer,
    CancelWreckTimers,
    /// Run `n` engine ticks with the pause gate open, then stop again.
    StepEngine(u32),
    RecordRaceTime(u64),
    /// Leave the process. Only the console `quit` and the error screen produce it.
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameTransition {
    pub next: GameState,
    pub effects: Vec<GameEffect>,
    pub outcome: Outcome,
}

/// Mode tag for logs and status. Parameters are included so the log shows the whole state.
pub fn mode_tag(mode: &Mode) -> String {
    match mode {
        Mode::Boot => "Boot".into(),
        Mode::Loading => "Loading".into(),
        Mode::Menu => "Menu".into(),
        Mode::Practice => "Practice".into(),
        Mode::Race(phase) => format!("Race({})", match phase {
            RacePhase::Countdown => "Countdown",
            RacePhase::Running => "Running",
            RacePhase::Finished => "Finished",
        }),
        Mode::Paused(point) => format!("Paused({})", match point {
            ResumePoint::Practice => "Practice",
            ResumePoint::RaceCountdown => "RaceCountdown",
            ResumePoint::RaceRunning => "RaceRunning",
        }),
        Mode::Results => "Results".into(),
        Mode::Error(ErrorKind::AssetFailed(id)) => format!("Error(AssetFailed({id}))"),
        Mode::Error(ErrorKind::Fault(code)) => format!("Error(Fault({code}))"),
    }
}

/// One step of the game machine.
pub fn step_game(state: &GameState, event: GameEvent) -> GameTransition {
    let mut next = state.clone();
    let rejected = |reason: Reject| GameTransition { next: state.clone(), effects: vec![], outcome: Outcome::Rejected(reason) };
    let ignored = GameTransition { next: state.clone(), effects: vec![], outcome: Outcome::Ignored };
    let to = |mode: Mode, effects: Vec<GameEffect>| {
        let changed = mode_tag(&mode) != mode_tag(&state.mode);
        let mut n = state.clone();
        n.mode = mode;
        GameTransition { next: n, effects, outcome: if changed { Outcome::Applied } else { Outcome::Unchanged } }
    };
    let race_start = |track: &str| {
        let _ = track; // the track id is recorded by the driver; there is only one track today
        vec![GameEffect::VehicleReset, GameEffect::VehiclePark, GameEffect::StartCountdown]
    };

    use GameEffect as E;
    use GameEvent as Ev;
    use Mode::*;
    use RacePhase::*;
    use ResumePoint as R;

    match (&state.mode, event) {
        // G1–G4: boot and loading.
        (Boot, Ev::BootDone) => to(Loading, vec![E::RequestAssets]),
        (Boot, Ev::Fault(code)) => to(Error(ErrorKind::Fault(code)), vec![E::ReleaseAssets]),
        (Loading, Ev::AssetsChanged { all_ready: true }) => {
            next.assets_ready = true;
            let mut t = to(Menu, vec![]);
            t.next.assets_ready = true;
            t
        }
        (Loading, Ev::AssetFailed(id)) => to(Error(ErrorKind::AssetFailed(id)), vec![E::ReleaseAssets]),

        // G5–G6, G23, G34: starting a session.
        (Menu, Ev::UiStart(StartRequest::Practice)) if state.assets_ready => to(Practice, vec![E::VehicleRelease]),
        (Menu, Ev::UiStart(StartRequest::Race { track })) if state.assets_ready => to(Race(Countdown), race_start(&track)),
        (Menu, Ev::UiStart(_)) => rejected(Reject::AssetsNotReady),
        (Results, Ev::UiRestart) if state.assets_ready => to(Race(Countdown), race_start("main")),
        (Results, Ev::UiRestart) => rejected(Reject::AssetsNotReady),
        (_, Ev::UiStart(_)) => rejected(Reject::WrongPhase),

        // G7–G9: practice.
        (Practice, Ev::UiPause) => to(Paused(R::Practice), vec![]),
        (Practice, Ev::UiQuit) => to(Menu, vec![E::VehiclePark]),
        (Practice, Ev::Reset) => to(Practice, vec![E::VehicleReset]),

        // G10–G13: countdown.
        (Race(Countdown), Ev::CountdownElapsed) => to(Race(Running), vec![E::VehicleRelease]),
        (Race(Countdown), Ev::UiPause) => to(Paused(R::RaceCountdown), vec![]),
        (Race(Countdown), Ev::UiQuit) => to(Menu, vec![E::VehiclePark, E::CancelCountdown]),
        (Race(Countdown), Ev::Reset) => rejected(Reject::WrongPhase),

        // G14–G18: running race.
        (Race(Running), Ev::UiPause) => to(Paused(R::RaceRunning), vec![]),
        (Race(Running), Ev::UiQuit) => to(Menu, vec![E::VehiclePark]),
        (Race(Running), Ev::RaceFinished { time_ms }) => {
            to(Race(Finished), vec![E::VehiclePark, E::RecordRaceTime(time_ms)])
        }
        (Race(Running), Ev::Reset) => to(Race(Running), vec![E::VehicleReset]),

        // G18 and its timer: a wreck starts a timer, and the timer respawns the car. Practice follows the same rule.
        (Practice | Race(Running), Ev::VehicleWrecked) => to(state.mode.clone(), vec![E::StartWreckTimer]),
        (Practice | Race(Running), Ev::WreckTimerElapsed) => to(state.mode.clone(), vec![E::VehicleRespawn]),
        (Practice | Race(Running), Ev::Wreck) => to(state.mode.clone(), vec![E::VehicleWreck]),

        // G19: finished race goes to results.
        (Race(Finished), Ev::ResultsDue) => to(Results, vec![]),

        // Timers keep running while the pause gate is open for `step n`. A real pause never fires them,
        // because the engine does not tick.
        (Paused(_), Ev::WreckTimerElapsed) => to(state.mode.clone(), vec![E::VehicleRespawn]),
        (Paused(R::RaceCountdown), Ev::CountdownElapsed) => to(Paused(R::RaceRunning), vec![E::VehicleRelease]),

        // G20–G22: pause and resume.
        (Paused(point), Ev::UiResume) => {
            let mode = match point {
                R::Practice => Practice,
                R::RaceCountdown => Race(Countdown),
                R::RaceRunning => Race(Running),
            };
            to(mode, vec![])
        }
        (Paused(_), Ev::UiQuit) => to(Menu, vec![E::VehiclePark, E::CancelWreckTimers]),
        (Paused(_), Ev::Reset) => rejected(Reject::Paused),
        (Paused(_), Ev::StepRequested(n)) => to(state.mode.clone(), vec![E::StepEngine(n)]),
        (_, Ev::StepRequested(_)) => rejected(Reject::NotPaused),

        // G24: leaving results.
        (Results, Ev::UiQuit) => to(Menu, vec![]),

        // G25–G28: assets lost, faults, retries and exit.
        (Menu | Practice | Race(_) | Paused(_) | Results, Ev::AssetsChanged { all_ready: false }) => {
            next.assets_ready = false;
            let mut t = to(Loading, vec![E::VehicleUnload]);
            t.next.assets_ready = false;
            t
        }
        (Menu | Practice | Race(_) | Paused(_) | Results | Loading, Ev::Fault(code)) => {
            to(Error(ErrorKind::Fault(code)), vec![E::ReleaseAssets])
        }
        (Error(ErrorKind::AssetFailed(id)), Ev::UiRetry(ref retry)) if id == retry => to(Loading, vec![E::RequestAssets]),
        (Error(_), Ev::UiQuit) => to(state.mode.clone(), vec![E::Exit]),

        // G30–G33: reload and stepping.
        (Menu | Results, Ev::Reload) => to(state.mode.clone(), vec![E::VehicleRebuild]),
        (Practice, Ev::Reload) => to(Practice, vec![E::VehicleRebuild, E::VehicleRelease]),
        (Race(_) | Paused(_), Ev::Reload) => rejected(if state.engine_paused() { Reject::Paused } else { Reject::InRace }),

        _ => ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(mode: Mode, assets_ready: bool) -> GameState {
        GameState { mode, assets_ready }
    }

    fn step(mode: Mode, assets_ready: bool, event: GameEvent) -> GameTransition {
        step_game(&state(mode, assets_ready), event)
    }

    #[test]
    fn boot_loads_then_the_menu_opens_when_assets_are_ready() {
        let t = step(Mode::Boot, false, GameEvent::BootDone);
        assert_eq!(t.next.mode, Mode::Loading);
        assert_eq!(t.effects, vec![GameEffect::RequestAssets]);
        let t = step(Mode::Loading, false, GameEvent::AssetsChanged { all_ready: true });
        assert_eq!(t.next.mode, Mode::Menu);
        assert!(t.next.assets_ready);
    }

    #[test]
    fn g4_a_failed_asset_is_an_error_naming_the_asset() {
        let t = step(Mode::Loading, false, GameEvent::AssetFailed("car.body".into()));
        assert_eq!(t.next.mode, Mode::Error(ErrorKind::AssetFailed("car.body".into())));
        assert_eq!(t.effects, vec![GameEffect::ReleaseAssets]);
    }

    #[test]
    fn g5_practice_needs_ready_assets_and_releases_the_car() {
        let ok = step(Mode::Menu, true, GameEvent::UiStart(StartRequest::Practice));
        assert_eq!((ok.next.mode, ok.outcome), (Mode::Practice, Outcome::Applied));
        assert_eq!(ok.effects, vec![GameEffect::VehicleRelease]);
        let refused = step(Mode::Menu, false, GameEvent::UiStart(StartRequest::Practice));
        assert_eq!(refused.outcome, Outcome::Rejected(Reject::AssetsNotReady));
        assert_eq!(refused.next.mode, Mode::Menu);
    }

    #[test]
    fn g6_race_starts_at_the_grid_with_a_countdown() {
        let t = step(Mode::Menu, true, GameEvent::UiStart(StartRequest::Race { track: "main".into() }));
        assert_eq!(t.next.mode, Mode::Race(RacePhase::Countdown));
        assert!(t.effects.contains(&GameEffect::StartCountdown));
        assert!(t.effects.contains(&GameEffect::VehiclePark));
    }

    #[test]
    fn g7_and_g20_pause_then_resume_to_the_same_mode() {
        let paused = step(Mode::Practice, true, GameEvent::UiPause);
        assert_eq!(paused.next.mode, Mode::Paused(ResumePoint::Practice));
        assert!(paused.next.engine_paused());
        let resumed = step(paused.next.mode.clone(), true, GameEvent::UiResume);
        assert_eq!(resumed.next.mode, Mode::Practice);
        assert!(!resumed.next.engine_paused());
    }

    #[test]
    fn g13_g22_reset_is_refused_while_counting_down_or_paused() {
        let t = step(Mode::Race(RacePhase::Countdown), true, GameEvent::Reset);
        assert_eq!(t.outcome, Outcome::Rejected(Reject::WrongPhase));
        let t = step(Mode::Paused(ResumePoint::Practice), true, GameEvent::Reset);
        assert_eq!(t.outcome, Outcome::Rejected(Reject::Paused));
    }

    #[test]
    fn g17_reset_during_a_running_race_is_unchanged_but_still_acts() {
        let t = step(Mode::Race(RacePhase::Running), true, GameEvent::Reset);
        assert_eq!(t.outcome, Outcome::Unchanged);
        assert_eq!(t.effects, vec![GameEffect::VehicleReset]);
    }

    #[test]
    fn g10_countdown_releases_the_car_into_the_race() {
        let t = step(Mode::Race(RacePhase::Countdown), true, GameEvent::CountdownElapsed);
        assert_eq!(t.next.mode, Mode::Race(RacePhase::Running));
        assert_eq!(t.effects, vec![GameEffect::VehicleRelease]);
    }

    #[test]
    fn g18_a_wreck_starts_the_timer_and_the_timer_respawns() {
        let t = step(Mode::Practice, true, GameEvent::VehicleWrecked);
        assert_eq!(t.effects, vec![GameEffect::StartWreckTimer]);
        assert_eq!(t.outcome, Outcome::Unchanged);
        let t = step(Mode::Practice, true, GameEvent::WreckTimerElapsed);
        assert_eq!(t.effects, vec![GameEffect::VehicleRespawn]);
    }

    #[test]
    fn g30_g33_stepping_is_only_allowed_while_paused() {
        let t = step(Mode::Paused(ResumePoint::Practice), true, GameEvent::StepRequested(3));
        assert_eq!(t.effects, vec![GameEffect::StepEngine(3)]);
        assert_eq!(t.next.mode, Mode::Paused(ResumePoint::Practice));
        let t = step(Mode::Practice, true, GameEvent::StepRequested(3));
        assert_eq!(t.outcome, Outcome::Rejected(Reject::NotPaused));
    }

    #[test]
    fn g32_reload_is_refused_in_a_race_and_allowed_in_practice() {
        let t = step(Mode::Race(RacePhase::Running), true, GameEvent::Reload);
        assert_eq!(t.outcome, Outcome::Rejected(Reject::InRace));
        let t = step(Mode::Practice, true, GameEvent::Reload);
        assert!(t.effects.contains(&GameEffect::VehicleRebuild));
        assert!(t.effects.contains(&GameEffect::VehicleRelease));
    }

    #[test]
    fn g25_losing_assets_sends_the_game_back_to_loading() {
        let t = step(Mode::Practice, true, GameEvent::AssetsChanged { all_ready: false });
        assert_eq!(t.next.mode, Mode::Loading);
        assert!(!t.next.assets_ready);
        assert!(t.effects.contains(&GameEffect::VehicleUnload));
    }

    #[test]
    fn g27_retry_only_for_the_asset_that_failed() {
        let error = Mode::Error(ErrorKind::AssetFailed("car.body".into()));
        let t = step(error.clone(), false, GameEvent::UiRetry("track.main".into()));
        assert_eq!(t.outcome, Outcome::Ignored);
        let t = step(error, false, GameEvent::UiRetry("car.body".into()));
        assert_eq!(t.next.mode, Mode::Loading);
        assert_eq!(t.effects, vec![GameEffect::RequestAssets]);
    }

    #[test]
    fn g34_starting_a_session_outside_the_menu_is_refused() {
        let t = step(Mode::Practice, true, GameEvent::UiStart(StartRequest::Practice));
        assert_eq!(t.outcome, Outcome::Rejected(Reject::WrongPhase));
    }

    #[test]
    fn g29_unlisted_pairs_are_ignored() {
        let t = step(Mode::Menu, true, GameEvent::CountdownElapsed);
        assert_eq!(t.outcome, Outcome::Ignored);
        assert_eq!(t.next, state(Mode::Menu, true));
    }

    #[test]
    fn g19_and_g23_results_flow() {
        let t = step(Mode::Race(RacePhase::Finished), true, GameEvent::ResultsDue);
        assert_eq!(t.next.mode, Mode::Results);
        let t = step(Mode::Results, true, GameEvent::UiRestart);
        assert_eq!(t.next.mode, Mode::Race(RacePhase::Countdown));
        let t = step(Mode::Results, false, GameEvent::UiRestart);
        assert_eq!(t.outcome, Outcome::Rejected(Reject::AssetsNotReady));
    }

    #[test]
    fn stepping_while_paused_still_fires_the_wreck_timer() {
        let t = step(Mode::Paused(ResumePoint::Practice), true, GameEvent::WreckTimerElapsed);
        assert_eq!(t.effects, vec![GameEffect::VehicleRespawn]);
    }

    #[test]
    fn g26_a_fault_stops_any_running_mode() {
        let t = step(Mode::Race(RacePhase::Running), true, GameEvent::Fault(7));
        assert_eq!(t.next.mode, Mode::Error(ErrorKind::Fault(7)));
    }
}
