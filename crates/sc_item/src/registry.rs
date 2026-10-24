//! 物品注册表：版本包通过它把物品关联到默认方块状态。
//!
//! Item registry: version packs link items to default block states through it.

use parking_lot::RwLock;
use sc_ecs::resource::Resource;
use std::collections::HashMap;
use std::sync::Arc;

/// 物品注册表条目。版本包通过它把物品关联到默认方块状态。
///
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

/// 可由版本包动态填充的物品注册表。
///
/// 主表是按网络 runtime id 索引的稠密表（runtime id 连续 0..N，同时作为网络 id
/// 和数组下标）；`by_name` 把标识符解析为 runtime id，供方块掉落与工具判定
/// （`sc:mining.tools[].items` / 掉落表条目）使用。
///
/// Item registry fillable dynamically by version packs.
///
/// Dense primary table indexed by network runtime id (runtime ids are contiguous 0..N, serving as both
/// network ids and array indices); `by_name` resolves identifier to runtime for block
/// drops/tools (`sc:mining.tools[].items` / loot-table drop entries).
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

    /// 登记名称到 runtime 的反向查找；改名保留较小的 runtime id。
    ///
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

    /// 物品标识符到 runtime id（供 `sc:mining` 与掉落表解析）。
    ///
    /// Item identifier to runtime id (for `sc:mining` and loot-table drops resolution).
    pub fn runtime_id_by_name(&self, name: &str) -> Option<u16> {
        self.by_name.read().get(name).copied()
    }

    /// 该物品标识符是否已注册。
    ///
    /// Whether this item identifier is registered.
    pub fn contains_name(&self, name: &str) -> bool {
        self.by_name.read().contains_key(name)
    }

    /// 全部已注册标识符（用于构建配方伪名家族表的启动快照）。
    ///
    /// All registered identifiers (one startup snapshot for building recipe pseudo-name family tables).
    pub fn names(&self) -> Vec<String> {
        self.by_name.read().keys().map(|key| key.to_string()).collect()
    }

    /// 按 runtime_id 直接索引主表（不做哈希查找）。
    ///
    /// Looks up by direct `runtime_id` indexing of the dense table (no hashing).
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

    /// 为某物品创建标准堆叠，若其代表方块则自动填入默认方块 runtime id。
    ///
    /// Create a standard stack for an item, automatically populating its default block runtime id if it represents a block.
    pub fn create_stack(&self, runtime_id: u16, count: u16) -> crate::stack::ItemStack {
        let block_runtime_id = self.block_runtime_id(runtime_id).unwrap_or(0);
        crate::stack::ItemStack::with_block(runtime_id, block_runtime_id, count)
    }

    pub fn max_stack_size(&self, runtime_id: u16) -> u16 {
        self.get(runtime_id)
            .map(|entry| entry.max_stack_size)
            .unwrap_or(64)
    }

    /// 已注册物品数量（跳过空洞索引）。
    ///
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

    /// 全部"方块物品"定义（带 default_block_runtime_id 的物品），按 runtime_id 升序。
    ///
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_block_mapping_is_data_driven() {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(5, "minecraft:stone").block(123));
        assert_eq!(registry.block_runtime_id(5), Some(123));
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
