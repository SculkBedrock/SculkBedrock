//! Multi-world model: `GameWorldId` / `GameWorld` / `WorldHub`.
//!
//! Each game world is an independent ECS `World` instance sharing the same
//! game command code. `WorldHub` lives on the root world; `hub_tick` runs
//! in-world commands over Running instances each tick.

use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_world::manager::MinecraftWorldId;
use std::collections::HashMap;

use crate::bus::PingMailbox;

/// World instance id (0=overworld, 1=nether, 2=end, higher=custom).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GameWorldId(pub u32);

/// Owning GameWorld of a player (explicit game-domain ownership, distinct
/// from the `MinecraftWorldId` save id). Maintained by position and updated
/// after cross-world teleport; game systems route by this component.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerGameWorld(pub GameWorldId);

/// World instance run state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldStatus {
    /// Runs game commands every tick.
    Running,
    /// Paused (teleporting/saving), skips ticks.
    Paused,
    /// Unloading (removed from the hub after migration/flush).
    Unloading,
}

/// One running game world instance.
#[derive(Clone)]
pub struct GameWorld {
    pub id: GameWorldId,
    /// Independent ECS World instance.
    pub world: World,
    pub status: WorldStatus,
}

/// Multi-world hub (a root-world resource).
///
/// `spawn_world` attaches default resources to new instances; game systems
/// iterate instances via `running()`/`running_mut()`.
#[derive(Resource, Clone, Default)]
pub struct WorldHub {
    worlds: HashMap<GameWorldId, GameWorld>,
}

impl WorldHub {
    /// Spawn a Running world instance with default resources.
    pub fn spawn_world(&mut self, id: GameWorldId) -> GameWorldId {
        let world = World::new();
        world.insert_resource(TickCounter::default());
        world.insert_resource(PingMailbox::default());
        self.worlds.insert(
            id,
            GameWorld {
                id,
                world,
                status: WorldStatus::Running,
            },
        );
        id
    }

    pub fn get(&self, id: &GameWorldId) -> Option<&GameWorld> {
        self.worlds.get(id)
    }

    pub fn get_mut(&mut self, id: &GameWorldId) -> Option<&mut GameWorld> {
        self.worlds.get_mut(id)
    }

    /// Remove an instance (pause, flush, then remove).
    pub fn remove(&mut self, id: &GameWorldId) -> Option<GameWorld> {
        self.worlds.remove(id)
    }

    pub fn count(&self) -> usize {
        self.worlds.len()
    }

    pub fn contains(&self, id: &GameWorldId) -> bool {
        self.worlds.contains_key(id)
    }

    /// Iterate Running instances (read-only).
    pub fn running(&self) -> impl Iterator<Item = &GameWorld> + '_ {
        self.worlds
            .values()
            .filter(|w| w.status == WorldStatus::Running)
    }

    /// Iterate Running instances (mutable).
    pub fn running_mut(&mut self) -> impl Iterator<Item = &mut GameWorld> + '_ {
        self.worlds
            .values_mut()
            .filter(|w| w.status == WorldStatus::Running)
    }
}

/// Per-instance tick counter (attached by `spawn_world`).
#[derive(Resource, Clone, Debug, Default)]
pub struct TickCounter {
    pub ticks: u64,
}

/// GameWorldId to Minecraft save-world map.
///
/// Each dimension (Overworld/Nether/End) gets one `GameWorld` instance,
/// linked here to save data in `MinecraftWorldManager` plus the dimension
/// id used to build ChunkKeys.
#[derive(Resource, Clone, Debug, Default)]
pub struct GameWorldMap {
    entries: HashMap<GameWorldId, GameWorldEntry>,
}

/// Save info for one GameWorld.
#[derive(Clone, Debug)]
pub struct GameWorldEntry {
    pub dimension: i32,
    pub minecraft_world_id: MinecraftWorldId,
}

impl GameWorldMap {
    pub fn insert(&mut self, id: GameWorldId, entry: GameWorldEntry) {
        self.entries.insert(id, entry);
    }

    pub fn get(&self, id: &GameWorldId) -> Option<&GameWorldEntry> {
        self.entries.get(id)
    }

    /// Find a GameWorldId by Minecraft save world.
    pub fn find_by_world(&self, minecraft_world_id: &MinecraftWorldId) -> Option<GameWorldId> {
        self.entries
            .iter()
            .find(|(_, entry)| entry.minecraft_world_id == *minecraft_world_id)
            .map(|(id, _)| *id)
    }

    pub fn remove(&mut self, id: &GameWorldId) -> Option<GameWorldEntry> {
        self.entries.remove(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
