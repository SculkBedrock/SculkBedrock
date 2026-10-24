//! 方块实体的通用容器背包组件。
//!
//! Generic container inventory component for block entities.

use sc_ecs::component::Component;
use sc_item::ItemStack;

/// 可复用的方块实体背包容器组件。
///
/// Reusable inventory container component for block entities.
///
/// 用于支撑标准 Minecraft 容器：
/// Designed to back standard Minecraft containers:
/// - 单箱子：27 槽；
///   Single Chest: 27 slots
/// - 大箱子：54 槽；
///   Double Chest: 54 slots
/// - 发射器/投掷器：9 槽；
///   Dispenser / Dropper: 9 slots
/// - 漏斗：5 槽。
///   Hopper: 5 slots
#[derive(Component, Clone, Debug, Default)]
pub struct BlockInventory {
    /// 槽位物品栈列表。
    ///
    /// Slot item stacks.
    pub slots: Vec<ItemStack>,
    /// 单槽最大堆叠数。
    ///
    /// Maximum stack size per slot.
    pub max_stack_size: u16,
}

impl BlockInventory {
    /// 创建有 `size` 个空槽的新方块背包。
    ///
    /// Creates a new block inventory with `size` empty slots.
    ///
    /// - `size`: 槽位数
    pub fn new(size: usize) -> Self {
        Self {
            slots: vec![ItemStack::empty(); size],
            max_stack_size: 64,
        }
    }

    /// 创建自定义最大堆叠数的新方块背包。
    ///
    /// Creates a new block inventory with custom max stack size.
    ///
    /// - `size`: 槽位数
    /// - `max_stack_size`: 单槽最大堆叠数
    pub fn with_max_stack(size: usize, max_stack_size: u16) -> Self {
        Self {
            slots: vec![ItemStack::empty(); size],
            max_stack_size,
        }
    }

    /// 槽位数量。
    ///
    /// Slot count.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// 背包是否有 0 个槽。
    ///
    /// Whether the inventory has 0 slots.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 是否所有槽位都为空。
    ///
    /// Whether all slots in the inventory are empty.
    pub fn is_all_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_empty())
    }

    /// 获取槽位的不可变引用。
    ///
    /// Get slot immutable reference.
    ///
    /// - `index`: 槽位索引
    pub fn get(&self, index: usize) -> Option<&ItemStack> {
        self.slots.get(index)
    }

    /// 获取槽位的可变引用。
    ///
    /// Get slot mutable reference.
    ///
    /// - `index`: 槽位索引
    pub fn get_mut(&mut self, index: usize) -> Option<&mut ItemStack> {
        self.slots.get_mut(index)
    }

    /// 设置槽位内容；索引越界时返回 `false`。
    ///
    /// Set slot content. Returns `true` if index was within bounds.
    ///
    /// - `index`: 槽位索引
    /// - `stack`: 物品栈
    pub fn set(&mut self, index: usize, stack: ItemStack) -> bool {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = stack;
            true
        } else {
            false
        }
    }

    /// 取走全部非空物品栈（用于方块破坏掉落生成）。
    ///
    /// Drain all non-empty stacks (useful for block break drop generation).
    pub fn drain_non_empty(&mut self) -> Vec<ItemStack> {
        let mut result = Vec::new();
        for slot in &mut self.slots {
            if !slot.is_empty() {
                result.push(std::mem::replace(slot, ItemStack::empty()));
            }
        }
        result
    }
}
