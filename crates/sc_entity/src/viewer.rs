//! 实体可见集组件。
//!
//! 记录每个实体被哪些玩家看到（实体侧）以及玩家视野中的观察者集合（玩家侧）。
//! `viewer_sync` 通过完整的 `ChunkView` 差分（spawn/despawn）维护这些集合；
//! 集合为空时 `movement_broadcast` 回退到同世界内全部 InGame 玩家。
//!
//! Entity visibility-set components.
//!
//! Tracks which players see each entity (entity side) and the viewer set (player side).
//! `viewer_sync` maintains them via full `ChunkView` diffs (spawn/despawn).
//! `movement_broadcast` falls back to all InGame players in the same world when the set is empty.

use std::collections::HashSet;
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;

/// 哪些玩家看到该实体（loader id 到玩家）。
///
/// Which players see an entity (loader id to player).
///
/// Removed by `viewer_sync` when a player leaves range; broadcasts (MoveEntityAbsolute /
/// SetEntityMotion) go only to players in the set.
#[derive(Component, Clone, Debug, Default)]
pub struct SpawnedFor {
    pub players: HashSet<EntityId>,
}

/// 玩家视野中包含哪些实体（玩家侧镜像）。
///
/// Which entities are in a player view (player-side mirror).
///
/// Updated by full `viewer_sync` diffs when a player crosses chunks.
#[derive(Component, Clone, Debug, Default)]
pub struct ViewersOf {
    pub entities: HashSet<EntityId>,
}
