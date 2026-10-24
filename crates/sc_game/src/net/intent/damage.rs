//! 生命值/受伤 intent（可靠事实，从不合并）。
//!
//! Health / hurt intents (reliable facts, never coalesced).
//!
//! 仅含游戏语义；`sc_network` 负责把它们映射为 `UpdateAttributes`
//! （血条）和 `EntityEvent` HURT/DEATH（动画）。
//! Game semantics only; `sc_network` maps them to `UpdateAttributes`
//! (health bar) and `EntityEvent` HURT/DEATH (animation).

use sc_game_macros::GameIntent;

/// 伤害后的权威生命值同步（可靠事实）。
///
/// Authoritative health sync after damage (reliable fact).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct SyncHealth {
    pub entity_id: u64,
}

/// 食物/饱和度/精力变化后的权威饥饿度同步（可靠事实）。
///
/// Authoritative hunger sync after food/saturation/exhaustion changes
/// (reliable fact).
///
/// 生命值与饥饿度独立同步：各自在变化时发布，不共享一次耦合更新。
/// Health and hunger sync independently: each publishes on its own change
/// instead of sharing one coupled update.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct SyncHunger {
    pub entity_id: u64,
}

/// 受伤/死亡动画提示（可靠事实，生命周期屏障）。
///
/// Hurt / death animation cue (reliable fact, lifecycle barrier).
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "fact", entity = "entity_id")]
pub struct HurtEntity {
    pub entity_id: u64,
    pub fatal: bool,
}
