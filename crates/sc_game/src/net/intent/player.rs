//! Player and entity movement intents (game-side semantics, no protocol ids).

use crate::net::intent::{IntentPayload, IntentReliability};
use sc_game_macros::GameIntent;

/// Player move mode (game-side semantics).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerMoveMode {
    /// Regular movement sync.
    Normal,
    /// Authoritative server reset.
    Reset,
    /// Teleport.
    Teleport,
}

/// Player position/rotation sync (targets other player entities).
#[derive(Clone, Debug)]
pub struct MovePlayer {
    pub entity_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub on_ground: bool,
    pub mode: PlayerMoveMode,
}

// Data-dependent reliability: Normal moves regenerate next tick while
// Reset/Teleport are lifecycle facts. Hand-written instead of the
// `GameIntent` derive as the canonical custom-payload example.
impl IntentPayload for MovePlayer {
    fn reliability(&self) -> IntentReliability {
        match self.mode {
            PlayerMoveMode::Normal => IntentReliability::OverwritableState,
            PlayerMoveMode::Reset | PlayerMoveMode::Teleport => IntentReliability::ReliableFact,
        }
    }

    fn entity_id(&self) -> Option<u64> {
        Some(self.entity_id)
    }

    fn into_intent(self) -> super::NetworkIntent {
        super::NetworkIntent::MovePlayer(self)
    }
}

/// Entity position/rotation sync (network side owns byte encoding).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "entity_id")]
pub struct MoveEntityAbsolute {
    pub entity_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub on_ground: bool,
}

/// Set entity velocity.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "entity_id")]
pub struct SetEntityMotion {
    pub entity_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Remove an entity.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct RemoveEntity {
    pub entity_id: u64,
}

/// Dismiss the death screen and reload the spawn area (reliable fact).
/// `sc_network` maps it to the `Respawn` packet; the position duplicates
/// the authoritative teleport for clients that snap before `MovePlayer`.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct RespawnPlayer {
    pub entity_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
