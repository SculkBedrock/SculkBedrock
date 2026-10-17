//! Loot-table data domain (`loot_tables/**/*.json`).
//!
//! Responsibilities:
//!
//! - [`LootTables`]: the parsed loot-table component. When a block or entity
//!   JSON declares `minecraft:loot` with a loot-table path reference, the
//!   loaders call [`parse_loot_tables`] on the referenced file and insert the
//!   resulting component into the component map (block / entity), keyed by
//!   [`LootTables::NAME`]. The original `minecraft:loot` path entry stays
//!   untouched as the vanilla declaration / provenance.
//! - [`parse_loot_tables`]: direct load — JSON in, component out. No schema
//!   validation, no field whitelist, no budgets (the pack JSON reader budget
//!   still bounds depth/size, same as every other pack JSON read).
//!
//! Decoupling: this module never imports the block domain. Blocks and entities
//! reference tables by path; this module owns the table content. Resolution of
//! a path to bytes is injected by the caller (the loader that owns the pack
//! source), so the same helpers serve version packs and behavior packs.

use crate::macros::ComponentKey;
use sc_ecs::component::Component;
use serde::Deserialize;
use serde_json::Value;

use crate::version_control::json_budget::BudgetReader;
use json_comments::StripComments;

// ---------------------------------------------------------------------------
// Loot-related component types (moved out of the block component registry)
// ---------------------------------------------------------------------------

/// Loot-table path reference (`minecraft:loot`); the referenced file is parsed
/// into a [`LootTables`] component at load time (see [`parse_loot_tables`]).
///
/// The registered component key (`NAME`) comes from the
/// `BlockComponents` `components_export!` list.
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(transparent)]
pub struct Loot(pub Box<str>);

/// Allowed drop enchants (allowlist for `entries[].enchantments` keys).
pub const ALLOWED_DROP_ENCHANTS: &[&str] = &["minecraft:fortune"];
/// Fortune `bonus_by_level.level` range (1..=3).
pub const FORTUNE_LEVEL_MIN: u32 = 1;
pub const FORTUNE_LEVEL_MAX: u32 = 3;
/// Count-distribution normalization tolerance.
pub const COUNT_SUM_TOLERANCE: f64 = 1e-6;
/// `one_of` total-probability tolerance.
pub const ONE_OF_TOTAL_TOLERANCE: f64 = 1e-9;
/// Per-entry `value + fortune bonus` cap (bounds malformed configs that could flood the drop queue).
pub const MAX_DROP_COUNT_TOTAL: u32 = 1024;

/// One `count` distribution option.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountOption {
    pub value: u32,
    pub chance: f64,
}

/// One fortune-bonus row.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FortuneBonus {
    pub level: u32,
    pub count: u32,
    pub chance: f64,
}

/// `entries[].enchantments["minecraft:fortune"]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FortuneRule {
    pub bonus_by_level: Vec<FortuneBonus>,
}

/// `entries[].requires`: per-entry held-item condition (optional).
///
/// The held-item identifier must exactly match one of `items` for the entry to roll;
/// empty hand never matches. When absent, the entry has no held-item restriction.
/// Expresses tool-specific drops (e.g. a grass block dropping itself only with shears).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiresRule {
    pub items: Vec<String>,
}

/// One entry of `sc:drops.entries[]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropEntry {
    pub item: String,
    pub chance: f64,
    pub count: Vec<CountOption>,
    #[serde(default)]
    pub enchantments: std::collections::BTreeMap<String, FortuneRule>,
    /// Held-item condition (optional; absent means unrestricted).
    #[serde(default)]
    pub requires: Option<RequiresRule>,
}

/// `sc:drops` (direct drop definitions; `minecraft:loot` references resolve
/// through [`parse_loot_tables`]).
///
/// Harvest gate: with `require_harvest=true` (the default), a held tool that fails the harvest
/// requirement falls back to the default empty drop; with `false`, any tool rolls normally.
///
/// The registered component key (`NAME`) comes from the
/// `BlockComponents` `components_export!` list.
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Drops {
    pub enabled: bool,
    pub mode: String,
    #[serde(default)]
    pub entries: Vec<DropEntry>,
    /// Harvest-gate switch (defaults to true).
    #[serde(default)]
    pub require_harvest: Option<bool>,
}

// ---------------------------------------------------------------------------
// Direct loot-table parsing
// ---------------------------------------------------------------------------

/// Parsed loot-table component (raw JSON content, loaded directly).
///
/// `serde(transparent)` keeps the loot-table JSON as-is; consumers read the
/// table shape themselves (external loot-table rolling is follow-up work).
/// The component key (`NAME`) comes from the `BlockComponents`
/// `components_export!` list (`"sc:loot_tables"`).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(transparent)]
pub struct LootTables(pub Value);

/// The vanilla component whose value references a loot-table file.
pub const LOOT_REFERENCE_COMPONENT: &str = "minecraft:loot";

/// Parse one loot-table file (JSON bytes) into a [`LootTables`] component.
///
/// Direct load: comments are stripped (packs ship them) and the standard
/// pack JSON reader budget applies; anything else is taken as-is.
/// Returns `None` only when the file is not parseable JSON at all.
pub fn parse_loot_tables(bytes: &[u8]) -> Option<LootTables> {
    serde_json::from_reader(BudgetReader::new(StripComments::new(bytes)))
        .ok()
        .map(LootTables)
}

/// Resolve one `minecraft:loot` reference path into the parsed table value.
///
/// `resolve` maps the reference path (e.g. `loot_tables/blocks/oak_log.json`)
/// to the file bytes; the loader owning the pack source injects it.
pub fn resolve_loot_tables(
    path: &str,
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Option<Value> {
    let bytes = resolve(path)?;
    parse_loot_tables(&bytes).map(|LootTables(value)| value)
}

/// Attach parsed loot tables to a block-style component map
/// ([`std::collections::BTreeMap`], one map per component layer).
///
/// When the map carries a string-valued `minecraft:loot` reference and the
/// injected resolver finds the file, the parsed [`LootTables`] component is
/// inserted under [`LootTables::NAME`]. Unresolvable references are left
/// untouched (the path entry keeps its vanilla meaning; loading never fails
/// on a missing loot-table file).
pub fn attach_loot_tables(
    map: &mut std::collections::BTreeMap<String, Value>,
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) {
    let Some(Value::String(path)) = map.get(LOOT_REFERENCE_COMPONENT) else {
        return;
    };
    let path = path.clone();
    if let Some(table) = resolve_loot_tables(&path, resolve) {
        map.insert(LootTables::NAME.to_string(), table);
    }
}

/// Attach parsed loot tables to an entity-style component map
/// ([`serde_json::Map`], as produced by the `components_export!` enum).
///
/// Same contract as [`attach_loot_tables`]; unresolvable references are
/// ignored so behavior-pack entity loading never fails on them.
pub fn attach_entity_loot_tables(
    map: &mut serde_json::Map<String, Value>,
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) {
    let Some(Value::String(path)) = map.get(LOOT_REFERENCE_COMPONENT) else {
        return;
    };
    let path = path.clone();
    if let Some(table) = resolve_loot_tables(&path, resolve) {
        map.insert(LootTables::NAME.to_string(), table);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    const TABLE: &[u8] = br#"{
        // packs ship comments
        "pools": [
            {
                "rolls": 1,
                "entries": [ { "type": "item", "name": "minecraft:stick" } ]
            }
        ]
    }"#;

    #[test]
    fn parse_loot_tables_loads_json_directly() {
        let LootTables(value) = parse_loot_tables(TABLE).expect("应解析");
        assert_eq!(value["pools"][0]["entries"][0]["name"], "minecraft:stick");
        // Not JSON at all -> None, never a panic.
        assert!(parse_loot_tables(b"{oops").is_none());
    }

    #[test]
    fn attach_inserts_component_into_block_map() {
        let mut map = BTreeMap::new();
        map.insert(
            LOOT_REFERENCE_COMPONENT.to_string(),
            json!("loot_tables/blocks/oak_log.json"),
        );
        let mut resolve =
            |path: &str| (path == "loot_tables/blocks/oak_log.json").then(|| TABLE.to_vec());
        attach_loot_tables(&mut map, &mut resolve);
        assert!(map.contains_key(LootTables::NAME), "应插入 LootTables 组件");
        assert_eq!(
            map[LootTables::NAME]["pools"][0]["rolls"], 1,
            "内容应为解析后的表"
        );
        // The vanilla path reference stays untouched.
        assert_eq!(map[LOOT_REFERENCE_COMPONENT], "loot_tables/blocks/oak_log.json");
    }

    #[test]
    fn attach_entity_map_and_unresolvable_reference() {
        let mut map = serde_json::Map::new();
        map.insert(
            LOOT_REFERENCE_COMPONENT.to_string(),
            json!("loot_tables/entities/zombie.json"),
        );
        let mut resolve = |_: &str| None;
        attach_entity_loot_tables(&mut map, &mut resolve);
        assert!(
            !map.contains_key(LootTables::NAME),
            "无法解析时保持原样（不失败）"
        );

        let mut resolve = |path: &str| path.ends_with("zombie.json").then(|| TABLE.to_vec());
        attach_entity_loot_tables(&mut map, &mut resolve);
        assert!(map.contains_key(LootTables::NAME));
    }
}
