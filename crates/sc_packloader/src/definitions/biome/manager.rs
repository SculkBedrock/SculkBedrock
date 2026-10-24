//! 生物群系管理器：索引生物群系与网络 id，并生成列表 NBT。
//!
//! Biome manager: indexes biomes and network ids, and builds the list NBT.

use crate::definitions::biome::MinecraftBiomeSpawner;
use log::debug;
use sc_ecs::resource::Resource;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use std::collections::HashMap;

/// 生物群系管理器（ECS 资源）：标识名 → 定义、标识名 → 网络 id、列表 NBT。
///
/// Biome manager (ECS resource): identifier → definition, identifier →
/// network id, and the list NBT.
#[derive(Clone, Debug, Resource)]
pub struct MinecraftBiomeManager {
    /// 标识名到生物群系定义的映射。
    ///
    /// Map from identifier to biome definition.
    map: HashMap<String, MinecraftBiomeSpawner>,
    /// 标识名到网络 id 的映射。
    ///
    /// Map from identifier to network id.
    ids: HashMap<String, i16>,
    /// 构建好的生物群系列表 NBT。
    ///
    /// Built biome list NBT.
    nbt: Option<NbtValue>,
}

impl MinecraftBiomeManager {
    /// 构造空管理器。
    ///
    /// Creates an empty manager.
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            ids: HashMap::new(),
            nbt: None,
        }
    }

    /// 从生物群系列表构造管理器。
    ///
    /// Creates a manager from a list of biomes.
    ///
    /// - `biomes`: 生物群系列表
    pub fn from_vec(biomes: Vec<MinecraftBiomeSpawner>) -> Self {
        let mut manager = Self::new();
        for b in biomes {
            manager.push_biome(b);
        }
        manager
    }

    /// 加入一个生物群系（以标识名为键，重复标识名会被覆盖）。
    ///
    /// Adds a biome (keyed by identifier; duplicates overwrite).
    ///
    /// - `biome`: 待加入的生物群系定义
    pub fn push_biome(&mut self, biome: MinecraftBiomeSpawner) {
        self.map.insert(biome.description.identifier.clone(), biome);
    }

    /// 按标识名查询生物群系。
    ///
    /// Looks up a biome by identifier.
    ///
    /// - `identifier`: 生物群系标识名
    pub fn get_biome(&self, identifier: &str) -> Option<&MinecraftBiomeSpawner> {
        self.map.get(identifier)
    }

    /// 设置标识名 → 网络 id 映射。
    ///
    /// Sets the identifier → network id map.
    ///
    /// - `ids`: 标识名到网络 id 的映射
    pub fn set_biome_ids(&mut self, ids: HashMap<String, i16>) {
        self.ids = ids;
    }

    /// 查询网络 id；未命中时尝试去掉 `minecraft:` 前缀再查。
    ///
    /// Looks up the network id, retrying without the `minecraft:` prefix.
    ///
    /// - `identifier`: 生物群系标识名
    pub fn biome_id(&self, identifier: &str) -> Option<i16> {
        self.ids.get(identifier).copied().or_else(|| {
            identifier
                .strip_prefix("minecraft:")
                .and_then(|short| self.ids.get(short).copied())
        })
    }

    /// 遍历全部已加载生物群系（按注册表顺序，用于构建 BiomeDefinitionList）。
    ///
    /// Iterate all loaded biomes (used to build BiomeDefinitionList, in biome registry order).
    pub fn iter(&self) -> impl Iterator<Item = &MinecraftBiomeSpawner> {
        self.map.values()
    }

    /// 生物群系数量。
    ///
    /// Number of biomes.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// 是否为空。
    ///
    /// Whether the manager is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 构建生物群系列表 NBT（各生物群系组件的 NBT 汇编）。
    ///
    /// Builds the biome list NBT (assembly of each biome's component NBT).
    pub fn build_nbt(&mut self) {
        debug!("BiomeManager: building biome list NBT...");
        let mut compound = CompoundNbt::new(Some("".to_string()));
        for (identifier, biome) in self.map.iter() {
            if let Some(components) = biome.components.as_ref() {
                if let Ok(nbt) = components.to_nbt() {
                    compound.insert(identifier, nbt);
                }
            }
        }
        self.nbt = Some(NbtValue::Compound(compound));
    }

    /// 获取构建好的列表 NBT。
    ///
    /// Returns the built list NBT.
    pub fn get_nbt(&self) -> Option<&NbtValue> {
        self.nbt.as_ref()
    }

    /// 导出 生物群系 id → 官方标签集合（`minecraft:tags` 组件）的映射。
    ///
    /// Export the biome id to official tag-set (`minecraft:tags` component) mapping.
    ///
    /// 消费者：世界生成的树特性 `canSpawnHere` 检查（与
    /// `Registries.BIOME.containsTag(BiomeTags.X, biomeId)` 同样的查询）。
    /// 标签字符串与官方生物群系 JSON 的 `minecraft:tags.tags` 一致（如 "forest"/"jungle"）。
    ///
    /// Consumer: the tree feature `canSpawnHere` check in world generation
    /// (same query as `Registries.BIOME.containsTag(BiomeTags.X, biomeId)`).
    /// Tag strings match the official biome json `minecraft:tags.tags` (e.g. "forest"/"jungle").
    pub fn biome_tags_by_id(&self) -> HashMap<i32, std::collections::HashSet<String>> {
        use crate::definitions::biome::component::Tags;
        let mut result = HashMap::new();
        for (identifier, biome) in self.map.iter() {
            let Some(id) = self.biome_id(identifier) else {
                continue;
            };
            let Some(components) = biome.components.as_ref() else {
                continue;
            };
            if let Some(tags) = components.get::<Tags>() {
                result.insert(id as i32, tags.tags.iter().cloned().collect());
            }
        }
        result
    }
}
