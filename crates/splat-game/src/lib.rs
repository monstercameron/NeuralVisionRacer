//! Pure state machines for Splat Racer. Each machine is a function `step(state, event) -> transition`:
//! no I/O, no clocks, no GPU, no physics. The driver in `splat-engine` owns all of that.
//!
//! See `documents/state-machines.md` for the transition tables these functions implement. Every row is named
//! in a comment (`G7`, `V21`, `A3`) so the code can be checked against the table.

pub mod asset;
pub mod game;
pub mod record;
pub mod vehicle;

pub use record::{Outcome, Record, Reject};
