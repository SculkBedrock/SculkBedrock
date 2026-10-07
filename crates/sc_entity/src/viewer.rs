//! Entity visibility-set components.
//!
//! Tracks which players see each entity (entity side) and the viewer set (player side).
//! `viewer_sync` maintains them via full `ChunkView` diffs (spawn/despawn).
//! `movement_broadcast` falls back to all InGame players in the same world when the set is empty.

use std::collections::HashSet;
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;

/// Which players see an entity (loader id to player).
///
/// Removed by `viewer_sync` when a player leaves range; broadcasts (MoveEntityAbsolute /
/// SetEntityMotion) go only to players in the set.
#[derive(Component, Clone, Debug, Default)]
pub struct SpawnedFor {
    pub players: HashSet<EntityId>,
}

/// Which entities are in a player view (player-side mirror).
///
/// Updated by full `viewer_sync` diffs when a player crosses chunks.
#[derive(Component, Clone, Debug, Default)]
pub struct ViewersOf {
    pub entities: HashSet<EntityId>,
}
