//! Watchtower (tour de guet) inference: narrows the real attack using every reading of the day.

pub mod engine;
pub mod format;
pub mod inference;
pub mod parse;
pub mod seed;

pub use engine::{AttackMode, EstimConf, HiddenTarget};
pub use format::format_summary;
pub use inference::{
    ExactPosterior, GuetError, GuetInput, InferenceOptions, Posterior, Reading, infer, infer_exact,
};
pub use parse::{ParsedText, parse_text};
