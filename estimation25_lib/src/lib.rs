//! Watchtower (tour de guet) estimation: finds the seeded offset path behind every reading of the
//! day and the attack range it implies.

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
    Estimate, EstimationError, EstimationInput, Reading, SeedEstimate, check_input, estimate,
    finish, search_seeds,
};
pub use parse::{ParsedText, parse_text};
pub use seed::{SeedMatch, seed_slices};
