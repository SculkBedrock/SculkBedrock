//! Health / hurt intents (reliable facts, never coalesced).
//!
//! Game semantics only; `sc_network` maps them to `UpdateAttributes`
//! (health bar) and `EntityEvent` HURT/DEATH (animation).

use sc_game_macros::GameIntent;

/// Authoritative health sync after damage (reliable fact).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct SyncHealth {
    pub entity_id: u64,
}

/// Authoritative hunger sync after food/saturation/exhaustion changes
/// (reliable fact). Health and hunger sync independently: each publishes
/// on its own change instead of sharing one coupled update.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct SyncHunger {
    pub entity_id: u64,
}

/// Hurt / death animation cue (reliable fact, lifecycle barrier).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct HurtEntity {
    pub entity_id: u64,
    pub fatal: bool,
}
