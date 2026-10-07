//! `MinecraftWorldId` and `MinecraftWorldManager`: the world registry.
//!
//! Each world is identified by UUID; supports lookup by type/name, generator
//! wiring, and removal. `MinecraftWorldId` is also a component on player
//! entities (marks which world they are in).

use crate::storage::WorldGenerator;
use crate::world::MinecraftWorld;
use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use sc_utils::world::r#type::WorldType;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Component, Ord, PartialOrd, Eq, PartialEq, Hash, Debug, Clone)]
pub struct MinecraftWorldId {
    pub world_id: Uuid,
}

impl MinecraftWorldId {
    pub fn random() -> Self {
        Self {
            world_id: Uuid::new_v4(),
        }
    }
}

#[derive(Resource, Clone)]
pub struct MinecraftWorldManager {
    worlds: HashMap<MinecraftWorldId, MinecraftWorld>,
    worlds_type: HashMap<MinecraftWorldId, WorldType>,
    worlds_name: HashMap<MinecraftWorldId, String>,
}

impl MinecraftWorldManager {
    pub fn new() -> Self {
        Self {
            worlds: HashMap::new(),
            worlds_type: HashMap::new(),
            worlds_name: HashMap::new(),
        }
    }

    pub fn push_world(
        &mut self,
        world_type: WorldType,
        mut world: MinecraftWorld,
    ) -> MinecraftWorldId {
        let mut world_id = MinecraftWorldId::random();
        while self.worlds.contains_key(&world_id) {
            // Retry on UUID collision.
            world_id = MinecraftWorldId::random();
        }
        world.world_id = world_id.clone();
        let world_name = world.world_name.clone();
        self.worlds.insert(world_id.clone(), world);
        self.worlds_type.insert(world_id.clone(), world_type);
        self.worlds_name.insert(world_id.clone(), world_name);
        world_id
    }

    pub fn get_world(&self, world_id: &MinecraftWorldId) -> Option<&MinecraftWorld> {
        self.worlds.get(world_id)
    }

    /// All loaded worlds (used when shutdown needs to flush dirty chunks of every world).
    pub fn worlds(&self) -> impl Iterator<Item = &MinecraftWorld> {
        self.worlds.values()
    }

    pub fn get_worlds_by_type(&self, world_type: &WorldType) -> Vec<&MinecraftWorld> {
        let mut worlds = Vec::new();
        for (world_id, ty) in self.worlds_type.iter() {
            if ty == world_type {
                if let Some(world) = self.worlds.get(world_id) {
                    worlds.push(world);
                }
            }
        }
        worlds
    }

    pub fn get_worlds_by_name(&self, name: &String) -> Vec<&MinecraftWorld> {
        let mut worlds = Vec::new();
        for (world_id, nm) in self.worlds_name.iter() {
            if nm == name {
                if let Some(world) = self.worlds.get(world_id) {
                    worlds.push(world);
                }
            }
        }
        worlds
    }

    pub fn get_world_type(&self, world_id: &MinecraftWorldId) -> Option<&WorldType> {
        self.worlds_type.get(world_id)
    }

    pub fn get_world_mut(&mut self, world_id: &MinecraftWorldId) -> Option<&mut MinecraftWorld> {
        self.worlds.get_mut(world_id)
    }

    /// Installs a plugin-owned generator without exposing storage internals to
    /// the network crate.
    pub fn set_world_generator(
        &mut self,
        world_id: &MinecraftWorldId,
        generator: Arc<dyn WorldGenerator>,
    ) -> bool {
        let Some(world) = self.worlds.get_mut(world_id) else {
            return false;
        };
        world.chunk_provider = world.chunk_provider.clone().with_generator(generator);
        true
    }

    pub fn remove_world(&mut self, world_id: &MinecraftWorldId) -> Option<MinecraftWorld> {
        self.worlds_type.remove(world_id)?;
        self.worlds_name.remove(world_id);
        self.worlds.remove(world_id)
    }
}
