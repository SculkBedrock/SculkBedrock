//! 方块权威 intent（单所有者提交，参见 `block::facts`）。
//!
//! Block authority intents (single-owner commits, see `block::facts`).

use sc_game_macros::GameIntent;
use sc_world::manager::MinecraftWorldId;

/// 方块变更（携带世界信息以供选择接收者）。
///
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

/// 单方块预测回滚，仅发给做预测的玩家。
///
/// Single-block prediction rollback, sent only to the predicting player.
///
/// 从不触发整列重发或客户端闪烁；拥塞时升级为整列刷新。
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
