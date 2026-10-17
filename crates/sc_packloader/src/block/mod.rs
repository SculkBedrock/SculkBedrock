//! Block definition domain (mirrors the `super::item` layout).
//!
//! File layout (flat, one file per responsibility):
//!
//! - [`component`]: static-component ECS Component structs (follows
//!   `item::component`: standalone structs deriving `Component` + `Deserialize`);
//! - [`parse`]: `.json` block-file parsing (vanilla `format_version` +
//!   `minecraft:block` structure, canonical state enumeration, permutation
//!   conditions). Loading is direct: JSON in, [`parse::ParsedBlockFile`]
//!   out — no format whitelist, no key hygiene, no budgets.
//! - [`bundle`]: bundle discovery/load from behavior packs
//!   (`behavior_packs/<pack>/blocks/**/*.json`), fingerprinting, and
//!   legacy-palette splitting. When a file's `minecraft:loot` references a
//!   loot-table file, the loader calls `crate::loot_tables::parse_loot_tables`
//!   and inserts the parsed [`crate::loot_tables::LootTables`] component into
//!   the component map (see [`crate::loot_tables`]).
//!
//! Components follow the items design: [`BlockComponents`] is the
//! `components_export!` enum over the raw component map, with typed
//! `get::<T>()` reads and an `is_known` registry; every listed component type
//! also carries an inherent `NAME` const (its registered key).
//!
//! | Item | Block |
//! |---|---|
//! | `item::component` structs | `block::component` structs |
//! | `ItemComponents` (`components_export!`) | `BlockComponents` (`components_export!`) |
//! | `MinecraftItemSpawner` (raw JSON + components) | `parse_block_file` output (`ParsedBlockFile`) |
//! | `ItemComponentTable` (dense table by runtime id) | `sc_block` snapshot dense columns (by `BlockStateId`; block tables live in the game domain because they need compiled ordering plus overlay expansion) |

pub mod bundle;
pub mod component;
pub mod parse;

use crate::components_export;
use crate::loot_tables::{Drops, Loot, LootTables};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Supported `.json` block schema version (manifest declaration).
pub const BLOCK_JSON_SCHEMA_VERSION: u32 = 1;

/// Block definition directory inside behavior packs (fixed, not configurable).
pub const BLOCKS_DIRECTORY: &str = "blocks";

/// Vanilla `format_version` written by the palette splitter.
pub const BLOCK_JSON_FORMAT_VERSION: &str = "1.10.0";

/// Allowed `block_data.network_id_mode` values.
pub const NETWORK_ID_MODE_HASHED: &str = "hashed";
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
    Workstation = "sc:workstation",
    Process = "sc:process",
    // Loot domain (types live in `crate::loot_tables`; see that module).
    Loot = "minecraft:loot",
    Drops = "sc:drops",
    LootTables = "sc:loot_tables",
];

impl BlockComponents {
    /// Typed read from a raw component map (parse-output layers are raw maps).
    ///
    /// Mirrors the enum's `get::<T>()`, but for the plain maps carried by
    /// [`parse::ParsedBlockFile`]: the alias comes from the component registry
    /// (`T` must be a listed component), missing/mismatched values read as `None`.
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
// Error: pack identity + file path + field path, never silently falls back
// ---------------------------------------------------------------------------

/// Parse error with full location.
///
/// Displays as:
/// `Vanilla: blocks/minecraft/oak_log.json: $.sc:protocol_runtime_ids: <message>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockJsonError {
    pub pack: String,
    pub file: String,
    pub field: String,
    pub message: String,
}

impl BlockJsonError {
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

/// Post-parse compilation budgets for `sc_block` (tool/drop/entry caps and
/// snapshot size). The packloader load path loads directly and does not
/// consume this struct; it stays the shared contract between the domains.
#[derive(Clone, Debug)]
pub struct BlockBundleBudgets {
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: u64,
    pub max_total_states: usize,
    pub max_states_per_block: usize,
    pub max_permutations_per_block: usize,
    pub max_prop_values_per_block: usize,
    pub max_condition_len: usize,
    pub max_prop_string_len: usize,
    pub max_identifier_len: usize,
    pub max_component_bytes: usize,
    /// Max `sc:mining.tools` entries per state (excluding `default`).
    pub max_mining_tools_per_state: usize,
    /// Max elements per `tools[].items`.
    pub max_mining_items_per_rule: usize,
    /// Max `enchantments` keys per state.
    pub max_mining_enchants_per_state: usize,
    /// Max `sc:drops.entries` entries per state.
    pub max_drop_entries_per_state: usize,
    /// Max `count` distribution options per entry.
    pub max_count_options_per_entry: usize,
    /// Max `bonus_by_level` rows per entry.
    pub max_fortune_rules_per_entry: usize,
    /// Max `requires.items` elements per entry.
    pub max_requires_items_per_entry: usize,
    /// Max `sc:workstation.slots` entries per state.
    pub max_workstation_slots_per_state: usize,
}

impl Default for BlockBundleBudgets {
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
        }
    }
}

// ---------------------------------------------------------------------------
// Manifest declaration
// ---------------------------------------------------------------------------

fn default_network_mode() -> String {
    NETWORK_ID_MODE_HASHED.to_string()
}

/// Version-pack manifest `block_data` section (absent means use the legacy palette path).
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct BlockDataManifest {
    /// Block schema version (only 1; unknown versions are rejected).
    pub schema_version: u32,
    /// Block definition directory inside behavior packs (informational; loading scans `blocks/`).
    #[serde(default)]
    pub directory: String,
    /// Network id mode (`hashed` | `palette`, only values the protocol build supports).
    #[serde(default = "default_network_mode")]
    pub network_id_mode: String,
}

impl BlockDataManifest {
    /// Validate the declaration itself (mode enum, schema version).
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
pub use bundle::{
    fingerprint_bundle, load_block_json_bundle, parse_hardness_json, parse_legacy_palette_entries,
    split_legacy_palette, split_state_key, BlockHardness, BlockJsonBundle, GeneratedBlockFile,
    LegacyPaletteEntry, SplitError,
};
pub use component::{
    collision_boxes_of, is_ignored_component, CanContainLiquid, CollisionBox, CollisionBoxes,
    DestructibleByMining, EfficiencyRule, LightDampening, LightEmission, Liquid, Mining,
    MiningRule, MiningToolEntry, NeedsSupport, Process, ProcessKind, RandomTick, Replaceable,
    Unbreakable, Workstation, WorkstationSlot, WorkstationSlotRole, ALLOWED_MINING_ENCHANTS,
    DEFAULT_HARVEST_PENALTY, EFFICIENCY_MAX_LEVEL_MAX, EFFICIENCY_MAX_LEVEL_MIN,
    HARVESTED_BRANCH_FACTOR, IGNORED_COMPONENTS,
};
pub use parse::{
    canonical_state_count, canonical_state_index, canonical_state_key, canonical_state_props,
    parse_block_file, parse_permutation_condition, validate_identifier, ConditionTest, NbtPropType,
    ParsedBlockFile, ParsedPermutation, PermutationCondition, PropValue, PropertyDef,
};
