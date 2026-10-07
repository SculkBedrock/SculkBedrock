//! `MinecraftEntitiesManager`: entity registry (identifier to spawner).
//!
//! Built from version-pack behavior-pack data; the `SpawnEntity` trait spawns entities by identifier from it.

use crate::MinecraftEntityId;
use std::collections::HashMap;
use sc_ecs::entity::EntityId;
use sc_ecs::resource::Resource;
use sc_ecs::world::World;

#[derive(Resource, Clone)]
pub struct MinecraftEntitiesManager {
    entities: HashMap<u64, EntityId>,
    /// Reverse index kept in sync with `entities`, avoiding O(n) scans on reverse lookup by EntityId.
    entity_ids: HashMap<EntityId, u64>,
    id_count: u64,
}

impl MinecraftEntitiesManager {
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            entity_ids: HashMap::new(),
            id_count: 0,
        }
    }

    pub fn push_entity(&mut self, world: &World, entity: &EntityId) {
        self.id_count += 1;
        world.add_component(entity, MinecraftEntityId(self.id_count));
        self.entities.insert(self.id_count, *entity);
        self.entity_ids.insert(*entity, self.id_count);
    }

    /// Remove an entity from the manager by its MinecraftEntityId.
    /// This must be called when a player disconnects to prevent the
    /// entities HashMap from growing unbounded over many join/leave cycles.
    pub fn remove_entity(&mut self, mc_entity_id: u64) -> Option<EntityId> {
        let result = self.entities.remove(&mc_entity_id);
        if let Some(entity) = result {
            self.entity_ids.remove(&entity);
        }
        self.entities.shrink_to_fit();
        self.entity_ids.shrink_to_fit();
        result
    }

    /// Remove an entity from the manager by its ECS EntityId.
    pub fn remove_entity_by_ecs_id(&mut self, entity: &EntityId) -> Option<u64> {
        let mc_id = self.entity_ids.remove(entity)?;
        self.entities.remove(&mc_id);
        self.entities.shrink_to_fit();
        self.entity_ids.shrink_to_fit();
        Some(mc_id)
    }

    pub fn get_entity(&self, id: u64) -> Option<EntityId> {
        self.entities.get(&id).copied()
    }

    pub fn get_id(&self, entity: EntityId) -> Option<u64> {
        self.entity_ids.get(&entity).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u64, &EntityId)> {
        self.entities.iter()
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }
}
