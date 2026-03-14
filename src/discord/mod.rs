pub mod api;
pub mod signature;
pub mod types;

// These re-exports are intentionally public for the bootstrap binary; the worker
// binary doesn't use them all, so suppress the unused-imports lint here.
#[allow(unused_imports)]
pub use signature::verify_discord_signature;
#[allow(unused_imports)]
pub use types::{
    interaction_types, response_types, DiscordInteraction, DiscordResponse,
};

// pas utilisé pour le moment mais prêt à l'emploi pour les interactions différées