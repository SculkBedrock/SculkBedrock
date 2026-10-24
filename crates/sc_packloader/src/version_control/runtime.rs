//! 运行时注册表：runtime id ↔ 标识符映射、物品/实体内容与创造模式物品表。
//!
//! Runtime registry: runtime id ↔ identifier mapping, item/entity content,
//! and the creative item table.

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

/// runtime.json 单条条目：标识名 + runtime id + 数据版本 + 是否组件化。
///
/// One runtime.json entry: identifier plus runtime id, data version, and
/// whether it is component-based.
#[derive(Deserialize, Debug, Clone)]
pub struct MinecraftRuntimeJson {
    /// 标识名。
    ///
    /// Identifier.
    pub name: String,
    /// runtime id。
    ///
    /// Runtime id.
    pub id: i16,
    /// 数据版本。
    ///
    /// Data version.
    pub version: i32,
    /// 是否组件化（JSON 键 `componentBased`）。
    ///
    /// Whether component-based (JSON key `componentBased`).
    #[serde(rename = "componentBased")]
    pub component_based: bool,
}

/// 内容表键：有 runtime id 时用数值键，否则用字符串标识键。
///
/// Content map key: the numeric key when a runtime id exists, otherwise the
/// string identifier.
#[derive(Clone, Hash, Eq, PartialEq)]
enum RuntimeIdentifier {
    /// 运行时 id 键。
    ///
    /// Runtime id key.
    Int(i16),
    /// 字符串标识键。
    ///
    /// String identifier key.
    String(Arc<str>),
}

/// creative_items.json 中的创造分组图标（物品 id + 可选 block_state_b64）。
///
/// Creative group icon in creative_items.json (item id + optional block_state_b64).
#[derive(Deserialize, Debug, Clone, Default)]
pub struct CreativeIconDef {
    /// 图标物品 id。
    ///
    /// Icon item id.
    pub id: String,
    /// 可选方块状态 base64（JSON 键 `block_state_b64`）。
    ///
    /// Optional block-state base64 (JSON key `block_state_b64`).
    #[serde(default, rename = "block_state_b64")]
    pub block_state_b64: Option<String>,
}

/// creative_items.json 中的创造分组定义（creative_category：1=建造、
/// 2=装备、3=物品、4=自然，对应 CreativeItemCategory 枚举）。
///
/// Creative group definition in creative_items.json (creative_category: 1=construction,
/// 2=equipment, 3=item, 4=nature, matching the CreativeItemCategory enum).
#[derive(Deserialize, Debug, Clone, Default)]
pub struct CreativeGroupDef {
    /// 创造类别编号。
    ///
    /// Creative category number.
    #[serde(default)]
    pub creative_category: i32,
    /// 分组名。
    ///
    /// Group name.
    #[serde(default)]
    pub name: String,
    /// 分组图标。
    ///
    /// Group icon.
    #[serde(default)]
    pub icon: Option<CreativeIconDef>,
}

/// creative_items.json 中的创造物品条目（group_index 索引 groups 数组）。
///
/// Creative item entry in creative_items.json (group_index indexes the groups array).
#[derive(Deserialize, Debug, Clone, Default)]
pub struct CreativeItemDef {
    /// 物品 id。
    ///
    /// Item id.
    pub id: String,
    /// 所属分组下标（JSON 键 `group_index`）。
    ///
    /// Owning group index (JSON key `group_index`).
    #[serde(default, rename = "group_index")]
    pub group_index: i64,
    /// 物品损伤值。
    ///
    /// Item damage value.
    #[serde(default)]
    pub damage: i32,
    /// 可选 NBT base64（JSON 键 `nbt_b64`）。
    ///
    /// Optional NBT base64 (JSON key `nbt_b64`).
    #[serde(default, rename = "nbt_b64")]
    pub nbt_b64: Option<String>,
}

/// creative_items.json 顶层结构（groups + items），
/// 加载自共享 gamedata creative_items.json 数据源。
///
/// Top-level creative_items.json structure (groups + items),
/// loaded from the shared gamedata creative_items.json source.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct CreativeItemsData {
    /// 分组列表。
    ///
    /// Group list.
    #[serde(default)]
    pub groups: Vec<CreativeGroupDef>,
    /// 物品条目列表。
    ///
    /// Item entry list.
    #[serde(default)]
    pub items: Vec<CreativeItemDef>,
}

/// 运行时注册表（ECS 资源）：加载期可变，SCLoad 后冻结为共享快照。
/// 登录路径上的克隆由数 MB 深拷贝降为几次 Arc 引用计数。
///
/// Runtime registry: mutable during load, frozen into a shared snapshot after SCLoad.
/// Cloning on the login path drops from a multi-MB deep copy to a few Arc refcounts.
#[derive(Resource, Clone)]
pub struct MinecraftRuntimeManager {
    /// 标识名 → runtime id。
    ///
    /// Identifier → runtime id.
    runtime_map: Arc<HashMap<Arc<str>, i16>>,
    /// 标识名插入顺序（palette 顺序）。
    ///
    /// Identifier insertion order (palette order).
    runtime_order: Arc<Vec<Arc<str>>>,
    /// 物品是否组件化（取自 runtime.json 的 componentBased 字段）。
    ///
    /// Whether the item is component-based (from the runtime.json componentBased field)
    component_based_map: Arc<HashMap<Arc<str>, bool>>,
    /// 物品数据版本（取自 runtime.json 的 version 字段，用于 ItemComponent 包）。
    ///
    /// Item data version (from the runtime.json version field, used by the ItemComponent packet)
    version_map: Arc<HashMap<Arc<str>, i32>>,
    /// 标识/runtime id → 物品定义。
    ///
    /// Identifier/runtime id → item definition.
    item_content_map: Arc<HashMap<RuntimeIdentifier, MinecraftItemSpawner>>,
    /// 标识/runtime id → 实体定义。
    ///
    /// Identifier/runtime id → entity definition.
    entity_content_map: Arc<HashMap<RuntimeIdentifier, MinecraftEntitySpawner>>,
    /// 物品调色板序列化字节。
    ///
    /// Serialized item palette bytes.
    item_palette: Option<Arc<Vec<u8>>>,
    /// 原版创造模式物品表（creative_items.json，可选）。
    ///
    /// Vanilla creative-item table (creative_items.json, optional)
    creative_items: Option<Arc<CreativeItemsData>>,
    /// 原版实体标识 NBT，直接解析自 entity_identifiers.dat 文件。
    /// 保存从资源文件加载的完整原版实体标识符集合。
    ///
    /// Vanilla entity-identifier NBT, parsed directly from the entity_identifiers.dat file
    /// Holds the full vanilla entity identifier set loaded from the resource file
    entity_network_nbt: Option<Arc<NbtValue>>,
}

impl MinecraftRuntimeManager {
    /// 从 runtime.json 条目列表构造注册表。
    ///
    /// Builds the registry from runtime.json entries.
    ///
    /// - `json`: runtime.json 条目列表
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

    /// 追加一条 runtime 条目（同名覆盖 id/version，保持首次插入顺序）。
    ///
    /// Appends a runtime entry (same name overwrites id/version, keeping the
    /// first insertion order).
    ///
    /// - `runtime`: 待追加的 runtime 条目
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

    /// 按标识名查 runtime id。
    ///
    /// Looks up the runtime id by identifier.
    ///
    /// - `identifier`: 物品/实体标识名
    pub fn get_runtime_id(&self, identifier: &str) -> Option<i16> {
        self.runtime_map.get(identifier).cloned()
    }

    /// 按 palette 顺序给出全部标识名（顺序表在前，缺失的按 id 排序补后）。
    ///
    /// Returns all identifiers in palette order (order table first, missing
    /// ones appended sorted by id).
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

    /// 按版本包 palette 顺序给出 runtime 条目。StartGame 物品调色板与
    /// ItemRegistry 都使用 item_palette.json 的顺序，调用方除非有意
    /// 分歧否则不应再排序。
    ///
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

    /// 按 runtime id 反查标识名。
    ///
    /// Reverse-looks up the identifier by runtime id.
    ///
    /// - `runtime_id`: 运行时 id
    pub fn get_runtime_ident(&self, runtime_id: i16) -> Option<String> {
        self.runtime_map
            .iter()
            .find(|(_, &v)| v == runtime_id)
            .map(|(k, _)| k.to_string())
    }

    /// 按标识名查物品数据版本。
    ///
    /// Looks up the item data version by identifier.
    ///
    /// - `identifier`: 物品标识名
    pub fn get_item_version(&self, identifier: &str) -> Option<i32> {
        self.version_map.get(identifier).cloned()
    }

    /// 按标识名查是否组件化。
    ///
    /// Looks up whether an identifier is component-based.
    ///
    /// - `identifier`: 物品标识名
    pub fn is_component_based(&self, identifier: &str) -> Option<bool> {
        self.component_based_map.get(identifier).cloned()
    }

    /// 合并一批实体定义；有 runtime id 的注入 RuntimeID 组件并按 id 建键。
    ///
    /// Merges a batch of entity definitions; injects the RuntimeID component
    /// and keys by id when a runtime id exists.
    ///
    /// - `map`: 实体标识名到实体定义的映射
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

    /// 合并一批物品定义；有 runtime id 的注入 RuntimeID 组件并按 id 建键。
    ///
    /// Merges a batch of item definitions; injects the RuntimeID component
    /// and keys by id when a runtime id exists.
    ///
    /// - `map`: 物品标识名到物品定义的映射
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

    /// 按 runtime id 查实体定义。
    ///
    /// Looks up an entity definition by runtime id.
    ///
    /// - `runtime_id`: 实体的运行时 id
    pub fn get_entity_spawner(&self, runtime_id: i16) -> Option<&MinecraftEntitySpawner> {
        self.entity_content_map
            .get(&RuntimeIdentifier::Int(runtime_id))
    }

    /// 按标识名查实体定义（优先 runtime id 键，回退字符串键）。
    ///
    /// Looks up an entity definition by identifier (runtime id key first,
    /// falling back to the string key).
    ///
    /// - `identifier`: 实体标识名
    pub fn get_entity_spawner_by_ident(&self, identifier: &str) -> Option<&MinecraftEntitySpawner> {
        if let Some(runtime_id) = self.runtime_map.get(identifier) {
            self.entity_content_map
                .get(&RuntimeIdentifier::Int(*runtime_id))
        } else {
            self.entity_content_map
                .get(&RuntimeIdentifier::String(Arc::from(identifier)))
        }
    }

    /// 按 runtime id 查物品定义。
    ///
    /// Looks up an item definition by runtime id.
    ///
    /// - `runtime_id`: 物品的运行时 id
    pub fn get_item_spawner(&self, runtime_id: i16) -> Option<&MinecraftItemSpawner> {
        self.item_content_map
            .get(&RuntimeIdentifier::Int(runtime_id))
    }

    /// 按标识名查物品定义（优先 runtime id 键，回退字符串键）。
    ///
    /// Looks up an item definition by identifier (runtime id key first,
    /// falling back to the string key).
    ///
    /// - `identifier`: 物品标识名
    pub fn get_item_spawner_by_ident(&self, identifier: &str) -> Option<&MinecraftItemSpawner> {
        if let Some(runtime_id) = self.runtime_map.get(identifier) {
            self.item_content_map
                .get(&RuntimeIdentifier::Int(*runtime_id))
        } else {
            self.item_content_map
                .get(&RuntimeIdentifier::String(Arc::from(identifier)))
        }
    }

    /// 列出全部实体标识名（id 键反查为标识名，字符串键原样返回）。
    ///
    /// Lists all entity identifiers (id keys reverse to identifiers, string
    /// keys pass through).
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

    /// 实体定义数量。
    ///
    /// Number of entity definitions.
    pub fn entities_len(&self) -> usize {
        self.entity_content_map.len()
    }

    /// 物品定义数量。
    ///
    /// Number of item definitions.
    pub fn items_len(&self) -> usize {
        self.item_content_map.len()
    }

    /// 生成物品调色板字节（按 palette 顺序：标识 + id + 是否组件化）。
    ///
    /// Generates the item palette bytes (palette order: identifier + id +
    /// component-based flag).
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
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

    /// 从 `definitions/creative_items.json` 字节加载原版创造模式物品表
    /// （Kaooot 格式，共享 gamedata 数据源）；成功返回 true。
    ///
    /// Load the vanilla creative-item table from definitions/creative_items.json bytes
    /// (Kaooot format, shared gamedata source); returns true on success.
    ///
    /// - `json_bytes`: JSON 文件字节
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
                log::warn!(
                    "{}",
                    t_log!("console.pack.runtime_creative_fail", error = error)
                );
                false
            }
        }
    }

    /// 获取创造模式物品表。
    ///
    /// Returns the creative item table.
    pub fn creative_items(&self) -> Option<&CreativeItemsData> {
        self.creative_items.as_deref()
    }

    /// 从 `entity_identifiers.dat` 文件字节加载原版实体标识 NBT。
    /// 直接使用文件中的完整原版实体定义，无需手工构建：
    ///   - 读取 entity_identifiers.dat（内含完整的原版实体定义）
    ///   - 直接使用文件中的 NBT 数据，不手工构建
    ///
    /// Load vanilla entity-identifier NBT from entity_identifiers.dat file bytes
    /// Loads the full vanilla entity definitions directly from the file:
    ///   - read entity_identifiers.dat (contains the full vanilla entity definitions)
    ///   - use the NBT data from the file directly, no manual build
    ///
    /// - `dat_bytes`: .dat 文件字节
    ///
    /// # Errors
    /// NBT 读取失败时返回 IO 错误。
    pub fn load_entity_identifiers(&mut self, dat_bytes: Vec<u8>) -> io::Result<()> {
        debug!("RuntimeManager: loading entity identifiers NBT from file...");
        let mut reader = ByteReader::from(dat_bytes);
        let nbt = BedrockNetworkNbt::read(&mut reader)?;
        self.entity_network_nbt = Some(Arc::new(nbt));
        Ok(())
    }

    /// 调色板字节（共享 Arc 克隆；StartGame 不再序列化该字段，
    /// 仅保留用于存在性检查与 ItemRegistry 包复用）。
    ///
    /// Palette bytes (shared Arc clone; StartGame no longer serializes this field,
    /// kept only for existence checks and ItemRegistry packet reuse).
    pub fn get_palette(&self) -> Option<Arc<Vec<u8>>> {
        self.item_palette.clone()
    }

    /// 原版实体标识 NBT（共享 Arc 克隆，避免整树深拷贝）。
    ///
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
