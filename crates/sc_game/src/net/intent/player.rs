//! 玩家与实体移动 intent（游戏侧语义，不含协议 id）。
//!
//! Player and entity movement intents (game-side semantics, no protocol ids).

use crate::net::intent::{IntentPayload, IntentReliability};
use sc_game_macros::GameIntent;

/// 玩家移动模式（游戏侧语义）。
///
/// Player move mode (game-side semantics).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerMoveMode {
    /// 常规移动同步。
    ///
    /// Regular movement sync.
    Normal,
    /// 权威服务端重置。
    ///
    /// Authoritative server reset.
    Reset,
    /// 传送。
    ///
    /// Teleport.
    Teleport,
}

/// 玩家位置/旋转同步（目标是其他玩家实体）。
///
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

/// 实体位置/旋转同步（字节编码由网络侧负责）。
///
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

/// 设置实体速度。
///
/// Set entity velocity.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "entity_id")]
pub struct SetEntityMotion {
    pub entity_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// 移除一个实体。
///
/// Remove an entity.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct RemoveEntity {
    pub entity_id: u64,
}

/// 关闭死亡界面并重载出生区域（可靠事实）。
///
/// Dismiss the death screen and reload the spawn area (reliable fact).
///
/// `sc_network` 将其映射为 `Respawn` 包；其中的位置复制权威传送，
/// 供那些在收到 `MovePlayer` 前先吸附的客户端使用。
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

/// 同步玩家游泳状态给玩家本人及同世界观看者。
///
/// Sync player swimming state to the player and same-world viewers.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "entity_id")]
pub struct SyncPlayerSwimming {
    pub entity_id: u64,
    pub is_swimming: bool,
}

/// 同步玩家氧气值与呼吸状态给玩家本人及同世界观看者。
///
/// Sync player air supply and breathing state to the player and same-world viewers.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "entity_id")]
pub struct SyncAirSupply {
    pub entity_id: u64,
    pub air_supply: i16,
    pub max_air: i16,
    pub is_breathing: bool,
}


