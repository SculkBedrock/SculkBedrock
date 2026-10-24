//! 表现层特效 intent（容忍偶发丢失，始终为可覆盖状态）。
//!
//! Cosmetic effect intents (tolerate one miss, always overwritable state).

use sc_game_macros::GameIntent;
use sc_world::manager::MinecraftWorldId;

/// 语义声音提示（协议 id 保留在 `sc_network`）。
///
/// Semantic sound cue; protocol ids stay in `sc_network`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundCue {
    /// 方块破坏。
    ///
    /// Block break.
    BlockBreak,
    /// 方块放置。
    ///
    /// Block place.
    BlockPlace,
    /// 物品拾取。
    ///
    /// Item pickup.
    ItemPickup,
    /// 箱子打开。
    ///
    /// Chest open.
    ChestOpen,
    /// 箱子关闭。
    ///
    /// Chest close.
    ChestClose,
    /// 物品破损（工具/武器耐久耗尽）。
    ///
    /// Item break (tool or weapon ran out of durability).
    ItemBreak,
}

/// 语义粒子提示。
///
/// Semantic particle cue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleCue {
    /// 方块破坏粒子。
    ///
    /// Block-break particles.
    BlockBreak,
}

/// 方块破坏进度提示阶段。
///
/// Block-break progress cue stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockBreakProgressCue {
    /// 开始破坏。
    ///
    /// Break started.
    Start,
    /// 进度更新。
    ///
    /// Progress update.
    Update,
    /// 破坏停止。
    ///
    /// Break stopped.
    Stop,
}

/// 语义声音提示；协议 id 保留在 `sc_network`。附带世界音效事件的附加数据。
///
/// Semantic sound cue; protocol ids stay in `sc_network`.
/// Extra data for level sound events.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "none")]
pub struct PlaySound {
    pub world_id: MinecraftWorldId,
    pub cue: SoundCue,
    pub data: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// 播放粒子特效。
///
/// Plays a particle effect.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "none")]
pub struct PlayParticle {
    pub world_id: MinecraftWorldId,
    pub cue: ParticleCue,
    pub data: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// 方块破坏进度提示（开始/更新/停止）。
///
/// Block-break progress cue (start/update/stop).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "none")]
pub struct BlockBreakProgress {
    pub world_id: MinecraftWorldId,
    pub cue: BlockBreakProgressCue,
    pub data: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// 方块动画动作类型（如箱子盖子开闭）。
///
/// Block animation cue (e.g. chest lid open/close).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockAnimationCue {
    /// 箱子盖子状态变更（data: 0 为关闭，1 为开启）。
    ///
    /// Chest lid state change (data: 0 for close, 1 for open).
    ChestLid,
}

/// 播放方块动作动画（例如箱子开闭盖子动画）。
///
/// Plays a block action animation (e.g. chest lid open/close).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "none")]
pub struct PlayBlockAnimation {
    pub world_id: MinecraftWorldId,
    pub cue: BlockAnimationCue,
    /// 动画数据（如箱子：0 = 关闭，1 = 打开）。
    /// Animation data (chest: 0 = closed, 1 = open).
    pub data: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}
