//! 服务端物品堆与玩家库存。
//!
//! Server-side item stack and player inventory.
//!
//! 本模块持有移动侧物品模型：[`ItemStack`] 是交互、拾取与容器移动中
//! 流转的"货币"，[`PlayerInventory`] 是权威主库存（0..36；快捷栏 0..9）。
//! 注册表知识位于 [`crate::registry`]。
//!
//! This module holds the movement-side item model: [`ItemStack`] is the
//! currency that flows through interactions, pickups and container moves,
//! and [`PlayerInventory`] is the authoritative main inventory (0..36;
//! hotbar is 0..9). Registry knowledge lives in [`crate::registry`].

use crate::registry::ItemRegistry;
use parking_lot::RwLock;
use sc_ecs::component::Component;
use std::sync::Arc;

/// 可堆叠的服务端物品。
///
/// A stackable server-side item.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ItemStack {
    /// Bedrock 网络物品 runtime id；0 表示空气。
    ///
    /// Bedrock network item runtime id; 0 means air.
    pub runtime_id: u16,
    pub count: u16,
    pub damage: u32,
    /// 关联的默认方块 network runtime id。并非每个物品都映射到方块。
    ///
    /// Associated default block network runtime id. Not every item maps to a block.
    pub block_runtime_id: u32,
    /// 物品 NBT 复合标签（写时复制 Arc）。
    ///
    /// Item NBT compound tag (copy-on-write Arc).
    pub tag: Option<Arc<sc_nbt::CompoundNbt>>,
}

impl ItemStack {
    /// 空物品堆常量。
    ///
    /// The empty item stack constant.
    pub const EMPTY: Self = Self {
        runtime_id: 0,
        count: 0,
        damage: 0,
        block_runtime_id: 0,
        tag: None,
    };

    /// 返回空物品堆。
    ///
    /// Returns an empty item stack.
    pub fn empty() -> Self {
        Self::EMPTY
    }

    /// 以 runtime id 与数量构造物品堆。
    ///
    /// - `runtime_id`: 网络 runtime id
    /// - `count`: 数量
    ///
    /// Constructs an item stack from a runtime id and count.
    ///
    /// - `runtime_id`: network runtime id
    /// - `count`: item count
    pub fn new(runtime_id: u16, count: u16) -> Self {
        Self {
            runtime_id,
            count,
            ..Self::EMPTY
        }
    }

    /// 以 runtime id、方块 runtime id 与数量构造物品堆。
    ///
    /// - `runtime_id`: 网络 runtime id
    /// - `block_runtime_id`: 关联方块 runtime id
    /// - `count`: 数量
    ///
    /// Constructs an item stack from a runtime id, block runtime id and count.
    ///
    /// - `runtime_id`: network runtime id
    /// - `block_runtime_id`: associated block runtime id
    /// - `count`: item count
    pub fn with_block(runtime_id: u16, block_runtime_id: u32, count: u16) -> Self {
        Self {
            runtime_id,
            block_runtime_id,
            count,
            damage: 0,
            tag: None,
        }
    }

    /// 读取物品 NBT。
    pub fn tag(&self) -> Option<&sc_nbt::CompoundNbt> {
        self.tag.as_deref()
    }

    /// 获取或创建可变物品 NBT（COW 语义）。
    pub fn tag_mut(&mut self) -> &mut sc_nbt::CompoundNbt {
        let arc = self.tag.get_or_insert_with(|| Arc::new(sc_nbt::CompoundNbt::new(None)));
        Arc::make_mut(arc)
    }

    /// 读取强类型 NBT 切片。
    pub fn facet<F: sc_nbt::NbtFacet>(&self) -> Option<F> {
        self.tag.as_ref().and_then(|t| F::read_from(t))
    }

    /// 写入强类型 NBT 切片。
    pub fn set_facet<F: sc_nbt::NbtFacet>(&mut self, facet: &F) {
        facet.write_to(self.tag_mut());
    }

    /// 原地变更强类型 NBT 切片。
    pub fn mutate_facet<F: sc_nbt::NbtFacet + Default>(&mut self, f: impl FnOnce(&mut F)) {
        let mut val = self.facet::<F>().unwrap_or_default();
        f(&mut val);
        self.set_facet(&val);
    }

    /// 移除强类型 NBT 切片。
    pub fn remove_facet<F: sc_nbt::NbtFacet>(&mut self) {
        if let Some(tag) = self.tag.as_mut() {
            F::remove_from(Arc::make_mut(tag));
        }
    }

    /// 是否为空（runtime id 为 0 或数量为 0）。
    ///
    /// Whether empty (runtime id 0 or count 0).
    pub fn is_empty(&self) -> bool {
        self.runtime_id == 0 || self.count == 0
    }

    /// 若为空则整体归一为 `EMPTY`。
    ///
    /// Normalizes to `EMPTY` as a whole when empty.
    pub fn clear_if_empty(&mut self) {
        if self.is_empty() {
            *self = Self::EMPTY;
        }
    }
}

/// 玩家主库存（0..36；快捷栏 0..9）。
///
/// Player main inventory (0..36; hotbar is 0..9).
#[derive(Component, Clone, Debug)]
pub struct PlayerInventory {
    /// 槽位列表（`Arc<RwLock>` 共享）。
    ///
    /// Slot list (shared via `Arc<RwLock>`).
    pub slots: Arc<RwLock<Vec<ItemStack>>>,
    /// 当前选中槽。
    ///
    /// Currently selected slot.
    pub selected_slot: Arc<RwLock<u8>>,
}

impl Default for PlayerInventory {
    /// 默认 36 格主库存。
    ///
    /// Default 36-slot main inventory.
    fn default() -> Self {
        Self::new(36)
    }
}

impl PlayerInventory {
    /// 以指定槽位数创建库存。
    ///
    /// - `size`: 槽位数量
    ///
    /// Creates an inventory with the given slot count.
    ///
    /// - `size`: slot count
    pub fn new(size: usize) -> Self {
        Self {
            slots: Arc::new(RwLock::new(vec![ItemStack::empty(); size])),
            selected_slot: Arc::new(RwLock::new(0)),
        }
    }

    /// 槽位数量。
    ///
    /// Slot count.
    pub fn len(&self) -> usize {
        self.slots.read().len()
    }
    /// 是否无槽位。
    ///
    /// Whether there are no slots.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 当前选中槽。
    ///
    /// Currently selected slot.
    pub fn selected_slot(&self) -> u8 {
        *self.selected_slot.read()
    }

    /// 设置选中槽；越界返回 `false`。
    ///
    /// - `slot`: 目标槽位
    ///
    /// Sets the selected slot; returns `false` when out of bounds.
    ///
    /// - `slot`: target slot index
    pub fn set_selected_slot(&self, slot: u8) -> bool {
        let mut selected = self.selected_slot.write();
        if slot as usize >= self.len() {
            return false;
        }
        *selected = slot;
        true
    }

    /// 读取指定槽（越界返回 `None`）。
    ///
    /// - `slot`: 槽位下标
    ///
    /// Reads a slot (`None` when out of bounds).
    ///
    /// - `slot`: slot index
    pub fn get(&self, slot: usize) -> Option<ItemStack> {
        self.slots.read().get(slot).cloned()
    }

    /// 覆写指定槽，返回旧值（越界返回 `None`）。
    ///
    /// - `slot`: 槽位下标
    /// - `stack`: 新物品堆
    ///
    /// Overwrites a slot and returns the old value (`None` when out of bounds).
    ///
    /// - `slot`: slot index
    /// - `stack`: new item stack
    pub fn set(&self, slot: usize, stack: ItemStack) -> Option<ItemStack> {
        let mut slots = self.slots.write();
        let target = slots.get_mut(slot)?;
        let old = std::mem::replace(target, stack);
        target.clear_if_empty();
        Some(old)
    }

    /// 从槽中预留一个物品；调用方在方块写入成功后保留变更，
    /// 失败时用 `set(slot, old_stack)` 回滚。
    ///
    /// - `slot`: 槽位下标
    ///
    /// Reserves one item from a slot; callers keep the change after a successful block write,
    /// or roll back with `set(slot, old_stack)` on failure.
    ///
    /// - `slot`: slot index
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

    /// 把一个此前预留的物品还回其槽。
    ///
    /// 这是增量式的：同一槽上多个待落地放置不会把整堆重复归还多次。
    ///
    /// - `slot`: 槽位下标
    /// - `item`: 待归还的物品（取其中 1 个）
    ///
    /// Return one previously reserved item to its slot.
    ///
    /// This is incremental so multiple pending placements on one slot cannot
    /// restore the entire stack more than once.
    ///
    /// - `slot`: slot index
    /// - `item`: item to give back (one unit of it is taken)
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
            || (stack.block_runtime_id != item.block_runtime_id
                && stack.block_runtime_id != 0
                && item.block_runtime_id != 0)
            || stack.tag != item.tag
        {
            return false;
        }
        if stack.block_runtime_id == 0 && item.block_runtime_id != 0 {
            stack.block_runtime_id = item.block_runtime_id;
        }
        let Some(count) = stack.count.checked_add(1) else {
            return false;
        };
        stack.count = count;
        true
    }

    /// 整批物品是否放得下（逐槽累积剩余容量；只有整批都放得下才为 true；
    /// 用于拾取检查）。
    ///
    /// - `incoming`: 待放入的物品批
    /// - `registry`: 物品注册表（查堆叠上限与默认方块映射）
    ///
    /// Whether a whole item batch fits (accumulates remaining capacity slot by slot,
    /// true only when the full batch fits; used for pickup checks).
    ///
    /// - `incoming`: item batch to place
    /// - `registry`: item registry (stack limits and default block mapping)
    pub fn can_add(&self, incoming: ItemStack, registry: &ItemRegistry) -> bool {
        if incoming.is_empty() {
            return true;
        }
        let max_stack = registry.max_stack_size(incoming.runtime_id);
        let default_block = registry.block_runtime_id(incoming.runtime_id).unwrap_or(0);
        let incoming_block = if incoming.block_runtime_id == 0 {
            default_block
        } else {
            incoming.block_runtime_id
        };
        let mut remaining = incoming.count;
        for slot in self.slots.read().iter() {
            let slot_block = if slot.block_runtime_id == 0 && slot.runtime_id == incoming.runtime_id {
                default_block
            } else {
                slot.block_runtime_id
            };
            if slot.runtime_id == incoming.runtime_id
                && slot.damage == incoming.damage
                && slot_block == incoming_block
                && slot.tag == incoming.tag
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

    /// 尝试把物品合并进库存，返回（剩余数量, 发生变化的槽列表）。
    ///
    /// 先合并同物品的既有堆，再填空槽；每次槽写入都被槽粒度同步观察到，
    /// 调用方可按槽发包。记录实际发生变化的下标供逐槽同步。
    ///
    /// - `incoming`: 待放入的物品
    /// - `registry`: 物品注册表
    ///
    /// Tries to merge items into the inventory, returning (leftover count, changed slot list).
    ///
    /// Merges into existing stacks of the same item first, then fills empty slots; each
    /// slot write is observed by slot-granular sync so callers can send per-slot packets.
    /// Records the indices that actually changed so callers can sync per slot.
    ///
    /// - `incoming`: items to place
    /// - `registry`: item registry
    pub fn insert_tracked(
        &self,
        mut incoming: ItemStack,
        registry: &ItemRegistry,
    ) -> (u16, Vec<usize>) {
        let mut changed = Vec::new();
        if incoming.is_empty() {
            return (0, changed);
        }
        let default_block = registry.block_runtime_id(incoming.runtime_id).unwrap_or(0);
        if incoming.block_runtime_id == 0 && default_block != 0 {
            incoming.block_runtime_id = default_block;
        }
        let max_stack = registry.max_stack_size(incoming.runtime_id);
        let mut slots = self.slots.write();
        for (slot_index, slot) in slots.iter_mut().enumerate() {
            if incoming.is_empty() {
                break;
            }
            if !slot.is_empty()
                && slot.runtime_id == incoming.runtime_id
                && slot.block_runtime_id == 0
                && default_block != 0
            {
                slot.block_runtime_id = default_block;
                changed.push(slot_index);
            }
            if slot.runtime_id == incoming.runtime_id
                && slot.damage == incoming.damage
                && slot.block_runtime_id == incoming.block_runtime_id
                && slot.tag == incoming.tag
            {
                let room = max_stack.saturating_sub(slot.count);
                let moved = room.min(incoming.count);
                if moved > 0 {
                    slot.count += moved;
                    incoming.count -= moved;
                    if !changed.contains(&slot_index) {
                        changed.push(slot_index);
                    }
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

    /// 尝试把物品合并进库存，返回剩余数量。
    ///
    /// - `incoming`: 待放入的物品
    /// - `registry`: 物品注册表
    ///
    /// Tries to merge items into the inventory, returning the leftover count.
    ///
    /// - `incoming`: items to place
    /// - `registry`: item registry
    pub fn insert(&self, incoming: ItemStack, registry: &ItemRegistry) -> u16 {
        self.insert_tracked(incoming, registry).0
    }

    /// 导出全部槽位快照。
    ///
    /// Exports a snapshot of all slots.
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

    #[test]
    fn stack_merges_across_zero_and_default_block_runtime_id() {
        let registry = ItemRegistry::new();
        // Torch (id=50) has default_block_runtime_id = 1523
        registry.upsert(ItemDefinition::new(50, "minecraft:torch").max_stack_size(64).block(1523));
        let inventory = PlayerInventory::new(1);

        // Case 1: Slot has block_runtime_id=1523 (crafted), incoming has block_runtime_id=0 (dropped)
        inventory.set(0, ItemStack::with_block(50, 1523, 4));
        let unassigned = ItemStack::new(50, 2); // block_runtime_id = 0
        assert!(inventory.can_add(unassigned.clone(), &registry));
        let leftover = inventory.insert(unassigned, &registry);
        assert_eq!(leftover, 0);
        let s0 = inventory.get(0).unwrap();
        assert_eq!(s0.count, 6);
        assert_eq!(s0.block_runtime_id, 1523);

        // Case 2: Slot has block_runtime_id=0, incoming has block_runtime_id=1523
        inventory.set(1, ItemStack::new(50, 3)); // block_runtime_id = 0
        let crafted = ItemStack::with_block(50, 1523, 5);
        let (leftover, changed) = inventory.insert_tracked(crafted, &registry);
        assert_eq!(leftover, 0);
        assert!(changed.contains(&0));
        let s0 = inventory.get(0).unwrap();
        // Slot 0 absorbs up to max_stack (64), so it becomes 6 + 5 = 11
        assert_eq!(s0.count, 11);
        assert_eq!(s0.block_runtime_id, 1523);
    }
}


#[cfg(test)]
mod facet_tests {
    use super::*;
    use crate::facets::{DamageFacet, DisplayFacet};

    #[test]
    fn item_stack_facet_mutation_and_read() {
        let mut stack = ItemStack::new(1, 1);
        assert!(stack.facet::<DamageFacet>().is_none());

        stack.set_facet(&DamageFacet { damage: 10 });
        assert_eq!(stack.facet::<DamageFacet>(), Some(DamageFacet { damage: 10 }));

        stack.mutate_facet::<DisplayFacet>(|disp| {
            disp.name = Some("Custom Blade".into());
            disp.lore.push("Legendary".into());
        });

        let disp = stack.facet::<DisplayFacet>().unwrap();
        assert_eq!(disp.name.as_deref(), Some("Custom Blade"));
        assert_eq!(disp.lore, vec!["Legendary"]);

        // DamageFacet still exists after mutating DisplayFacet
        assert_eq!(stack.facet::<DamageFacet>(), Some(DamageFacet { damage: 10 }));

        // Removing DamageFacet
        stack.remove_facet::<DamageFacet>();
        assert!(stack.facet::<DamageFacet>().is_none());
        assert!(stack.facet::<DisplayFacet>().is_some());
    }

    #[test]
    fn stacks_with_different_tags_do_not_merge() {
        let registry = ItemRegistry::new();
        registry.upsert(crate::registry::ItemDefinition::new(1, "minecraft:stone").max_stack_size(64));

        let inventory = PlayerInventory::new(1);
        let mut s1 = ItemStack::new(1, 1);
        s1.set_facet(&DamageFacet { damage: 5 });
        let mut s2 = ItemStack::new(1, 1);
        s2.set_facet(&DamageFacet { damage: 10 });

        inventory.set(0, s1);
        assert!(!inventory.can_add(s2.clone(), &registry));
    }
}
