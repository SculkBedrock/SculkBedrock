//! Block definition domain (mirrors the `super::item` layout).
//!
//! - [`component`]: static-component ECS Component structs + registry
//!   (follows `item::component`: standalone structs deriving `Component`, fixed list);
//! - [`schema`]: `.block.json` (vanilla `format_version` + `minecraft:block`
//!   structure) discovery, bounded reads, and schema parsing (packloader side of the bundle pipeline;
//!   semantic compilation and dense tables live in `sc_block`, see its snapshot).
//!
//! Correspondence with `item/`:
//!
//! | Item | Block |
//! |---|---|
//! | `item::component` structs + `components_export!` | `block::component` structs + registry |
//! | `MinecraftItemSpawner` (raw JSON + components) | schema parse output (`ParsedBlockFile`, see [`schema`]) |
//! | `ItemComponentTable` (dense table by runtime id, inside packloader) | `sc_block` snapshot dense columns (by `BlockStateId`; block tables live in the game domain because they need compiled ordering plus overlay expansion) |
//! | `insert(world, entity)` (attaches item components to entities, e.g. drop display) | No counterpart: block files declare no `behaviors`/`block_entity` placeholders |
//!
//! Tags live outside this domain: vanilla block JSON has no tag slot; tags are version-pack-level data
//! (`definitions/block_tags.json`, `SCVersionPack::take_block_tags`).

pub mod component;
pub mod schema;

// Re-export common types at the domain root (callers use `crate::block::X`).
pub use component::{
    collision_boxes_of, get_block_component, is_ignored_component, is_known_component,
    validate_block_component, validate_collision_boxes, BlockComponentSchema, CanContainLiquid,
    CollisionBox, CollisionBoxes, CountOption, DestructibleByMining, DropEntry, Drops,
    EfficiencyRule, FortuneBonus, FortuneRule, LightDampening, LightEmission, Liquid, Loot, Mining,
    MiningRule, MiningToolEntry, NeedsSupport, RandomTick, Replaceable, RequiresRule, Unbreakable,
    ALLOWED_DROP_ENCHANTS, ALLOWED_MINING_ENCHANTS, ALL_BLOCK_COMPONENTS, COUNT_SUM_TOLERANCE,
    DEFAULT_HARVEST_PENALTY, FORTUNE_LEVEL_MAX, FORTUNE_LEVEL_MIN, HARVESTED_BRANCH_FACTOR,
    IGNORED_COMPONENTS, MAX_DROP_COUNT_TOTAL, ONE_OF_TOTAL_TOLERANCE,
};
pub use schema::{
    canonical_state_count, canonical_state_index, canonical_state_key, canonical_state_props,
    fingerprint_bundle, fingerprint_bundle_with_budgets, load_block_json_bundle, parse_block_file,
    parse_hardness_json, parse_legacy_palette_entries, parse_permutation_condition,
    reject_duplicate_keys, split_legacy_palette, split_state_key, validate_block_path,
    validate_identifier, BlockBundleBudgets, BlockDataManifest, BlockHardness, BlockJsonBundle,
    BlockJsonError, ConditionTest, GeneratedBlockFile, LegacyPaletteEntry, NbtPropType,
    ParsedBlockFile, ParsedPermutation, PermutationCondition, PropValue, PropertyDef, SplitError,
    BLOCKS_DIRECTORY, BLOCK_BUDGETS_VERSION, BLOCK_JSON_FORMAT_VERSION, BLOCK_JSON_SCHEMA_VERSION,
    NETWORK_ID_MODE_HASHED, NETWORK_ID_MODE_PALETTE,
};
