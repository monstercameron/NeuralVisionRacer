//! Asset machine (one loadable model part). Rows A1–A15 of `documents/state-machines.md`, without the GPU upload stage.
//!
//! The engine's machine stops at `Ready` after the collider is built. Uploading the splat to the GPU is the
//! renderer's job and is not a machine stage, so headless runs (no GPU) can still reach `Ready`.

use crate::record::Outcome;

/// Attempts before a retryable fetch error becomes terminal (A3).
pub const MAX_FETCH_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetStage {
    Idle,
    Fetching { attempt: u8 },
    Decoding,
    ColliderBuilding,
    Ready,
    Failed(AssetFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetFailure {
    Fetch(FetchError),
    Decode(DecodeError),
    Collider(ColliderError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    NotFound,
    /// Transient; retried up to `MAX_FETCH_ATTEMPTS`.
    Network,
}

impl FetchError {
    pub fn retryable(&self) -> bool {
        matches!(self, FetchError::Network)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Deterministic: the same bytes fail the same way, so it is never retried.
    Corrupt(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColliderError {
    /// The splat had no cells to voxelise.
    Empty,
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetEvent {
    Request,
    Fetched(Result<(), FetchError>),
    Decoded(Result<(), DecodeError>),
    ColliderBuilt(Result<(), ColliderError>),
    Release,
}

impl AssetEvent {
    pub fn tag(&self) -> &'static str {
        match self {
            AssetEvent::Request => "Request",
            AssetEvent::Fetched(_) => "Fetched",
            AssetEvent::Decoded(_) => "Decoded",
            AssetEvent::ColliderBuilt(_) => "ColliderBuilt",
            AssetEvent::Release => "Release",
        }
    }
}

/// What the driver must do for the asset after a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetEffect {
    StartFetch,
    StartDecode,
    StartCollider,
    /// Tell the game the asset is ready (or failed, with the reason).
    NotifyReady,
    NotifyFailed(AssetFailure),
    /// Free the payload and any collider built for the asset.
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetTransition {
    pub next: AssetStage,
    pub effects: Vec<AssetEffect>,
    pub outcome: Outcome,
}

pub fn stage_tag(stage: &AssetStage) -> String {
    match stage {
        AssetStage::Idle => "Idle".into(),
        AssetStage::Fetching { attempt } => format!("Fetching({attempt})"),
        AssetStage::Decoding => "Decoding".into(),
        AssetStage::ColliderBuilding => "ColliderBuilding".into(),
        AssetStage::Ready => "Ready".into(),
        AssetStage::Failed(failure) => format!("Failed({})", failure_tag(failure)),
    }
}

fn failure_tag(failure: &AssetFailure) -> &'static str {
    match failure {
        AssetFailure::Fetch(FetchError::NotFound) => "Fetch(NotFound)",
        AssetFailure::Fetch(FetchError::Network) => "Fetch(Network)",
        AssetFailure::Decode(_) => "Decode",
        AssetFailure::Collider(_) => "Collider",
    }
}

/// One step of the asset machine. `has_collider` is true for parts that need physics (the car body).
pub fn step_asset(stage: &AssetStage, event: AssetEvent, has_collider: bool) -> AssetTransition {
    use AssetEffect as F;
    use AssetEvent as Ev;
    use AssetStage::*;

    let to = |next: AssetStage, effects: Vec<AssetEffect>| {
        let outcome = if stage_tag(&next) != stage_tag(stage) { Outcome::Applied } else { Outcome::Unchanged };
        AssetTransition { next, effects, outcome }
    };
    let ignored = AssetTransition { next: stage.clone(), effects: vec![], outcome: Outcome::Ignored };

    match (stage, event) {
        // A1: a request starts a fetch from Idle or after an explicit retry (A14).
        (Idle | Failed(_), Ev::Request) => to(Fetching { attempt: 1 }, vec![F::StartFetch]),
        // A15: requesting a loaded asset changes nothing.
        (Ready, Ev::Request) => AssetTransition { next: Ready, effects: vec![], outcome: Outcome::Unchanged },

        // A2: fetched bytes go to the decoder.
        (Fetching { .. }, Ev::Fetched(Ok(()))) => to(Decoding, vec![F::StartDecode]),
        // A3: a retryable error below the attempt limit fetches again.
        (Fetching { attempt }, Ev::Fetched(Err(ref e))) if e.retryable() && *attempt < MAX_FETCH_ATTEMPTS => {
            to(Fetching { attempt: attempt + 1 }, vec![F::StartFetch])
        }
        // A4: anything else is terminal.
        (Fetching { .. }, Ev::Fetched(Err(e))) => {
            let failure = AssetFailure::Fetch(e);
            to(Failed(failure.clone()), vec![F::NotifyFailed(failure)])
        }

        // A5–A6: decoding.
        (Decoding, Ev::Decoded(Ok(()))) if has_collider => to(ColliderBuilding, vec![F::StartCollider]),
        (Decoding, Ev::Decoded(Ok(()))) => to(Ready, vec![F::NotifyReady]),
        (Decoding, Ev::Decoded(Err(e))) => {
            let failure = AssetFailure::Decode(e);
            to(Failed(failure.clone()), vec![F::NotifyFailed(failure)])
        }

        // A10–A11: the collider.
        (ColliderBuilding, Ev::ColliderBuilt(Ok(()))) => to(Ready, vec![F::NotifyReady]),
        (ColliderBuilding, Ev::ColliderBuilt(Err(e))) => {
            let failure = AssetFailure::Collider(e);
            to(Failed(failure.clone()), vec![F::NotifyFailed(failure)])
        }

        // A13: releasing a loaded asset drops its payload and collider.
        (Ready | Failed(_), Ev::Release) => to(Idle, vec![F::Release]),

        _ => ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(events: Vec<AssetEvent>, has_collider: bool) -> AssetStage {
        events.into_iter().fold(AssetStage::Idle, |s, e| step_asset(&s, e, has_collider).next)
    }

    #[test]
    fn a1_to_a10_the_car_goes_idle_to_ready_through_its_collider() {
        let stage = run(
            vec![AssetEvent::Request, AssetEvent::Fetched(Ok(())), AssetEvent::Decoded(Ok(())), AssetEvent::ColliderBuilt(Ok(()))],
            true,
        );
        assert_eq!(stage, AssetStage::Ready);
    }

    #[test]
    fn a8_a_model_without_a_collider_is_ready_after_decode() {
        let stage = run(vec![AssetEvent::Request, AssetEvent::Fetched(Ok(())), AssetEvent::Decoded(Ok(()))], false);
        assert_eq!(stage, AssetStage::Ready);
    }

    #[test]
    fn a3_network_errors_retry_up_to_the_limit_then_fail() {
        let mut stage = step_asset(&AssetStage::Idle, AssetEvent::Request, true).next;
        for attempt in 1..MAX_FETCH_ATTEMPTS {
            let t = step_asset(&stage, AssetEvent::Fetched(Err(FetchError::Network)), true);
            assert_eq!(t.next, AssetStage::Fetching { attempt: attempt + 1 });
            assert_eq!(t.effects, vec![AssetEffect::StartFetch]);
            stage = t.next;
        }
        let t = step_asset(&stage, AssetEvent::Fetched(Err(FetchError::Network)), true);
        assert!(matches!(t.next, AssetStage::Failed(AssetFailure::Fetch(FetchError::Network))));
    }

    #[test]
    fn a4_not_found_fails_on_the_first_attempt() {
        let t = step_asset(&AssetStage::Fetching { attempt: 1 }, AssetEvent::Fetched(Err(FetchError::NotFound)), true);
        assert!(matches!(t.next, AssetStage::Failed(AssetFailure::Fetch(FetchError::NotFound))));
        assert_eq!(t.outcome, Outcome::Applied);
    }

    #[test]
    fn a6_decode_errors_are_never_retried() {
        let t = step_asset(&AssetStage::Decoding, AssetEvent::Decoded(Err(DecodeError::Corrupt("bad".into()))), true);
        assert!(matches!(t.next, AssetStage::Failed(AssetFailure::Decode(_))));
    }

    #[test]
    fn a11_collider_failure_is_terminal_with_the_reason() {
        let t = step_asset(&AssetStage::ColliderBuilding, AssetEvent::ColliderBuilt(Err(ColliderError::Empty)), true);
        assert_eq!(t.next, AssetStage::Failed(AssetFailure::Collider(ColliderError::Empty)));
        assert_eq!(t.effects, vec![AssetEffect::NotifyFailed(AssetFailure::Collider(ColliderError::Empty))]);
    }

    #[test]
    fn a14_an_explicit_request_retries_a_failed_asset() {
        let failed = AssetStage::Failed(AssetFailure::Fetch(FetchError::Network));
        let t = step_asset(&failed, AssetEvent::Request, true);
        assert_eq!(t.next, AssetStage::Fetching { attempt: 1 });
    }

    #[test]
    fn a15_requesting_a_ready_asset_is_unchanged_with_no_effects() {
        let t = step_asset(&AssetStage::Ready, AssetEvent::Request, true);
        assert_eq!(t.outcome, Outcome::Unchanged);
        assert!(t.effects.is_empty());
    }

    #[test]
    fn a13_release_returns_to_idle_and_frees_the_payload() {
        let t = step_asset(&AssetStage::Ready, AssetEvent::Release, true);
        assert_eq!(t.next, AssetStage::Idle);
        assert_eq!(t.effects, vec![AssetEffect::Release]);
    }
}
