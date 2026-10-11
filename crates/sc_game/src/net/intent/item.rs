//! Item-entity lifecycle intents (spawn/move/despawn/merge/take).

use sc_game_macros::GameIntent;
use sc_item::ItemStack;
use sc_world::manager::MinecraftWorldId;

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "runtime_id")]
pub struct SpawnItemEntity {
    pub world_id: MinecraftWorldId,
    pub runtime_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Initial scatter velocity.
    pub motion_x: f32,
    pub motion_y: f32,
    pub motion_z: f32,
    pub stack: ItemStack,
}

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "runtime_id")]
pub struct MoveItemEntity {
    pub world_id: MinecraftWorldId,
    pub runtime_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub on_ground: bool,
}

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "runtime_id")]
pub struct DespawnItemEntity {
    pub world_id: MinecraftWorldId,
    pub runtime_id: u64,
}

/// Sync a survivor's new stack count after item merge.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "runtime_id")]
pub struct UpdateItemStackSize {
    pub world_id: MinecraftWorldId,
    pub runtime_id: u64,
    pub count: u16,
}

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "runtime_id")]
pub struct TakeItemEntity {
    pub world_id: MinecraftWorldId,
    pub runtime_id: u64,
    pub target_entity_id: u64,
}
