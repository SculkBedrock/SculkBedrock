//! 物品实体生命周期 intent（生成/移动/消失/合并/拾取）。
//!
//! Item-entity lifecycle intents (spawn/move/despawn/merge/take).

use sc_game_macros::GameIntent;
use sc_item::ItemStack;
use sc_world::manager::MinecraftWorldId;

/// 生成一个物品实体（可靠事实）。
///
/// Spawns an item entity (reliable fact).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "runtime_id")]
pub struct SpawnItemEntity {
    pub world_id: MinecraftWorldId,
    pub runtime_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// 初始散射速度。
    ///
    /// Initial scatter velocity.
    pub motion_x: f32,
    pub motion_y: f32,
    pub motion_z: f32,
    pub stack: ItemStack,
}

/// 移动一个物品实体（可覆盖状态）。
///
/// Moves an item entity (overwritable state).
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

/// 消失一个物品实体（可靠事实）。
///
/// Despawns an item entity (reliable fact).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "runtime_id")]
pub struct DespawnItemEntity {
    pub world_id: MinecraftWorldId,
    pub runtime_id: u64,
}

/// 物品合并后同步幸存者的新堆叠数。
///
/// Sync a survivor's new stack count after item merge.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "runtime_id")]
pub struct UpdateItemStackSize {
    pub world_id: MinecraftWorldId,
    pub runtime_id: u64,
    pub count: u16,
}

/// 物品实体被玩家拾取（可靠事实）。
///
/// An item entity was picked up by a player (reliable fact).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "runtime_id")]
pub struct TakeItemEntity {
    pub world_id: MinecraftWorldId,
    pub runtime_id: u64,
    pub target_entity_id: u64,
}
