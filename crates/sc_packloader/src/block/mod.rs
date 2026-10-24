//! 方块定义域：静态组件、方块 JSON 解析与行为包加载。
//!
//! 解析结果携带规范状态枚举、排列条件与战利品表引用。
//!
//! Block definition domain: static components, block JSON parsing, and behavior-pack loading.
//!
//! Parsed results carry canonical state enumeration, permutation conditions, and loot-table references.

pub mod bundle;
pub mod component;
pub mod parse;

pub use component::{
    collision_boxes_of, is_ignored_component, CanContainLiquid, Chest, CollisionBox, CollisionBoxes,
    CraftingTable, DestructibleByMining, EfficiencyRule, Furnace, LightDampening, LightEmission,
    Liquid, Mining, MiningRule, MiningToolEntry, NeedsSupport, RandomTick, Replaceable,
    Unbreakable, WorkstationSlot, WorkstationSlotRole, ALLOWED_MINING_ENCHANTS,
    DEFAULT_HARVEST_PENALTY, EFFICIENCY_MAX_LEVEL_MAX, EFFICIENCY_MAX_LEVEL_MIN,
    HARVESTED_BRANCH_FACTOR, IGNORED_COMPONENTS,
};
use crate::components_export;
use crate::loot_tables::Loot;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Constants
// Constants
// ---------------------------------------------------------------------------

/// 支持的 `.json` 方块 schema 版本（清单声明用）。
///
/// Supported `.json` block schema version (manifest declaration).
pub const BLOCK_JSON_SCHEMA_VERSION: u32 = 1;

/// 行为包内的方块定义目录（固定，不可配置）。
///
/// Block definition directory inside behavior packs (fixed, not configurable).
pub const BLOCKS_DIRECTORY: &str = "blocks";

/// palette 拆分器写出的原版 `format_version`。
///
/// Vanilla `format_version` written by the palette splitter.
pub const BLOCK_JSON_FORMAT_VERSION: &str = "1.10.0";

/// 允许的 `block_data.network_id_mode` 取值。
///
/// Allowed `block_data.network_id_mode` values.
pub const NETWORK_ID_MODE_HASHED: &str = "hashed";
/// 调色板模式：网络 id 直接取自方块调色板顺序。
///
/// Palette mode: network ids come straight from the block palette order.
pub const NETWORK_ID_MODE_PALETTE: &str = "palette";

// ---------------------------------------------------------------------------
// BlockComponents: the items-style component enum over the raw component map
// ---------------------------------------------------------------------------

components_export![
    BlockComponents,
    CollisionBoxes = "minecraft:collision_box",
    LightEmission = "minecraft:light_emission",
    LightDampening = "minecraft:light_dampening",
    DestructibleByMining = "minecraft:destructible_by_mining",
    Replaceable = "sc:replaceable",
    Liquid = "sc:liquid",
    RandomTick = "sc:random_tick",
    NeedsSupport = "sc:needs_support",
    CanContainLiquid = "sc:can_contain_liquid",
    Unbreakable = "sc:unbreakable",
    Mining = "sc:mining",
    Chest = "sc:chest",
    Furnace = "sc:furnace",
    CraftingTable = "sc:crafting_table",
    // Loot domain (types live in `crate::loot_tables`; see that module).
    Loot = "minecraft:loot",
];

impl BlockComponents {
    /// 从原始组件映射做类型化读取（解析输出层就是原始映射）。
    ///
    /// Typed read from a raw component map (parse-output layers are raw maps).
    ///
    /// 镜像枚举的 `get::<T>()`，但面向 [`parse::ParsedBlockFile`] 携带的
    /// 普通映射：别名来自组件注册表（`T` 必须是已列出组件），
    /// 缺失/类型不匹配读作 `None`。
    ///
    /// Mirrors the enum's `get::<T>()`, but for the plain maps carried by
    /// [`parse::ParsedBlockFile`]: the alias comes from the component registry
    /// (`T` must be a listed component), missing/mismatched values read as `None`.
    ///
    /// - `map`: 原始组件映射
    pub fn get_from<T>(map: &std::collections::BTreeMap<String, serde_json::Value>) -> Option<T>
    where
        T: sc_ecs::component::Component + serde::de::DeserializeOwned,
    {
        let alias = Self::get_alias(&T::name())?;
        map.get(&alias)
            .and_then(|v| serde_json::from_value::<T>(v.clone()).ok())
    }
}

// ---------------------------------------------------------------------------
// Errors: pack identity + file path + field path, never silently fall back
// Error: pack identity + file path + field path, never silently falls back
// ---------------------------------------------------------------------------

/// 带完整位置的解析错误：包身份 + 文件路径 + 字段路径，绝不静默回退。
///
/// Parse error with full location: pack identity + file path + field path,
/// never silently falls back.
///
/// 显示为：
/// `Vanilla: blocks/minecraft/oak_log.json: $.sc:protocol_runtime_ids: <message>`。
///
/// Displays as:
/// `Vanilla: blocks/minecraft/oak_log.json: $.sc:protocol_runtime_ids: <message>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockJsonError {
    /// 包标识（版本包名或行为包路径）。
    ///
    /// Pack identity (version-pack name or behavior-pack path).
    pub pack: String,
    /// 包内文件路径。
    ///
    /// File path inside the pack.
    pub file: String,
    /// JSON 字段路径（如 `$.block_data.schema_version`）。
    ///
    /// JSON field path (e.g. `$.block_data.schema_version`).
    pub field: String,
    /// 错误描述。
    ///
    /// Error message.
    pub message: String,
}

impl BlockJsonError {
    /// 由四段位置信息构造错误。
    ///
    /// Builds an error from the four location parts.
    ///
    /// - `pack`: 包标识
    /// - `file`: 文件路径
    /// - `field`: 字段路径
    /// - `message`: 错误描述
    pub fn new(
        pack: impl Into<String>,
        file: impl Into<String>,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            pack: pack.into(),
            file: file.into(),
            field: field.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for BlockJsonError {
    /// 以 `包: 文件: 字段: 描述` 格式化。
    ///
    /// Formats as `pack: file: field: message`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {}: {}: {}",
            self.pack, self.file, self.field, self.message
        )
    }
}

impl std::error::Error for BlockJsonError {}

// ---------------------------------------------------------------------------
// Compile budgets (consumed by `sc_block` semantic compilation; the packloader
// load path itself is a direct load and enforces no budgets)
// ---------------------------------------------------------------------------

/// `sc_block` 解析后编译预算（工具/掉落/条目上限与快照大小）。
/// 包加载器加载路径为直接加载，不消费本结构；它是域间的共享契约。
///
/// Post-parse compilation budgets for `sc_block` (tool/drop/entry caps and
/// snapshot size). The packloader load path loads directly and does not
/// consume this struct; it stays the shared contract between the domains.
#[derive(Clone, Debug)]
pub struct BlockBundleBudgets {
    /// 最大文件数。
    ///
    /// Maximum number of files.
    pub max_files: usize,
    /// 单文件最大字节数。
    ///
    /// Maximum bytes per file.
    pub max_file_bytes: usize,
    /// 总字节数上限。
    ///
    /// Maximum total bytes.
    pub max_total_bytes: u64,
    /// 状态总数上限。
    ///
    /// Maximum total states.
    pub max_total_states: usize,
    /// 单方块状态数上限。
    ///
    /// Maximum states per block.
    pub max_states_per_block: usize,
    /// 单方块排列（permutation）数上限。
    ///
    /// Maximum permutations per block.
    pub max_permutations_per_block: usize,
    /// 单方块属性取值数上限。
    ///
    /// Maximum property values per block.
    pub max_prop_values_per_block: usize,
    /// 条件表达式长度上限。
    ///
    /// Maximum condition length.
    pub max_condition_len: usize,
    /// 属性字符串长度上限。
    ///
    /// Maximum property string length.
    pub max_prop_string_len: usize,
    /// 标识符长度上限。
    ///
    /// Maximum identifier length.
    pub max_identifier_len: usize,
    /// 组件字节数上限。
    ///
    /// Maximum component bytes.
    pub max_component_bytes: usize,
    /// 每状态 `sc:mining.tools` 条目上限（不含 `default`）。
    ///
    /// Max `sc:mining.tools` entries per state (excluding `default`).
    pub max_mining_tools_per_state: usize,
    /// 每个 `tools[].items` 的元素数上限。
    ///
    /// Max elements per `tools[].items`.
    pub max_mining_items_per_rule: usize,
    /// 每状态 `enchantments` 键数上限。
    ///
    /// Max `enchantments` keys per state.
    pub max_mining_enchants_per_state: usize,
    /// 每状态编译后掉落条目上限（来自被引用的战利品表）。
    ///
    /// Max compiled drop entries per state (from the referenced loot table).
    pub max_drop_entries_per_state: usize,
    /// 每条目 `count` 分布选项数上限。
    ///
    /// Max `count` distribution options per entry.
    pub max_count_options_per_entry: usize,
    /// 每条目 `bonus_by_level` 行数上限。
    ///
    /// Max `bonus_by_level` rows per entry.
    pub max_fortune_rules_per_entry: usize,
    /// 每条目 `requires.items` 元素数上限。
    ///
    /// Max `requires.items` elements per entry.
    pub max_requires_items_per_entry: usize,
    /// 每状态 `sc:workstation.slots` 条目上限（旧别名）。
    ///
    /// Max `sc:workstation.slots` entries per state (legacy alias).
    pub max_workstation_slots_per_state: usize,
    /// 每状态 `sc:furnace.slots` 条目上限。
    ///
    /// Max `sc:furnace.slots` entries per state.
    pub max_furnace_slots_per_state: usize,
    /// 每状态 `sc:chest.slots` 值上限。
    ///
    /// Max `sc:chest.slots` value per state.
    pub max_chest_slots_per_state: usize,
}

impl Default for BlockBundleBudgets {
    /// 默认预算（对齐历史加载限制）。
    ///
    /// Default budgets (aligned with the historical load limits).
    fn default() -> Self {
        Self {
            max_files: 8192,
            max_file_bytes: 1024 * 1024,
            max_total_bytes: 128 * 1024 * 1024,
            max_total_states: 65536,
            max_states_per_block: 2048,
            max_permutations_per_block: 256,
            max_prop_values_per_block: 256,
            max_condition_len: 512,
            max_prop_string_len: 256,
            max_identifier_len: 256,
            max_component_bytes: 16 * 1024,
            max_mining_tools_per_state: 64,
            max_mining_items_per_rule: 64,
            max_mining_enchants_per_state: 8,
            max_drop_entries_per_state: 32,
            max_count_options_per_entry: 16,
            max_fortune_rules_per_entry: 8,
            max_requires_items_per_entry: 64,
            max_workstation_slots_per_state: 8,
            max_furnace_slots_per_state: 8,
            max_chest_slots_per_state: 108,
        }
    }
}

// ---------------------------------------------------------------------------
// Manifest declarations
// Manifest declaration
// ---------------------------------------------------------------------------

/// `network_id_mode` 缺省值。
///
/// Default value of `network_id_mode`.
fn default_network_mode() -> String {
    NETWORK_ID_MODE_HASHED.to_string()
}

/// 版本包清单的 `block_data` 段（缺省表示走旧版 palette 路径）。
///
/// Version-pack manifest `block_data` section (absent means use the legacy palette path).
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct BlockDataManifest {
    /// 方块 schema 版本（仅 1；未知版本拒绝）。
    ///
    /// Block schema version (only 1; unknown versions are rejected).
    pub schema_version: u32,
    /// 行为包内的方块定义目录（信息性；加载扫描 `blocks/`）。
    ///
    /// Block definition directory inside behavior packs (informational; loading scans `blocks/`).
    #[serde(default)]
    pub directory: String,
    /// 网络 id 模式（`hashed` | `palette`，仅协议构建支持的取值）。
    ///
    /// Network id mode (`hashed` | `palette`, only values the protocol build supports).
    #[serde(default = "default_network_mode")]
    pub network_id_mode: String,
}

impl BlockDataManifest {
    /// 校验声明本身（模式枚举、schema 版本）。
    ///
    /// Validate the declaration itself (mode enum, schema version).
    ///
    /// - `pack_id`: 包标识（用于错误定位）
    ///
    /// # Errors
    /// schema 版本或网络 id 模式不受支持时返回定位错误。
    pub fn validate(&self, pack_id: &str) -> Result<(), BlockJsonError> {
        if self.schema_version != BLOCK_JSON_SCHEMA_VERSION {
            return Err(BlockJsonError::new(
                pack_id,
                "<manifest>",
                "$.block_data.schema_version",
                format!(
                    "不支持的 block schema_version {}（仅支持 1）",
                    self.schema_version
                ),
            ));
        }
        if self.network_id_mode != NETWORK_ID_MODE_HASHED
            && self.network_id_mode != NETWORK_ID_MODE_PALETTE
        {
            return Err(BlockJsonError::new(
                pack_id,
                "<manifest>",
                "$.block_data.network_id_mode",
                format!(
                    "不支持的 network_id_mode {:?}（仅 hashed|palette）",
                    self.network_id_mode
                ),
            ));
        }
        Ok(())
    }
}

// Re-export common types at the domain root (callers use `crate::block::X`).
// Re-export common types at the domain root (callers use `crate::block::X`).
pub use bundle::{
    fingerprint_bundle, load_block_json_bundle, parse_hardness_json, parse_legacy_palette_entries,
    split_legacy_palette, split_state_key, BlockHardness, BlockJsonBundle, GeneratedBlockFile,
    LegacyPaletteEntry, SplitError,
};
pub use parse::{
    canonical_state_count, canonical_state_index, canonical_state_key, canonical_state_props,
    parse_block_file, parse_permutation_condition, validate_identifier, ConditionTest, NbtPropType,
    ParsedBlockFile, ParsedPermutation, PermutationCondition, PropValue, PropertyDef,
};
