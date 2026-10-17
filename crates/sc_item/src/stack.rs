//! Server-side item stack and player inventory.
//!
//! This module holds the movement-side item model: [`ItemStack`] is the
//! currency that flows through interactions, pickups and container moves,
//! and [`PlayerInventory`] is the authoritative main inventory (0..36;
//! hotbar is 0..9). Registry knowledge lives in [`crate::registry`].

use crate::registry::ItemRegistry;
use parking_lot::RwLock;
use sc_ecs::component::Component;
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
    /// slot write is observed by slot-granular sync so callers can send per-slot packets.
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
    use crate::registry::ItemDefinition;

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
    fn restore_one_only_restores_a_single_reserved_item() {
        let inventory = PlayerInventory::new(1);
        inventory.set(0, ItemStack::new(1, 2));
        let reserved = inventory.reserve_one(0).unwrap();
        assert_eq!(inventory.get(0).unwrap().count, 1);
        assert!(inventory.restore_one(0, &ItemStack {
            count: 1,
            ..reserved
        }));
        assert_eq!(inventory.get(0).unwrap().count, 2);
    }

    #[test]
    fn item_stack_is_empty_and_serializes_semantics() {
        // A stack with 0 items is empty, no matter what the id is.
        let stack = ItemStack::new(3, 0);
        assert!(stack.is_empty());
        let mut mutable = stack;
        mutable.clear_if_empty();
        assert_eq!(mutable, ItemStack::EMPTY);
    }

    #[test]
    fn selected_slot_bounds_are_enforced() {
        let inventory = PlayerInventory::new(9);
        assert!(inventory.set_selected_slot(8));
        assert_eq!(inventory.selected_slot(), 8);
        assert!(!inventory.set_selected_slot(9));
        assert_eq!(inventory.selected_slot(), 8);
    }
}
