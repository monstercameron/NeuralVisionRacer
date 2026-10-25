//! What a transition did, in a form the log and tests can both read.

use std::fmt;

/// Why an event was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    WrongPhase,
    Paused,
    NotPaused,
    InRace,
    NotLoaded,
    AssetsNotReady,
    AlreadyRespawning,
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Reject::WrongPhase => "WrongPhase",
            Reject::Paused => "Paused",
            Reject::NotPaused => "NotPaused",
            Reject::InRace => "InRace",
            Reject::NotLoaded => "NotLoaded",
            Reject::AssetsNotReady => "AssetsNotReady",
            Reject::AlreadyRespawning => "AlreadyRespawning",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The state changed (its tag, or its effects).
    Applied,
    /// The event was valid but left the tag as it was. Counter-only steps are `Unchanged` with no effects.
    Unchanged,
    /// No row matched. Expected for samples in states that do not use them.
    Ignored,
    /// The event was refused for a stated reason. Always logged.
    Rejected(Reject),
}

impl Outcome {
    pub fn label(&self) -> String {
        match self {
            Outcome::Applied => "applied".into(),
            Outcome::Unchanged => "unchanged".into(),
            Outcome::Ignored => "ignored".into(),
            Outcome::Rejected(reason) => format!("rejected: {reason}"),
        }
    }
}

/// One machine step, as the log records it: `from --event--> to [outcome]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub machine: String,
    pub from: String,
    pub event: String,
    pub to: String,
    pub outcome: Outcome,
}

impl Record {
    pub fn new(machine: impl Into<String>, from: String, event: &str, to: String, outcome: Outcome) -> Self {
        Self { machine: machine.into(), from, event: event.to_string(), to, outcome }
    }

    /// The log message, for example `Race(Countdown) --CountdownElapsed--> Race(Running) [applied]`.
    pub fn message(&self) -> String {
        format!("{} --{}--> {} [{}]", self.from, self.event, self.to, self.outcome.label())
    }

    /// Whether the driver should log this record. Counter-only steps (same tag, no effect) are not logged.
    pub fn is_loggable(&self) -> bool {
        self.from != self.to || self.outcome != Outcome::Unchanged
    }
}
