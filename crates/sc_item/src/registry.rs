//! Item registry: version packs link items to default block states through it.

use parking_lot::RwLock;
use sc_ecs::resource::Resource;
use std::collections::HashMap;
use std::sync::Arc;

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
