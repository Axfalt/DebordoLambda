//! Watchtower (tour de guet) estimation: finds the seeded offset path behind the day's readings
//! and the attack range it implies.

pub mod engine;
pub mod format;
pub mod inference;
mod isa;
pub mod mho;
pub mod parse;
pub mod seed;

pub use engine::{AttackMode, EstimConf, HiddenTarget};
pub use format::format_summary;
pub use inference::{
    Estimate, EstimationError, EstimationInput, Observation, Reading, Summary, WindowEstimate,
    check_input, estimate, finish, observations, search_seeds, summarize,
};
pub use parse::{ParsedText, parse_text};
pub use seed::{Window, WindowMatch, seed_slice};
