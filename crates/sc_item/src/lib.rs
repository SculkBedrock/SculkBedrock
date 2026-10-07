//! Authoritative server-side item model.
//!
//! This crate does not depend on the network protocol: the network layer only converts `ItemData`
//! to/from `ItemStack`, while gameplay validates transactions and commits inventory. Version packs can
//! therefore register new items without hardcoding item ids into the protocol or chunk core.

use parking_lot::RwLock;
use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use std::collections::HashMap;
use std::sync::Arc;

/// A stackable server-side item.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ItemStack {
    /// Bedrock network item runtime id; 0 means air.
    pub runtime_id: u16,
    pub count: u16,
    pub damage: u32,
    /// Associated default block network runtime id. Not every item maps to a block.
    pub block_runtime_id: u32,
}

impl ItemStack {
    pub const EMPTY: Self = Self {
        runtime_id: 0,
        count: 0,
        damage: 0,
        block_runtime_id: 0,
    };

    pub fn empty() -> Self {
        Self::EMPTY
    }

    pub fn new(runtime_id: u16, count: u16) -> Self {
        Self {
            runtime_id,
            count,
            ..Self::EMPTY
        }
    }

    pub fn with_block(runtime_id: u16, block_runtime_id: u32, count: u16) -> Self {
        Self {
            runtime_id,
            block_runtime_id,
            count,
            damage: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.runtime_id == 0 || self.count == 0
    }

    pub fn clear_if_empty(&mut self) {
        if self.is_empty() {
            *self = Self::EMPTY;
        }
    }
}

/// Item registry entry. Version packs link items to default block states through it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemDefinition {
    pub runtime_id: u16,
    pub name: Arc<str>,
    pub max_stack_size: u16,
    pub default_block_runtime_id: Option<u32>,
}

impl ItemDefinition {
    pub fn new(runtime_id: u16, name: impl Into<Arc<str>>) -> Self {
        Self {
            runtime_id,
            name: name.into(),
            max_stack_size: 64,
            default_block_runtime_id: None,
        }
    }

    pub fn block(mut self, block_runtime_id: u32) -> Self {
        self.default_block_runtime_id = Some(block_runtime_id);
        self
    }

    pub fn max_stack_size(mut self, max_stack_size: u16) -> Self {
        self.max_stack_size = max_stack_size.max(1);
        self
    }
}

/// Item registry fillable dynamically by version packs.
///
/// Dense primary table indexed by network runtime id (runtime ids are contiguous 0..N, serving as both
/// network ids and array indices) for O(1) lookup; `by_name` resolves identifier to runtime for block
/// drops/tools (`sc:mining.tools[].items` / `sc:drops.entries[].item`).
#[derive(Resource, Clone, Debug, Default)]
pub struct ItemRegistry {
    /// Dense primary table: runtime_id is the Vec index (slot 0 = air, not registered).
    entries: Arc<RwLock<Vec<Option<ItemDefinition>>>>,
    /// Item identifier to runtime id (for drop/tool resolution; first registration wins, renames keep the smallest id).
    by_name: Arc<RwLock<HashMap<Box<str>, u16>>>,
}

impl ItemRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, definition: ItemDefinition) -> Result<(), ItemDefinition> {
        let id = definition.runtime_id as usize;
        let mut entries = self.entries.write();
        if entries.len() <= id {
            entries.resize(id + 1, None);
        }
        if entries[id].is_some() {
            return Err(definition);
        }
        Self::link_name(&self.by_name, &definition);
        entries[id] = Some(definition);
        Ok(())
    }

    pub fn upsert(&self, definition: ItemDefinition) {
        let id = definition.runtime_id as usize;
        let mut entries = self.entries.write();
        if entries.len() <= id {
            entries.resize(id + 1, None);
        }
        // On rename under the same runtime_id, drop the old name mapping (avoids dangling identifiers).
        if let Some(old) = entries[id].as_ref() {
            if old.name.as_ref() != definition.name.as_ref() {
                {
                    let old_key: &str = &old.name;
                    self.by_name.write().remove(old_key);
                }
            }
        }
        Self::link_name(&self.by_name, &definition);
        entries[id] = Some(definition);
    }

    /// Registers the name-to-runtime reverse lookup; renames keep the smaller runtime id.
    fn link_name(by_name: &Arc<RwLock<HashMap<Box<str>, u16>>>, definition: &ItemDefinition) {
        let mut by_name = by_name.write();
        let key: &str = &definition.name;
        match by_name.get(key) {
            Some(existing) if *existing <= definition.runtime_id => {}
            _ => {
                let k: &str = &definition.name;
                by_name.insert(Box::from(k), definition.runtime_id);
            }
        }
    }

    /// Item identifier to runtime id (for `sc:mining`/`sc:drops` resolution).
    pub fn runtime_id_by_name(&self, name: &str) -> Option<u16> {
        self.by_name.read().get(name).copied()
    }

    /// Whether this item identifier is registered.
    pub fn contains_name(&self, name: &str) -> bool {
        self.by_name.read().contains_key(name)
    }

    /// All registered identifiers (one startup snapshot for building recipe pseudo-name family tables).
    pub fn names(&self) -> Vec<String> {
        self.by_name.read().keys().map(|key| key.to_string()).collect()
    }

    /// O(1): runtime_id indexes directly, no hashing.
    pub fn get(&self, runtime_id: u16) -> Option<ItemDefinition> {
        self.entries
            .read()
            .get(runtime_id as usize)
            .cloned()
            .flatten()
    }

    pub fn block_runtime_id(&self, runtime_id: u16) -> Option<u32> {
        self.get(runtime_id)?.default_block_runtime_id
    }

    pub fn max_stack_size(&self, runtime_id: u16) -> u16 {
        self.get(runtime_id)
            .map(|entry| entry.max_stack_size)
            .unwrap_or(64)
    }

    /// Registered item count (skipping hollow indices).
    pub fn len(&self) -> usize {
        self.entries
            .read()
            .iter()
            .filter(|entry| entry.is_some())
            .count()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// All "block item" definitions (items with a default_block_runtime_id), in ascending runtime_id.
    /// Used to build listings that enumerate placeable items, such as the creative inventory (CreativeContent).
    pub fn block_items(&self) -> Vec<ItemDefinition> {
        let entries = self.entries.read();
        let mut out: Vec<ItemDefinition> = entries
            .iter()
            .filter_map(Clone::clone)
            .filter(|definition| definition.default_block_runtime_id.is_some())
            .collect();
        out.sort_by_key(|definition| definition.runtime_id);
        out
    }
}

/// Player main inventory (0..36; hotbar is 0..9).
#[derive(Component, Clone, Debug)]
pub struct PlayerInventory {
    pub slots: Arc<RwLock<Vec<ItemStack>>>,
    pub selected_slot: Arc<RwLock<u8>>,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self::new(36)
    }
}

impl PlayerInventory {
    pub fn new(size: usize) -> Self {
        Self {
            slots: Arc::new(RwLock::new(vec![ItemStack::empty(); size])),
            selected_slot: Arc::new(RwLock::new(0)),
        }
    }

    pub fn len(&self) -> usize {
        self.slots.read().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn selected_slot(&self) -> u8 {
        *self.selected_slot.read()
    }

    pub fn set_selected_slot(&self, slot: u8) -> bool {
        let mut selected = self.selected_slot.write();
        if slot as usize >= self.len() {
            return false;
        }
        *selected = slot;
        true
    }

    pub fn get(&self, slot: usize) -> Option<ItemStack> {
        self.slots.read().get(slot).cloned()
    }

    pub fn set(&self, slot: usize, stack: ItemStack) -> Option<ItemStack> {
        let mut slots = self.slots.write();
        let target = slots.get_mut(slot)?;
        let old = std::mem::replace(target, stack);
        target.clear_if_empty();
        Some(old)
    }

    /// Reserves one item from a slot; callers keep the change after a successful block write,
    /// or roll back with `set(slot, old_stack)` on failure.
    pub fn reserve_one(&self, slot: usize) -> Option<ItemStack> {
        let mut slots = self.slots.write();
        let stack = slots.get_mut(slot)?;
        if stack.is_empty() {
            return None;
        }
        let reserved = stack.clone();
        stack.count -= 1;
        stack.clear_if_empty();
        Some(reserved)
    }

    /// Return one previously reserved item to its slot.
    ///
    /// This is incremental so multiple pending placements on one slot cannot
    /// restore the entire stack more than once.
    pub fn restore_one(&self, slot: usize, item: &ItemStack) -> bool {
        if item.is_empty() {
            return false;
        }
        let mut slots = self.slots.write();
        let Some(stack) = slots.get_mut(slot) else {
            return false;
        };
        if stack.is_empty() {
            *stack = ItemStack {
                count: 1,
                ..item.clone()
            };
            return true;
        }
        if stack.runtime_id != item.runtime_id
            || stack.damage != item.damage
            || stack.block_runtime_id != item.block_runtime_id
        {
            return false;
        }
        let Some(count) = stack.count.checked_add(1) else {
            return false;
        };
        stack.count = count;
        true
    }

    /// Whether a whole item batch fits (accumulates remaining capacity slot by slot,
    /// true only when the full batch fits; used for pickup checks).
    pub fn can_add(&self, incoming: ItemStack, registry: &ItemRegistry) -> bool {
        if incoming.is_empty() {
            return true;
        }
        let max_stack = registry.max_stack_size(incoming.runtime_id);
        let mut remaining = incoming.count;
        for slot in self.slots.read().iter() {
            if slot.runtime_id == incoming.runtime_id
                && slot.damage == incoming.damage
                && slot.block_runtime_id == incoming.block_runtime_id
            {
                let room = max_stack.saturating_sub(slot.count);
                remaining = remaining.saturating_sub(room);
            } else if slot.is_empty() {
                remaining = remaining.saturating_sub(max_stack);
            }
            if remaining == 0 {
                return true;
            }
        }
        false
    }

    /// Tries to merge items into the inventory, returning (leftover count, changed slot list).
    ///
    /// Merges into existing stacks of the same item first, then fills empty slots; each
    /// `setItem` fires `onSlotChange` -> `sendSlot` (per-slot InventorySlotPacket).
    /// Records the indices that actually changed so callers can sync per slot.
    pub fn insert_tracked(
        &self,
        mut incoming: ItemStack,
        registry: &ItemRegistry,
    ) -> (u16, Vec<usize>) {
        let mut changed = Vec::new();
        if incoming.is_empty() {
            return (0, changed);
        }
        let max_stack = registry.max_stack_size(incoming.runtime_id);
        let mut slots = self.slots.write();
        for (slot_index, slot) in slots.iter_mut().enumerate() {
            if incoming.is_empty() {
                break;
            }
            if slot.runtime_id == incoming.runtime_id
                && slot.damage == incoming.damage
                && slot.block_runtime_id == incoming.block_runtime_id
            {
                let room = max_stack.saturating_sub(slot.count);
                let moved = room.min(incoming.count);
                if moved > 0 {
                    slot.count += moved;
                    incoming.count -= moved;
                    changed.push(slot_index);
                }
            }
        }
        for (slot_index, slot) in slots.iter_mut().enumerate() {
            if incoming.is_empty() {
                break;
            }
            if slot.is_empty() {
                let moved = max_stack.min(incoming.count);
                *slot = ItemStack {
                    count: moved,
                    ..incoming.clone()
                };
                incoming.count -= moved;
                changed.push(slot_index);
            }
        }
        (incoming.count, changed)
    }

    /// Tries to merge items into the inventory, returning the leftover count.
    pub fn insert(&self, incoming: ItemStack, registry: &ItemRegistry) -> u16 {
        self.insert_tracked(incoming, registry).0
    }

    pub fn snapshot(&self) -> Vec<ItemStack> {
        self.slots.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_and_insert_obey_stack_limit() {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(1, "minecraft:stone").max_stack_size(2));
        let inventory = PlayerInventory::new(2);
        inventory.set(0, ItemStack::new(1, 2));
        assert_eq!(inventory.reserve_one(0).unwrap().count, 2);
        assert_eq!(inventory.get(0).unwrap().count, 1);
        assert_eq!(inventory.insert(ItemStack::new(1, 4), &registry), 1);
        assert_eq!(inventory.get(0).unwrap().count, 2);
    }

    #[test]
    fn registered_block_mapping_is_data_driven() {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(5, "minecraft:stone").block(123));
        assert_eq!(registry.block_runtime_id(5), Some(123));
    }

    #[test]
    fn restore_one_only_restores_a_single_reserved_item() {
        let inventory = PlayerInventory::new(1);
        inventory.set(0, ItemStack::new(1, 2));
        let reserved = inventory.reserve_one(0).unwrap();
        assert_eq!(inventory.get(0).unwrap().count, 1);
        assert!(inventory.restore_one(
            0,
            &ItemStack {
                count: 1,
                ..reserved
            }
        ));
        assert_eq!(inventory.get(0).unwrap().count, 2);
    }

    #[test]
    fn block_items_returns_only_placeable_sorted_by_runtime_id() {
        let registry = ItemRegistry::new();
        // Non-block items (e.g. swords) are excluded; block items sort by ascending runtime_id.
        registry.upsert(ItemDefinition::new(9, "minecraft:diamond_sword"));
        registry.upsert(ItemDefinition::new(7, "minecraft:dirt").block(200));
        registry.upsert(ItemDefinition::new(3, "minecraft:stone").block(100));
        let block_items = registry.block_items();
        assert_eq!(block_items.len(), 2);
        assert_eq!(block_items[0].runtime_id, 3);
        assert_eq!(block_items[1].runtime_id, 7);
        assert_eq!(block_items[0].default_block_runtime_id, Some(100));
    }

    #[test]
    fn dense_table_get_is_o1_and_holes_return_none() {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(3, "minecraft:stone"));
        registry.upsert(ItemDefinition::new(257, "minecraft:apple"));
        // runtime_id is the index: direct lookup, no panic.
        assert_eq!(registry.get(3).unwrap().name.as_ref(), "minecraft:stone");
        assert_eq!(registry.get(257).unwrap().name.as_ref(), "minecraft:apple");
        // Hollow indices return None.
        assert!(registry.get(4).is_none());
        assert!(registry.get(999).is_none());
        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());
    }

    #[test]
    fn name_lookup_tracks_registry() {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(3, "minecraft:stone"));
        registry.upsert(ItemDefinition::new(7, "minecraft:dirt"));
        assert_eq!(registry.runtime_id_by_name("minecraft:stone"), Some(3));
        assert_eq!(registry.runtime_id_by_name("minecraft:dirt"), Some(7));
        assert_eq!(registry.runtime_id_by_name("minecraft:unknown"), None);
        assert!(registry.contains_name("minecraft:stone"));
        assert!(!registry.contains_name("minecraft:air"));
        // Renames under the same runtime_id drop the old name.
        registry.upsert(ItemDefinition::new(3, "minecraft:granite"));
        assert_eq!(registry.runtime_id_by_name("minecraft:granite"), Some(3));
        assert_eq!(registry.runtime_id_by_name("minecraft:stone"), None);
    }

    #[test]
    fn register_rejects_duplicate_runtime_id() {
        let registry = ItemRegistry::new();
        assert!(registry
            .register(ItemDefinition::new(3, "minecraft:stone"))
            .is_ok());
        assert!(registry
            .register(ItemDefinition::new(3, "minecraft:granite"))
            .is_err());
    }
}
