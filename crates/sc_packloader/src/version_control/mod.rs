//! 版本包（version pack）模型：清单、世界生成数据与各段原始字节。
//!
//! Version pack model: manifest, worldgen data, and raw section bytes.

use crate::block::{BlockDataManifest, BlockJsonBundle};
use crate::definitions::attribute::EntityAttribute;
use crate::definitions::biome::MinecraftBiomeSpawner;
use crate::pack::ResourcePack;
use crate::pack_loader::pack::zipped::PackSource;
use crate::pack_loader::pack::zipped::ZippedResourcePack;
use crate::version_control::loader::VersionPackLoaderError;
use crate::version_control::runtime::MinecraftRuntimeJson;
use sc_ecs::resource::Resource;
use sc_utils::game::structs::minecraft_version::MinecraftVersion;
use serde::Deserialize;
use std::collections::HashMap;

pub mod biome_features;
pub mod identifier;
pub(crate) mod json_budget;
pub mod loader;
pub mod manager;
pub mod runtime;
pub mod scdb;

/// 版本包清单（.scver 内 manifest）。
///
/// Version pack manifest (inside a .scver).
#[derive(Deserialize, Debug, Clone)]
pub struct SCVersionPackManifest {
    /// 包名。
    ///
    /// Pack name.
    pub name: String,
    /// 包描述。
    ///
    /// Pack description.
    pub description: String,
    /// 作者列表。
    ///
    /// Author list.
    pub authors: Vec<String>,
    /// 游戏版本字符串。
    ///
    /// Game version string.
    pub minecraft_version: String,
    /// 网络协议版本字符串（如 "1.26.30"），用于 StartGame vanillaVersion、
    /// MOTD、ResourcePackStack game_version 等在线协议字段。
    /// 与 minecraft_version（如 "1.26.33"）不同：后者是版本包/客户端的实际游戏版本，
    /// 前者是本协议构建支持的在线版本。
    ///
    /// Network protocol version string (e.g. "1.26.30") for StartGame vanillaVersion,
    /// MOTD, ResourcePackStack game_version, and other online protocol fields.
    /// Differs from minecraft_version (e.g. "1.26.33"): the latter is the actual game version of
    /// the version pack/client, the former is the online version this protocol build supports.
    #[serde(default)]
    pub minecraft_network_version: Option<String>,
    /// 协议号。
    ///
    /// Protocol number.
    pub protocol_version: u32,
    /// 新版方块数据声明（`definitions/blocks/<ns>/<path>.block.json`）。
    /// 缺省时回退旧版 `block_palette.nbt` 路径；一旦声明，加载失败不得回退。
    ///
    /// New block-data declaration (`definitions/blocks/<ns>/<path>.block.json`).
    /// When absent, falls back to the legacy `block_palette.nbt` path; once declared, load failures must not fall back.
    #[serde(default)]
    pub block_data: Option<BlockDataManifest>,
}

impl SCVersionPackManifest {
    /// 返回网络版本字符串；清单未提供 minecraft_network_version 时回退
    /// minecraft_version，保持向后兼容。
    ///
    /// Return the network version string. Falls back to minecraft_version when the manifest
    /// omits minecraft_network_version, preserving backward compatibility.
    pub fn network_version(&self) -> &str {
        self.minecraft_network_version
            .as_deref()
            .unwrap_or(&self.minecraft_version)
    }
}

/// 版本包 `definitions/worldgen/` 数据（密度函数 / 噪声地形数据）。
/// 包加载器只做 IO 与反序列化（不做语义解析）；语义解析位于世界生成。
///
/// Version-pack `definitions/worldgen/` data (density_function / noise terrain data).
/// Packloader only does IO and deserialization (no semantic parsing); semantic parsing lives in world generation.
#[derive(Debug, Clone, Default)]
pub struct WorldgenDataBundle {
    /// 相对路径（如 "overworld/offset"）→ JSON 字节（位于 definitions/worldgen/density_function/）。
    ///
    /// Relative path (e.g. "overworld/offset") to JSON bytes (under definitions/worldgen/density_function/).
    pub density_functions: HashMap<String, Vec<u8>>,
    /// 相对路径（如 "continentalness"）→ JSON 字节（位于 definitions/worldgen/noise/）。
    ///
    /// Relative path (e.g. "continentalness") to JSON bytes (under definitions/worldgen/noise/).
    pub noises: HashMap<String, Vec<u8>>,
    /// 相对路径（如 "overworld"）→ JSON 字节（位于 definitions/worldgen/noise_settings/）。
    ///
    /// Relative path (e.g. "overworld") to JSON bytes (under definitions/worldgen/noise_settings/).
    pub noise_settings: HashMap<String, Vec<u8>>,
}

/// 版本包（ECS 资源）：清单 + 各段原始数据，字段多为加载后取走（take）式消费。
///
/// Version pack (ECS resource): manifest plus raw sections; most fields are
/// consumed via `take` after loading.
#[derive(Resource, Debug, Clone)]
pub struct SCVersionPack {
    /// 包清单。
    ///
    /// Pack manifest.
    pub manifest: SCVersionPackManifest,
    /// 内嵌行为包来源列表。
    ///
    /// Embedded behavior pack sources.
    behavior_packs: Vec<PackSource>,
    /// 生物群系列表。
    ///
    /// Biome list.
    biomes: Vec<MinecraftBiomeSpawner>,
    /// 属性名 → 默认属性。
    ///
    /// Attribute name → default attribute.
    attributes: HashMap<String, EntityAttribute>,
    /// 内嵌插件字节（zip 列表，每个条目一个插件包）。
    ///
    /// Embedded plugin bytes (zip list; one entry per plugin package).
    plugins: Vec<Vec<u8>>,
    /// 运行时 id 映射表 JSON 列表。
    ///
    /// Runtime id mapping JSON entries.
    pub runtime_id: Vec<MinecraftRuntimeJson>,
    /// 原版 entity_identifiers.dat 文件原始字节。
    /// 保存从资源文件加载的完整原版实体标识符集合。
    ///
    /// Raw bytes of the vanilla entity_identifiers.dat file
    /// Holds the full vanilla entity identifier set loaded from the resource file
    entity_identifiers: Vec<u8>,
    /// definitions/block_palette.nbt 原始字节（可选；缺省时回退引导字典）。
    /// 语义解析位于 sc_block；包加载器不接触方块语义。
    ///
    /// Raw bytes of definitions/block_palette.nbt (optional; falls back to a bootstrapped dictionary when absent).
    /// Semantic parsing lives in sc_block; packloader does not touch block semantics.
    block_palette: Option<Vec<u8>>,
    /// definitions/creative_items.json 原始字节（可选，Kaooot 格式创造模式物品表）。
    /// 由 MinecraftRuntimeManager 解析（分组 + 每物品 group_index）。
    ///
    /// Raw bytes of definitions/creative_items.json (optional Kaooot-format creative-item table).
    /// Parsed by MinecraftRuntimeManager (groups + per-item group_index).
    creative_items: Option<Vec<u8>>,
    /// 生物群系标识名 → 网络 id。
    ///
    /// Biome identifier → network id.
    biome_ids: HashMap<String, i16>,
    /// definitions/block_tags.json 原版方块标签表（标签 → 方块标识列表，可选）。
    /// 标签是版本包级数据（不在方块文件内）；标识到状态的解析发生在消费方；
    /// 包加载器只做 IO 与反序列化。
    ///
    /// definitions/block_tags.json vanilla block tag table (tag to block-identifier list, optional).
    /// Tags are version-pack-level data (not inside block files); identifier-to-state resolution
    /// happens in consumers; packloader only does IO and deserialization.
    block_tags: HashMap<String, Vec<String>>,
    /// definitions/recipe_groups.json 合成原料组表（组名 → 物品标识列表，可选）。
    ///
    /// definitions/recipe_groups.json crafting ingredient group table
    /// (group name to item-identifier list, optional).
    ///
    /// 合并的标签查询表 + Bedrock 旧版伪名族（如
    /// `{"item": "minecraft:planks"/"minecraft:wood"/...}` 展开，按旧版数据顺序；
    /// `data` 索引语义依赖它），离线生成并签入。
    /// 包加载器只做 IO 与反序列化；成员到注册表的过滤发生在消费方（配方注册表加载器），
    /// 未知成员被丢弃。
    ///
    /// Merged tag-query table plus Bedrock legacy pseudo-name families
    /// (e.g. `{"item": "minecraft:planks"/"minecraft:wood"/...}` expansion, in legacy data order;
    /// the `data` index semantics depend on it), generated offline and checked in.
    /// Packloader only does IO and deserialization; member-to-registry filtering happens in consumers
    /// (the recipe-registry loader), unknown members are dropped.
    recipe_groups: HashMap<String, Vec<String>>,
    /// definitions/biome_features.json 世界生成特性调度数据（可选，
    /// 抽取自 biome_definitions.nbt）。调度语义位于世界生成。
    ///
    /// definitions/biome_features.json worldgen feature-scheduling data (optional,
    /// extracted from biome_definitions.nbt). Scheduling semantics live in world generation.
    biome_features: biome_features::BiomeFeaturesData,
    /// definitions/worldgen/ 地形数据（density_function + noise JSON）。
    ///
    /// definitions/worldgen/ terrain data (density_function + noise JSON).
    worldgen: WorldgenDataBundle,
    /// 新版方块数据包（manifest 声明 `block_data` 时解析；`sc_block` 编译的输入）。
    /// 未声明时为 None（走旧版 palette 路径）。
    ///
    /// New block bundle (parsed when the manifest declares `block_data`; input to `sc_block` compilation).
    /// None when undeclared (legacy palette path is used).
    block_json_bundle: Option<BlockJsonBundle>,
    /// 清单是否声明 `block_data`（声明的包不得混用旧版 palette/tag 字节）。
    ///
    /// Whether the manifest declares `block_data` (declared bundles must not mix with legacy palette/tag bytes).
    block_json_declared: bool,
}

impl SCVersionPack {
    /// 取走生物群系列表。
    ///
    /// Takes the biome list.
    pub fn take_biomes(&mut self) -> Vec<MinecraftBiomeSpawner> {
        std::mem::take(&mut self.biomes)
    }

    /// 取走 entity_identifiers 字节，避免内存中保留重复副本。
    ///
    /// Consume the entity_identifiers bytes to avoid keeping a duplicate copy in memory
    pub fn take_entity_identifiers(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.entity_identifiers)
    }
    /// 借用 block_palette 原始字节（缺省时为 None）。
    ///
    /// Borrow the raw block_palette bytes (None when absent).
    ///
    /// 由 dlopen 的 rust-kind 插件读取：每个插件 DLL 静态链接自己的
    /// sc_world 副本及各自的 `BlockStateDictionary::global()`，因此需要
    /// 这些字节注册进自己的字典副本。
    ///
    /// Read by dlopen rust-kind plugins: each plugin DLL statically links its own
    /// sc_world copy with its own `BlockStateDictionary::global()`, so it needs
    /// the bytes to register into its own dictionary copy.
    pub fn block_palette_bytes(&self) -> Option<&[u8]> {
        self.block_palette.as_deref()
    }
    /// 取走 block_palette 原始字节（缺省时为 None）。
    ///
    /// Take the raw block_palette bytes (None when absent).
    pub fn take_block_palette(&mut self) -> Option<Vec<u8>> {
        self.block_palette.take()
    }
    /// 取走 creative_items.json 原始字节（缺省时为 None）。
    ///
    /// Take the raw creative_items.json bytes (None when absent).
    pub fn take_creative_items(&mut self) -> Option<Vec<u8>> {
        self.creative_items.take()
    }
    /// 取走生物群系 id 映射。
    ///
    /// Takes the biome id map.
    pub fn take_biome_ids(&mut self) -> HashMap<String, i16> {
        std::mem::take(&mut self.biome_ids)
    }
    /// 取走方块标签表（标签 → 方块标识列表；definitions/block_tags.json，可选）。
    ///
    /// Take the block tag table (tag to block-identifier list; definitions/block_tags.json, optional).
    pub fn take_block_tags(&mut self) -> HashMap<String, Vec<String>> {
        std::mem::take(&mut self.block_tags)
    }
    /// block_tags 是否存在（非空）。
    ///
    /// Whether block_tags is present (non-empty).
    pub fn has_block_tags(&self) -> bool {
        !self.block_tags.is_empty()
    }
    /// 取走合成原料组表（组名 → 物品标识列表；definitions/recipe_groups.json，可选）。
    ///
    /// Take the crafting ingredient group table (group name to item-identifier list;
    /// definitions/recipe_groups.json, optional).
    pub fn take_recipe_groups(&mut self) -> HashMap<String, Vec<String>> {
        std::mem::take(&mut self.recipe_groups)
    }
    /// recipe_groups 是否存在（非空）。
    ///
    /// Whether recipe_groups is present (non-empty).
    pub fn has_recipe_groups(&self) -> bool {
        !self.recipe_groups.is_empty()
    }
    /// 取走特性调度数据（definitions/biome_features.json，可选）。
    ///
    /// Take the feature-scheduling data (definitions/biome_features.json, optional).
    pub fn take_biome_features(&mut self) -> biome_features::BiomeFeaturesData {
        std::mem::take(&mut self.biome_features)
    }
    /// biome_features 是否存在（非空）。
    ///
    /// Whether biome_features is present (non-empty).
    pub fn has_biome_features(&self) -> bool {
        !self.biome_features.biomes.is_empty()
    }
    /// 取走方块数据包（供 `sc_block` 编译；取走后保留声明标记）。
    ///
    /// Take the block bundle (for `sc_block` compilation; the declared flag is kept after taking).
    pub fn take_block_json_bundle(&mut self) -> Option<BlockJsonBundle> {
        self.block_json_bundle.take()
    }

    /// 是否声明了新版方块数据（声明的包不得回退或混用旧版 palette）。
    ///
    /// Whether new block data is declared (declared packs must not fall back to or mix the legacy palette).
    pub fn has_declared_block_json(&self) -> bool {
        self.block_json_declared
    }

    /// 写入派生 palette 字节（由快照派生，供动态插件字典副本消费）。
    ///
    /// Write derived palette bytes (snapshot-derived, consumed by dynamic plugin dictionary copies).
    pub fn set_derived_block_palette(&mut self, bytes: Vec<u8>) {
        self.block_palette = Some(bytes);
    }

    /// 取走世界生成数据（density_function + noise JSON 包）。
    ///
    /// Take the worldgen data (density_function + noise JSON bundle).
    pub fn take_worldgen(&mut self) -> WorldgenDataBundle {
        std::mem::take(&mut self.worldgen)
    }
    /// 借用世界生成数据（非消费）。
    ///
    /// Borrow the worldgen data (non-consuming).
    pub fn worldgen(&self) -> &WorldgenDataBundle {
        &self.worldgen
    }
    /// 世界生成数据是否存在。
    ///
    /// Whether worldgen data is present.
    pub fn has_worldgen(&self) -> bool {
        !self.worldgen.density_functions.is_empty() || !self.worldgen.noises.is_empty()
    }
    /// block_palette 是否存在。
    ///
    /// Whether block_palette is present.
    pub fn has_block_palette(&self) -> bool {
        self.block_palette.is_some()
    }
    /// 包内插件字节（zip 列表；每个条目一个插件包）。
    /// 实际加载由版本包插件加载器完成：
    /// `kind:"rust"` 经内置工厂解析，`kind:"cabi"` 走 HostPluginV1。
    ///
    /// In-pack plugin bytes (zip list; one entry per plugin package).
    /// Actual loading is done by the version-pack plugin loader:
    /// `kind:"rust"` resolves via the builtin factory, `kind:"cabi"` goes through HostPluginV1.
    pub fn plugins_bytes(&self) -> &[Vec<u8>] {
        &self.plugins
    }

    /// 引导方取得所有权后取走内嵌插件归档。
    ///
    /// Consume embedded plugin archives after bootstrap takes ownership.
    pub fn take_plugins_bytes(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.plugins)
    }

    /// 插件数量。
    ///
    /// Number of plugins.
    pub fn plugins_len(&self) -> usize {
        self.plugins.len()
    }
    /// 取走并解析内嵌行为包（版本, 包）列表；解析失败的包被跳过。
    ///
    /// Takes and parses embedded behavior packs as (version, pack); packs
    /// that fail to parse are skipped.
    pub fn take_behavior_packs(&mut self) -> Vec<(MinecraftVersion, ResourcePack)> {
        let mut behavior_packs: Vec<(MinecraftVersion, ResourcePack)> = Vec::new();
        for mut source in std::mem::take(&mut self.behavior_packs) {
            if let Some(pack) = ZippedResourcePack::get_resource_pack_shared(&mut source, false) {
                behavior_packs.push((pack.manifest.version, pack));
            }
        }
        behavior_packs
    }

    /// 返回生物群系列表副本。
    ///
    /// Returns a copy of the biome list.
    pub fn get_biomes(&self) -> Result<Vec<MinecraftBiomeSpawner>, VersionPackLoaderError> {
        Ok(self.biomes.clone())
    }

    /// 取走默认属性表。
    ///
    /// Takes the default attribute table.
    pub fn take_attributes(&mut self) -> HashMap<String, EntityAttribute> {
        std::mem::take(&mut self.attributes)
    }

    /// 取走运行时 id 映射表。
    ///
    /// Takes the runtime id mapping entries.
    pub fn take_runtime_id(&mut self) -> Vec<MinecraftRuntimeJson> {
        std::mem::take(&mut self.runtime_id)
    }
}
