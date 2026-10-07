//! `EntityId` and `EntityManager`: entity handles and allocation.
//!
//! `EntityId(u32)` is generated monotonically (ids are never reused, so a
//! despawn never shadows the components of a live entity);
//! `EntityLocation` is a storage index.

use std::fmt::{Display, Formatter};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct EntityId(u64);

impl Display for EntityId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct EntityLocation(u32);

struct EntitySlot {
    generation: u32,
    location: Option<EntityLocation>,
}

pub struct EntityManager {
    slots: Vec<EntitySlot>,
    free_slots: Vec<u32>,
    locations: Vec<Option<EntityId>>,
}

impl EntityManager {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_slots: Vec::new(),
            locations: Vec::new(),
        }
    }

    pub fn spawn(&mut self) -> (EntityId, EntityLocation) {
        let index = if let Some(index) = self.free_slots.pop() {
            index
        } else {
            let index = u32::try_from(self.slots.len()).expect("entity slot space exhausted");
            self.slots.push(EntitySlot {
                generation: 0,
                location: None,
            });
            self.locations.push(None);
            index
        };
        let slot = &mut self.slots[index as usize];
        let id = EntityId((u64::from(slot.generation) << 32) | u64::from(index));
        let location = EntityLocation(index);
        slot.location = Some(location);
        self.locations[index as usize] = Some(id);
        (id, location)
    }

    pub fn despawn(&mut self, id: &EntityId) -> Option<EntityLocation> {
        let index = id.0 as u32;
        let slot = self.slots.get_mut(index as usize)?;
        let generation = (id.0 >> 32) as u32;
        if slot.generation != generation {
            return None;
        }
        let location = slot.location.take()?;
        self.locations[index as usize] = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.free_slots.push(index);
        Some(location)
    }

    pub fn get_entity_location(&self, entity: &EntityId) -> Option<EntityLocation> {
        let index = entity.0 as u32;
        let slot = self.slots.get(index as usize)?;
        ((entity.0 >> 32) as u32 == slot.generation)
            .then(|| slot.location)
            .flatten()
    }

    pub fn get_entity_id(&self, location: &EntityLocation) -> Option<EntityId> {
        self.locations.get(location.0 as usize).copied().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::EntityManager;

    #[test]
    fn recycled_slot_rejects_stale_entity_id() {
        let mut manager = EntityManager::new();
        let (first, location) = manager.spawn();
        assert_eq!(manager.get_entity_location(&first), Some(location));
        assert_eq!(manager.despawn(&first), Some(location));
        let (second, second_location) = manager.spawn();
        assert_eq!(location, second_location);
        assert_ne!(first, second);
        assert_eq!(manager.get_entity_location(&first), None);
        assert_eq!(manager.get_entity_location(&second), Some(second_location));
    }
}
