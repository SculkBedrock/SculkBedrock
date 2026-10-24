//! `EntityId` 与 `EntityManager`：实体句柄与分配。
//! `EntityId(u64)` 单调生成（id 永不重用）；`EntityLocation` 是存储索引。
//!
//! `EntityId` and `EntityManager`: entity handles and allocation.
//! `EntityId(u64)` is generated monotonically (ids are never reused);
//! `EntityLocation` is a storage index.

use std::fmt::{Display, Formatter};

/// 实体 id：由（代数, 索引）构成的唯一句柄。
///
/// Entity id: a unique handle composed of (generation, index).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct EntityId(u64);

impl Display for EntityId {
    /// 以十进制输出实体的原始 id 数值。
    ///
    /// Formats the entity's raw id value in decimal.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// 实体位置：实体在存储中的索引。
///
/// Entity location: the index of an entity in storage.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct EntityLocation(u32);

/// 实体槽位：记录代数与当前位置，用于回收与过期 id 检测。
///
/// Entity slot: tracks the generation and current location, used for
/// recycling and stale-id detection.
struct EntitySlot {
    /// 槽位代数：每次回收后自增，用于识别过期的 `EntityId`。
    ///
    /// Slot generation: incremented on every recycle, used to detect stale
    /// `EntityId`s.
    generation: u32,
    /// 实体当前所在位置；槽位空闲时为 `None`。
    ///
    /// The location the entity currently occupies; `None` when the slot is
    /// vacant.
    location: Option<EntityLocation>,
}

/// 实体管理器：分配/回收实体 id 并维护 id 与位置的映射。
///
/// Entity manager: allocates and recycles entity ids and maintains the
/// id/location mappings.
pub struct EntityManager {
    /// 按索引存放的槽位表。
    ///
    /// Slot table indexed by slot index.
    slots: Vec<EntitySlot>,
    /// 空闲槽位索引栈。
    ///
    /// Stack of free slot indices.
    free_slots: Vec<u32>,
    /// 位置到当前实体 id 的反查表。
    ///
    /// Reverse lookup from location to current entity id.
    locations: Vec<Option<EntityId>>,
}

impl EntityManager {
    /// 创建一个空的实体管理器。
    ///
    /// Creates an empty entity manager.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_slots: Vec::new(),
            locations: Vec::new(),
        }
    }

    /// 分配一个实体，返回其 id 与位置。
    ///
    /// Allocates an entity and returns its id and location.
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

    /// 回收一个实体，返回其原位置；id 过期或已回收时返回 `None`。
    ///
    /// Despawns an entity and returns its former location; returns `None`
    /// if the id is stale or already recycled.
    ///
    /// - `id`: 要回收的实体 id
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

    /// 查询实体当前的位置；id 过期时返回 `None`。
    ///
    /// Looks up the current location of an entity; returns `None` if the
    /// id is stale.
    ///
    /// - `entity`: 实体 id
    pub fn get_entity_location(&self, entity: &EntityId) -> Option<EntityLocation> {
        let index = entity.0 as u32;
        let slot = self.slots.get(index as usize)?;
        ((entity.0 >> 32) as u32 == slot.generation)
            .then(|| slot.location)
            .flatten()
    }

    /// 查询某个位置当前对应的实体 id；位置为空时返回 `None`。
    ///
    /// Looks up the entity id currently at a location; returns `None` if
    /// the location is vacant.
    ///
    /// - `location`: 实体位置
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
