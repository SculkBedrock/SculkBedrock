use crate::entity::{EntityComponents, MinecraftEntitySpawner};
use crate::item::{ItemComponents, MinecraftItemSpawner};
use crate::version_control::json_budget::BudgetReader;
use log::debug;
use sc_binary::{ByteReader, ByteWriter};
use sc_ecs::resource::Resource;
use sc_log::t_log;
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::reader::NbtReadTrait;
use sc_nbt::NbtValue;
use sc_utils::components::RuntimeID;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;

#[derive(Deserialize, Debug, Clone)]
pub struct MinecraftRuntimeJson {
    pub name: String,
    pub id: i16,
    pub version: i32,
    #[serde(rename = "componentBased")]
    pub component_based: bool,
}

#[derive(Clone, Hash, Eq, PartialEq)]
enum RuntimeIdentifier {
    Int(i16),
    String(Arc<str>),
}

/// Creative group icon in creative_items.json (item id + optional block_state_b64).
#[derive(Deserialize, Debug, Clone, Default)]
pub struct CreativeIconDef {
    pub id: String,
    #[serde(default, rename = "block_state_b64")]
    pub block_state_b64: Option<String>,
}

/// Creative group definition in creative_items.json (creative_category: 1=construction,
/// 2=equipment, 3=item, 4=nature, matching the CreativeItemCategory enum).
#[derive(Deserialize, Debug, Clone, Default)]
pub struct CreativeGroupDef {
    #[serde(default)]
    pub creative_category: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: Option<CreativeIconDef>,
}

/// Creative item entry in creative_items.json (group_index indexes the groups array).
#[derive(Deserialize, Debug, Clone, Default)]
pub struct CreativeItemDef {
    pub id: String,
    #[serde(default, rename = "group_index")]
    pub group_index: i64,
    #[serde(default)]
    pub damage: i32,
    #[serde(default, rename = "nbt_b64")]
    pub nbt_b64: Option<String>,
}

/// Top-level creative_items.json structure (groups + items),
/// loaded from the shared gamedata creative_items.json source.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct CreativeItemsData {
    #[serde(default)]
    pub groups: Vec<CreativeGroupDef>,
    #[serde(default)]
    pub items: Vec<CreativeItemDef>,
}

/// Runtime registry: mutable during load, frozen into a shared snapshot after SCLoad.
/// Cloning on the login path drops from a multi-MB deep copy to a few Arc refcounts.
#[derive(Resource, Clone)]
pub struct MinecraftRuntimeManager {
    runtime_map: Arc<HashMap<Arc<str>, i16>>,
    runtime_order: Arc<Vec<Arc<str>>>,
    // Whether the item is component-based (from the runtime.json componentBased field)
    component_based_map: Arc<HashMap<Arc<str>, bool>>,
    // Item data version (from the runtime.json version field, used by the ItemComponent packet)
    version_map: Arc<HashMap<Arc<str>, i32>>,
    item_content_map: Arc<HashMap<RuntimeIdentifier, MinecraftItemSpawner>>,
    entity_content_map: Arc<HashMap<RuntimeIdentifier, MinecraftEntitySpawner>>,
    item_palette: Option<Arc<Vec<u8>>>,
    /// Vanilla creative-item table (creative_items.json, optional)
    creative_items: Option<Arc<CreativeItemsData>>,
    /// Vanilla entity-identifier NBT, parsed directly from the entity_identifiers.dat file
    /// Holds the full vanilla entity identifier set loaded from the resource file
    entity_network_nbt: Option<Arc<NbtValue>>,
}

impl MinecraftRuntimeManager {
    pub fn new(json: Vec<MinecraftRuntimeJson>) -> MinecraftRuntimeManager {
        let mut runtime_map = HashMap::new();
        let mut runtime_order = Vec::new();
        let mut component_based_map = HashMap::new();
        let mut version_map = HashMap::new();
        for json in json {
            let name: Arc<str> = Arc::from(json.name);
            if !runtime_map.contains_key(&name) {
                runtime_order.push(Arc::clone(&name));
            }
            runtime_map.insert(Arc::clone(&name), json.id);
            component_based_map.insert(Arc::clone(&name), json.component_based);
            version_map.insert(name, json.version);
        }
        Self {
            runtime_map: Arc::new(runtime_map),
            runtime_order: Arc::new(runtime_order),
            component_based_map: Arc::new(component_based_map),
            version_map: Arc::new(version_map),
            item_content_map: Arc::new(HashMap::new()),
            entity_content_map: Arc::new(HashMap::new()),
            item_palette: None,
            creative_items: None,
            entity_network_nbt: None,
        }
    }

    pub fn push_runtime(&mut self, runtime: MinecraftRuntimeJson) {
        let name: Arc<str> = Arc::from(runtime.name);
        let runtime_order = Arc::make_mut(&mut self.runtime_order);
        let runtime_map = Arc::make_mut(&mut self.runtime_map);
        if !runtime_map.contains_key(&name) {
            runtime_order.push(Arc::clone(&name));
        }
        runtime_map.insert(Arc::clone(&name), runtime.id);
        Arc::make_mut(&mut self.component_based_map)
            .insert(Arc::clone(&name), runtime.component_based);
        Arc::make_mut(&mut self.version_map).insert(name, runtime.version);
    }

    pub fn get_runtime_id(&self, identifier: &str) -> Option<i16> {
        self.runtime_map.get(identifier).cloned()
    }

    fn ordered_runtime_names(&self) -> Vec<Arc<str>> {
        let mut seen = HashSet::new();
        let mut names = Vec::with_capacity(self.runtime_map.len());

        for name in self.runtime_order.iter() {
            if self.runtime_map.contains_key(name) && seen.insert(name.clone()) {
                names.push(name.clone());
            }
        }

        let mut missing = self
            .runtime_map
            .keys()
            .filter(|name| !seen.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        missing.sort_by(|a, b| {
            self.runtime_map
                .get(a)
                .cmp(&self.runtime_map.get(b))
                .then_with(|| a.cmp(b))
        });
        names.extend(missing);
        names
    }

    /// Runtime entries in version-pack palette order. Both the
    /// StartGame item palette and ItemRegistry use the item_palette.json order, so
    /// callers should not sort this again unless they intentionally diverge.
    pub fn runtime_entries(&self) -> Vec<(String, i16)> {
        self.ordered_runtime_names()
            .into_iter()
            .filter_map(|name| {
                self.runtime_map
                    .get(&name)
                    .copied()
                    .map(|id| (name.to_string(), id))
            })
            .collect()
    }

    pub fn get_runtime_ident(&self, runtime_id: i16) -> Option<String> {
        self.runtime_map
            .iter()
            .find(|(_, &v)| v == runtime_id)
            .map(|(k, _)| k.to_string())
    }

    pub fn get_item_version(&self, identifier: &str) -> Option<i32> {
        self.version_map.get(identifier).cloned()
    }

    pub fn is_component_based(&self, identifier: &str) -> Option<bool> {
        self.component_based_map.get(identifier).cloned()
    }

    pub fn push_entity_map(&mut self, map: HashMap<String, MinecraftEntitySpawner>) {
        let entity_content_map = Arc::make_mut(&mut self.entity_content_map);
        for (identifier, mut content) in map {
            let mut ident = RuntimeIdentifier::String(Arc::from(identifier.as_str()));
            if let Some(runtime_id) = self.runtime_map.get(identifier.as_str()).copied() {
                if let Some(components) = content.components.as_mut() {
                    components.push(RuntimeID(runtime_id));
                } else {
                    content.components = EntityComponents::new_single(RuntimeID(runtime_id));
                }
                ident = RuntimeIdentifier::Int(runtime_id);
            }
            entity_content_map.insert(ident, content);
        }
    }

    pub fn push_item_map(&mut self, map: HashMap<String, MinecraftItemSpawner>) {
        let item_content_map = Arc::make_mut(&mut self.item_content_map);
        for (identifier, mut content) in map {
            let mut ident = RuntimeIdentifier::String(Arc::from(identifier.as_str()));
            if let Some(runtime_id) = self.runtime_map.get(identifier.as_str()).copied() {
                if let Some(components) = content.components.as_mut() {
                    components.push(RuntimeID(runtime_id));
                } else {
                    content.components = ItemComponents::new_single(RuntimeID(runtime_id));
                }
                ident = RuntimeIdentifier::Int(runtime_id);
            }

            item_content_map.insert(ident, content);
        }
    }

    pub fn get_entity_spawner(&self, runtime_id: i16) -> Option<&MinecraftEntitySpawner> {
        self.entity_content_map
            .get(&RuntimeIdentifier::Int(runtime_id))
    }

    pub fn get_entity_spawner_by_ident(&self, identifier: &str) -> Option<&MinecraftEntitySpawner> {
        if let Some(runtime_id) = self.runtime_map.get(identifier) {
            self.entity_content_map
                .get(&RuntimeIdentifier::Int(*runtime_id))
        } else {
            self.entity_content_map
                .get(&RuntimeIdentifier::String(Arc::from(identifier)))
        }
    }

    pub fn get_item_spawner(&self, runtime_id: i16) -> Option<&MinecraftItemSpawner> {
        self.item_content_map
            .get(&RuntimeIdentifier::Int(runtime_id))
    }

    pub fn get_item_spawner_by_ident(&self, identifier: &str) -> Option<&MinecraftItemSpawner> {
        if let Some(runtime_id) = self.runtime_map.get(identifier) {
            self.item_content_map
                .get(&RuntimeIdentifier::Int(*runtime_id))
        } else {
            self.item_content_map
                .get(&RuntimeIdentifier::String(Arc::from(identifier)))
        }
    }

    pub fn get_entity_idents(&self) -> Vec<String> {
        let mut idents = Vec::new();
        for (ident, _) in self.entity_content_map.iter() {
            match ident {
                RuntimeIdentifier::Int(id) => {
                    if let Some(ident) = self.get_runtime_ident(*id) {
                        idents.push(ident.clone());
                    }
                }
                RuntimeIdentifier::String(ident) => {
                    idents.push(ident.to_string());
                }
            }
        }
        idents
    }

    pub fn entities_len(&self) -> usize {
        self.entity_content_map.len()
    }

    pub fn items_len(&self) -> usize {
        self.item_content_map.len()
    }

    pub fn generate_palette(&mut self) -> io::Result<()> {
        debug!("RuntimeManager: generating palette...");
        let mut palette_bytes = ByteWriter::new();
        palette_bytes.write_var_u32(self.runtime_map.len() as u32)?;
        for ident in self.ordered_runtime_names() {
            let Some(id) = self.runtime_map.get(&ident) else {
                continue;
            };
            palette_bytes.write_string(&ident)?;
            palette_bytes.write_i16_le(*id)?;
            // Use the runtime.json componentBased field instead of a fixed false
            let component_based = self
                .component_based_map
                .get(&ident)
                .copied()
                .unwrap_or(false);
            palette_bytes.write_bool(component_based)?;
        }
        let palette_bytes = palette_bytes.as_slice();
        self.item_palette = Some(Arc::new(palette_bytes.to_vec()));
        Ok(())
    }

    /// Load the vanilla creative-item table from definitions/creative_items.json bytes
    /// (Kaooot format, shared gamedata source).
    pub fn load_creative_items(&mut self, json_bytes: Vec<u8>) -> bool {
        match serde_json::from_reader::<_, CreativeItemsData>(BudgetReader::new(
            json_bytes.as_slice(),
        )) {
            Ok(data) => {
                debug!(
                    "RuntimeManager: loaded creative items: {} groups, {} items",
                    data.groups.len(),
                    data.items.len()
                );
                self.creative_items = Some(Arc::new(data));
                true
            }
            Err(error) => {
                log::warn!("{}", t_log!("console.pack.runtime_creative_fail", error = error));
                false
            }
        }
    }

    pub fn creative_items(&self) -> Option<&CreativeItemsData> {
        self.creative_items.as_deref()
    }

    /// Load vanilla entity-identifier NBT from entity_identifiers.dat file bytes
    /// Loads the full vanilla entity definitions directly from the file:
    ///   - read entity_identifiers.dat (contains the full vanilla entity definitions)
    ///   - use the NBT data from the file directly, no manual build
    pub fn load_entity_identifiers(&mut self, dat_bytes: Vec<u8>) -> io::Result<()> {
        debug!("RuntimeManager: loading entity identifiers NBT from file...");
        let mut reader = ByteReader::from(dat_bytes);
        let nbt = BedrockNetworkNbt::read(&mut reader)?;
        self.entity_network_nbt = Some(Arc::new(nbt));
        Ok(())
    }

    /// Palette bytes (shared Arc clone; StartGame no longer serializes this field,
    /// kept only for existence checks and later ItemRegistry packet reuse).
    pub fn get_palette(&self) -> Option<Arc<Vec<u8>>> {
        self.item_palette.clone()
    }

    /// Vanilla entity-identifier NBT (shared Arc clone, avoids deep-copying the whole tree).
    pub fn get_entity_network_nbt(&self) -> Option<Arc<NbtValue>> {
        self.entity_network_nbt.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(name: &str, id: i16) -> MinecraftRuntimeJson {
        MinecraftRuntimeJson {
            name: name.to_string(),
            id,
            version: 1,
            component_based: false,
        }
    }

    #[test]
    fn runtime_entries_preserve_version_pack_order() {
        let mut manager = MinecraftRuntimeManager::new(vec![
            runtime("minecraft:stone", 1),
            runtime("minecraft:air", 0),
            runtime("minecraft:acacia_button", -140),
        ]);

        manager.push_runtime(runtime("minecraft:apple", 257));
        manager.push_runtime(runtime("minecraft:stone", 1));

        let entries = manager.runtime_entries();
        let names = entries
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "minecraft:stone",
                "minecraft:air",
                "minecraft:acacia_button",
                "minecraft:apple",
            ]
        );
    }
}
