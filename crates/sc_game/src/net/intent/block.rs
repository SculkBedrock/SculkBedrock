//! Block authority intents (single-owner commits, see `block::facts`).

use sc_game_macros::GameIntent;
use sc_world::manager::MinecraftWorldId;

/// Block change (carries the world for receiver selection).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "none")]
pub struct UpdateBlock {
    pub world_id: MinecraftWorldId,
    pub dimension: i32,
    pub incarnation: u128,
    pub generation: u64,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub runtime_id: u32,
    pub flags: u32,
    pub layer: u32,
}

/// Single-block prediction rollback, sent only to the predicting player.
/// Never triggers full-column resends or client flicker; congestion
/// upgrades to column refresh.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct CorrectBlockPrediction {
    pub entity_id: u64,
    pub world_id: MinecraftWorldId,
    pub dimension: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub runtime_id: u32,
    pub flags: u32,
    pub layer: u32,
}
