//! One `.block.json` file per block: discovery, bounded reads, and schema parsing on the version-pack side.
//!
//! Responsibility split: IO, path and total budgets, JSON syntax/schema parsing, and pathed bundles belong to packloader;
//! Responsibility split: IO, path and total budgets, JSON syntax/schema parsing, and pathed bundles belong to packloader;
//! cross-file semantic checks for states/ids/components and dense-table compilation belong to `sc_block`.
//! This module computes no state hashes and touches no gameplay semantic registries.
//!
//! File layout: `definitions/blocks/<namespace>/<path>.block.json`,
//! one identifier per file; the path must match the identifier.
//!
//! File bodies use the vanilla block JSON structure (`format_version` + `minecraft:block`,
//! `description` / `components` / `permutations`); the two server-required payloads with no vanilla
//! counterpart live under the `sc:` namespace and must be declared explicitly:
//!
//! - `sc:default_state`: default-state property assignment (missing means reject, never guess);
//! - `sc:protocol_runtime_ids`: protocol runtime ids in one-to-one canonical state order
//!   (protocol identity is never recomputed from sorting).
//!
//! Namespace: `sc:*` is the only canonical namespace. Any `ur:*` key is rejected with no compatibility handling
//! (version packs must use `sc:*`).

use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Supported `.block.json` schema version. Unknown versions are rejected.
pub const BLOCK_JSON_SCHEMA_VERSION: u32 = 1;

/// Block definition directory inside version packs (fixed, not configurable).
pub const BLOCKS_DIRECTORY: &str = "definitions/blocks";

/// Accepted vanilla `format_version` values (strings; all other values are rejected, never silently accepted).
pub const BLOCK_JSON_FORMAT_VERSION: &str = "1.10.0";

/// Allowed `block_data.network_id_mode` values.
pub const NETWORK_ID_MODE_HASHED: &str = "hashed";
pub const NETWORK_ID_MODE_PALETTE: &str = "palette";

// ---------------------------------------------------------------------------
// Error: pack identity + file path + field path, never silently falls back
// ---------------------------------------------------------------------------

/// Parse error with full location.
///
/// Displays as:
/// `Vanilla: definitions/blocks/minecraft/oak_log.block.json: $.palette[1].protocol_runtime_id: <duplicate-state message>`.
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

impl fmt::Display for BlockJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}: {}: {}",
            self.pack, self.file, self.field, self.message
        )
    }
}

impl std::error::Error for BlockJsonError {}

// ---------------------------------------------------------------------------
// Total budgets
// ---------------------------------------------------------------------------

/// Load-time total budgets. File/byte budgets run in the packloader discovery phase;
/// state/rule budgets run in schema parsing and `sc_block` compilation.
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
    /// Max `enchantments` keys per state (currently at most 1 in practice, budget keeps headroom).
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

/// Budget version (part of the fingerprint input; default changes alter the fingerprint).
pub const BLOCK_BUDGETS_VERSION: u32 = 4;

// ---------------------------------------------------------------------------
// Manifest declaration
// ---------------------------------------------------------------------------

fn default_blocks_dir() -> String {
    BLOCKS_DIRECTORY.to_string()
}

fn default_network_mode() -> String {
    NETWORK_ID_MODE_HASHED.to_string()
}

/// Version-pack manifest `block_data` section (new field; absent means use the legacy palette path).
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct BlockDataManifest {
    /// Block schema version (only 1; unknown versions are rejected).
    pub schema_version: u32,
    /// Block definition directory (fixed to `definitions/blocks`).
    #[serde(default = "default_blocks_dir")]
    pub directory: String,
    /// Network id mode (`hashed` | `palette`, only values the protocol build supports).
    #[serde(default = "default_network_mode")]
    pub network_id_mode: String,
}

impl BlockDataManifest {
    /// Validate the declaration itself (fixed dir, mode enum, schema version).
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
        if self.directory != BLOCKS_DIRECTORY {
            return Err(BlockJsonError::new(
                pack_id,
                "<manifest>",
                "$.block_data.directory",
                format!("directory 首版固定为 {BLOCKS_DIRECTORY:?}"),
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


#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap, HashSet};

    use sc_nbt::compound::CompoundNbt;
    use sc_nbt::NbtValue;
    use crate::block::component::{
        get_block_component, DestructibleByMining, LightDampening, LightEmission, Loot, Replaceable,
    };
    use std::io::{Cursor, Write};

    const PACK: &str = "test-pack";

    fn air_json() -> Vec<u8> {
        r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {"sc:replaceable": true},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]

            }
        }"#
        .as_bytes()
        .to_vec()
    }

    fn log_json() -> Vec<u8> {
        r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "minecraft:oak_log",
                    "states": {"pillar_axis": ["x", "y", "z"]}
                },
                "components": {
                    "minecraft:collision_box": {"origin": [-8, 0, -8], "size": [16, 16, 16]},
                    "minecraft:destructible_by_mining": {"value": 2.0},
                    "minecraft:light_emission": 0,
                    "minecraft:light_dampening": 15,
                    "minecraft:display_name": "Oak Log",
                    "minecraft:geometry": "geometry.oak_log"
                },
                "permutations": [
                    {
                        "condition": "q.block_property('minecraft:oak_log','pillar_axis') == 'y'",
                        "components": {"minecraft:loot": "loot_tables/blocks/oak_log.json"}
                    }
                ],
                "sc:default_state": {"pillar_axis": "y"},
                "sc:protocol_runtime_ids": [100, 101, 102]
            }
        }"#
        .as_bytes()
        .to_vec()
    }

    fn log_path() -> &'static str {
        "definitions/blocks/minecraft/oak_log.block.json"
    }

    fn air_path() -> &'static str {
        "definitions/blocks/minecraft/air.block.json"
    }

    #[test]
    fn path_contract_accepts_canonical_and_rejects_evil() {
        assert_eq!(
            validate_block_path(log_path(), 256).unwrap(),
            "minecraft:oak_log"
        );
        assert_eq!(
            validate_block_path(
                "definitions/blocks/example/machines/crusher.block.json",
                256
            )
            .unwrap(),
            "example:machines/crusher"
        );
        for bad in [
            "definitions/blocks/minecraft/../x.block.json",
            "definitions/blocks/minecraft//x.block.json",
            "definitions/blocks/minecraft/Oak_Log.block.json",
            "definitions\\blocks\\minecraft\\a.block.json",
            "other/blocks/minecraft/a.block.json",
            "definitions/blocks/minecraft/a.json",
            "definitions/blocks/.block.json",
            "definitions/blocks/minecraft/./a.block.json",
        ] {
            assert!(validate_block_path(bad, 256).is_err(), "应拒绝：{bad}");
        }
        assert!(validate_identifier("minecraft:oak_log", 256).is_ok());
        assert!(validate_identifier("no-namespace", 256).is_err());
        assert!(validate_identifier(":empty-ns", 256).is_err());
    }

    #[test]
    fn schema_parse_accepts_air_and_log() {
        let budgets = BlockBundleBudgets::default();
        let air = parse_block_file(PACK, air_path(), &air_json(), &budgets).expect("air 应合法");
        assert_eq!(air.identifier, "minecraft:air");
        assert!(air.properties.is_empty());
        assert_eq!(air.state_count(), 1);
        assert_eq!(air.default_index, 0);
        assert_eq!(air.network_ids, vec![0]);
        assert_eq!(air.state_key(0), "default");
        assert_eq!(
            get_block_component::<Replaceable>(&air.base).map(|r| r.0),
            Some(true)
        );

        let log = parse_block_file(PACK, log_path(), &log_json(), &budgets).expect("log 应合法");
        assert_eq!(log.identifier, "minecraft:oak_log");
        assert_eq!(log.properties.len(), 1);
        assert_eq!(log.properties[0].name, "pillar_axis");
        assert_eq!(log.properties[0].nbt_type, NbtPropType::String);
        assert_eq!(log.state_count(), 3);
        // Default state is the 2nd item in declaration value order ('y').
        assert_eq!(log.default_index, 1);
        assert_eq!(log.state_key(1), "pillar_axis=y");
        // Protocol runtime ids align in canonical order, never sorted by value.
        assert_eq!(log.network_ids, vec![100, 101, 102]);
        assert_eq!(
            get_block_component::<DestructibleByMining>(&log.base).map(|d| d.value),
            Some(2.0)
        );
        assert_eq!(
            get_block_component::<LightEmission>(&log.base).map(|l| l.0),
            Some(0)
        );
        assert_eq!(
            get_block_component::<LightDampening>(&log.base).map(|l| l.0),
            Some(15)
        );
        // Client-side entries are dropped and never enter base.
        assert!(!log.base.contains_key("minecraft:display_name"));
        assert!(!log.base.contains_key("minecraft:geometry"));
        // Permutation condition evaluation.
        assert_eq!(log.permutations.len(), 1);
        let perm = &log.permutations[0];
        assert!(perm.condition.matches(&log.state_props(1)));
        assert!(!perm.condition.matches(&log.state_props(0)));
        assert!(!perm.condition.matches(&log.state_props(2)));
        assert_eq!(
            get_block_component::<Loot>(&perm.components).map(|l| l.0.to_string()),
            Some("loot_tables/blocks/oak_log.json".to_string())
        );
        // Condition text round-trips.
        assert_eq!(
            perm.condition.to_expression("minecraft:oak_log"),
            "q.block_property('minecraft:oak_log','pillar_axis') == 'y'"
        );
    }

    /// Canonical state enumeration: property names ascending, rightmost varies fastest.
    #[test]
    fn canonical_enumeration_is_deterministic() {
        let props = vec![
            PropertyDef {
                name: "a".to_string(),
                nbt_type: NbtPropType::Byte,
                values: vec![PropValue::Byte(0), PropValue::Byte(1)],
            },
            PropertyDef {
                name: "b".to_string(),
                nbt_type: NbtPropType::Int,
                values: vec![PropValue::Int(0), PropValue::Int(1)],
            },
        ];
        assert_eq!(canonical_state_count(&props), 4);
        let keys: Vec<String> = (0..4)
            .map(|i| canonical_state_key(&canonical_state_props(&props, i)))
            .collect();
        assert_eq!(keys, vec!["a=0,b=0", "a=0,b=1", "a=1,b=0", "a=1,b=1"]);
        // Index math and enumeration are inverses.
        for (i, key) in keys.iter().enumerate() {
            let props_of_state = canonical_state_props(&props, i);
            let assignment: BTreeMap<String, PropValue> = props_of_state.into_iter().collect();
            assert_eq!(canonical_state_index(&props, &assignment), Some(i), "{key}");
        }
        let mut wrong = BTreeMap::new();
        wrong.insert("a".to_string(), PropValue::Byte(1));
        wrong.insert("b".to_string(), PropValue::Byte(1));
        assert_eq!(canonical_state_index(&props, &wrong), None);
    }

    /// Both vanilla `description.states` spellings are accepted; mixed types/duplicate values are rejected.
    #[test]
    fn states_forms_and_typing() {
        let budgets = BlockBundleBudgets::default();
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "test:forms",
                    "states": [
                        {"name": "lit", "values": [false, true]},
                        {"name": "age", "values": [0, 1, 2]}
                    ]
                },
                "sc:default_state": {"lit": true, "age": 2},
                "sc:protocol_runtime_ids": [1, 2, 3, 4, 5, 6]
            }
        }"#;
        let parsed = parse_block_file(
            PACK,
            "definitions/blocks/test/forms.block.json",
            json.as_bytes(),
            &budgets,
        )
        .expect("数组写法应合法");
        assert_eq!(parsed.properties.len(), 2);
        // Property names ascending: age < lit
        assert_eq!(parsed.properties[0].name, "age");
        assert_eq!(parsed.properties[0].nbt_type, NbtPropType::Int);
        assert_eq!(parsed.properties[1].name, "lit");
        assert_eq!(parsed.properties[1].nbt_type, NbtPropType::Byte);
        // age has 3 values, lit has 2; the rightmost (lit) varies fastest.
        assert_eq!(parsed.state_count(), 6);
        assert_eq!(parsed.state_key(0), "age=0,lit=0");
        assert_eq!(parsed.state_key(5), "age=2,lit=1");
        // Default state age=2, lit=true maps to 2*2 + 1 = 5
        assert_eq!(parsed.default_index, 5);

        // Mixed types in one property are rejected.
        let mixed = json.replace("\"values\": [false, true]", "\"values\": [false, 1]");
        let e = parse_block_file(
            PACK,
            "definitions/blocks/test/forms.block.json",
            mixed.as_bytes(),
            &budgets,
        )
        .expect_err("混合类型必须拒绝");
        assert!(e.to_string().contains("值类型必须一致"), "实际：{e}");
        // Duplicate values are rejected.
        let dup = json.replace("\"values\": [0, 1, 2]", "\"values\": [0, 1, 1]");
        parse_block_file(
            PACK,
            "definitions/blocks/test/forms.block.json",
            dup.as_bytes(),
            &budgets,
        )
        .expect_err("重复取值必须拒绝");
        // Float values are rejected (no lossy conversion).
        let float = json.replace("\"values\": [0, 1, 2]", "\"values\": [0, 1, 2.5]");
        parse_block_file(
            PACK,
            "definitions/blocks/test/forms.block.json",
            float.as_bytes(),
            &budgets,
        )
        .expect_err("浮点必须拒绝");
    }

    /// Restricted permutation-condition syntax: `&&` conjunctions accepted, the rest loudly rejected.
    #[test]
    fn condition_grammar_is_restricted_and_loud() {
        let budgets = BlockBundleBudgets::default();
        let properties = vec![PropertyDef {
            name: "pillar_axis".to_string(),
            nbt_type: NbtPropType::String,
            values: vec![
                PropValue::String("x".to_string()),
                PropValue::String("y".to_string()),
            ],
        }];
        let ok = |src: &str| {
            parse_permutation_condition(
                PACK,
                log_path(),
                "$.c",
                "minecraft:oak_log",
                &properties,
                src,
                &budgets,
            )
            .unwrap_or_else(|e| panic!("应接受 {src}：{e}"))
        };
        let conjoined = ok(
            "q.block_property('minecraft:oak_log','pillar_axis') == 'x' && q.block_property('minecraft:oak_log','pillar_axis') == 'x'",
        );
        assert_eq!(conjoined.tests.len(), 2);
        for bad in [
            "",
            "q.block_property('minecraft:oak_log','pillar_axis') == 'x' || q.block_property('minecraft:oak_log','pillar_axis') == 'y'",
            "q.block_property('minecraft:oak_log','pillar_axis') != 'x'",
            "q.block_property('other:block','pillar_axis') == 'x'",
            "q.block_property('minecraft:oak_log','nope') == 'x'",
            "q.block_property('minecraft:oak_log','pillar_axis') == 'z'",
            "q.block_property('minecraft:oak_log','pillar_axis') == query.something",
            "variable.x == 1",
        ] {
            parse_permutation_condition(
                PACK,
                log_path(),
                "$.c",
                "minecraft:oak_log",
                &properties,
                bad,
                &budgets,
            )
            .expect_err(&format!("应拒绝：{bad}"));
        }
    }

    #[test]
    fn schema_parse_rejects_missing_unknown_and_mismatched() {
        let budgets = BlockBundleBudgets::default();
        let air = String::from_utf8(air_json()).unwrap();
        // Unknown components (including ur:*, no compatibility handling).
        let bad = air.replace("\"sc:replaceable\": true", "\"ur:fly\": {}");
        let e = parse_block_file(PACK, air_path(), bad.as_bytes(), &budgets)
            .expect_err("未知组件必须拒绝");
        assert!(e.to_string().contains("未知组件"), "实际：{e}");
        // Missing sc:default_state (never guessed).
        let bad = air.replace("\"sc:default_state\": {},", "");
        let e = parse_block_file(PACK, air_path(), bad.as_bytes(), &budgets)
            .expect_err("缺默认态必须拒绝");
        assert!(e.to_string().contains("sc:default_state"), "实际：{e}");
        // Missing sc:protocol_runtime_ids (drop the whole line, avoid a dangling comma).
        let bad = air.replace(",\n                \"sc:protocol_runtime_ids\": [0]", "");
        let e = parse_block_file(PACK, air_path(), bad.as_bytes(), &budgets)
            .expect_err("缺 protocol id 必须拒绝");
        assert!(
            e.to_string().contains("sc:protocol_runtime_ids"),
            "实际：{e}"
        );
        // Protocol id count mismatches the state-combination count.
        let log = String::from_utf8(log_json()).unwrap();
        let bad = log.replace(
            "\"sc:protocol_runtime_ids\": [100, 101, 102]",
            "\"sc:protocol_runtime_ids\": [100, 101]",
        );
        let e = parse_block_file(PACK, log_path(), bad.as_bytes(), &budgets)
            .expect_err("protocol id 数量不符必须拒绝");
        assert!(e.to_string().contains("一一对应"), "实际：{e}");
        // Negative/non-integer protocol ids.
        let bad = log.replace("[100, 101, 102]", "[-1, 101, 102]");
        parse_block_file(PACK, log_path(), bad.as_bytes(), &budgets)
            .expect_err("负 protocol id 必须拒绝");
        // Default-state value outside the declared domain.
        let bad = log.replace(
            "\"sc:default_state\": {\"pillar_axis\": \"y\"}",
            "\"sc:default_state\": {\"pillar_axis\": \"w\"}",
        );
        let e = parse_block_file(PACK, log_path(), bad.as_bytes(), &budgets)
            .expect_err("越界默认态必须拒绝");
        assert!(e.to_string().contains("声明值"), "实际：{e}");
        // Default state missing a property.
        let bad = log.replace(
            "\"sc:default_state\": {\"pillar_axis\": \"y\"}",
            "\"sc:default_state\": {}",
        );
        let e = parse_block_file(PACK, log_path(), bad.as_bytes(), &budgets)
            .expect_err("默认态缺属性必须拒绝");
        assert!(e.to_string().contains("覆盖全部"), "实际：{e}");
        // destructible_by_mining and sc:unbreakable are mutually exclusive.
        let bad = air.replace(
            "\"sc:replaceable\": true",
            "\"minecraft:destructible_by_mining\": {\"value\": 1.0}, \"sc:unbreakable\": {}",
        );
        let e = parse_block_file(PACK, air_path(), bad.as_bytes(), &budgets)
            .expect_err("互斥组件必须拒绝");
        assert!(e.to_string().contains("不能同时声明"), "实际：{e}");
        // Unsupported format_version.
        let bad = air.replace("\"1.10.0\"", "\"1.21.0\"");
        let e = parse_block_file(PACK, air_path(), bad.as_bytes(), &budgets)
            .expect_err("未知 format_version 必须拒绝");
        assert!(e.to_string().contains("format_version"), "实际：{e}");
        // Unknown top-level field.
        let bad = air.replace("\"format_version\"", "\"schema_version\"");
        let e = parse_block_file(PACK, air_path(), bad.as_bytes(), &budgets)
            .expect_err("未知顶层字段必须拒绝");
        assert!(e.to_string().contains("未知字段"), "实际：{e}");
        // Identifier mismatches the path.
        let e = parse_block_file(
            PACK,
            "definitions/blocks/minecraft/birch_log.block.json",
            &log_json(),
            &budgets,
        )
        .expect_err("路径不一致必须拒绝");
        assert!(e.to_string().contains("不一致"), "实际：{e}");
        // Duplicate JSON key.
        let dup = air.replace("\"sc:protocol_runtime_ids\": [0]", "\"a\": 1, \"a\": 2");
        let e = parse_block_file(PACK, air_path(), dup.as_bytes(), &budgets)
            .expect_err("重复 key 必须拒绝");
        assert!(e.to_string().contains("重复 key"), "实际：{e}");
    }

    #[test]
    fn manifest_validation_rejects_unknown_schema_dir_and_mode() {
        let ok = BlockDataManifest {
            schema_version: 1,
            directory: BLOCKS_DIRECTORY.to_string(),
            network_id_mode: "hashed".to_string(),
        };
        ok.validate("pack").expect("合法声明");
        for bad in [
            BlockDataManifest {
                schema_version: 99,
                directory: BLOCKS_DIRECTORY.to_string(),
                network_id_mode: "hashed".to_string(),
            },
            BlockDataManifest {
                schema_version: 1,
                directory: "other/dir".to_string(),
                network_id_mode: "hashed".to_string(),
            },
            BlockDataManifest {
                schema_version: 1,
                directory: BLOCKS_DIRECTORY.to_string(),
                network_id_mode: "ordered".to_string(),
            },
        ] {
            bad.validate("pack").expect_err("非法声明必须拒绝");
        }
    }

    fn make_source(entries: &[(&str, &[u8])]) -> crate::pack_loader::pack::zipped::PackSource {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        crate::pack_loader::pack::zipped::PackSource::from_bytes(bytes).unwrap()
    }

    fn test_manifest() -> BlockDataManifest {
        BlockDataManifest {
            schema_version: 1,
            directory: BLOCKS_DIRECTORY.to_string(),
            network_id_mode: "hashed".to_string(),
        }
    }

    #[test]
    fn discovery_sorts_and_parses_with_file_paths() {
        // Inserted in reverse on purpose: scan results must sort by canonical path.
        let air = air_json();
        let log = log_json();
        let mut pack = make_source(&[
            (log_path(), &log),
            (air_path(), &air),
            ("other/notes.txt", b"ignore me"),
        ]);
        let bundle = load_block_json_bundle(
            &mut pack,
            &test_manifest(),
            PACK,
            &BlockBundleBudgets::default(),
        )
        .expect("发现应成功")
        .expect("bundle 应存在");
        assert_eq!(bundle.files.len(), 2);
        assert_eq!(bundle.files[0].identifier, "minecraft:air");
        assert_eq!(bundle.files[1].identifier, "minecraft:oak_log");
        assert_eq!(bundle.network_id_mode, "hashed");
    }

    #[test]
    fn discovery_fails_loud_on_empty_badpath_and_broken() {
        // Zero files.
        let mut pack = make_source(&[("manifest.json", b"{}")]);
        load_block_json_bundle(
            &mut pack,
            &test_manifest(),
            PACK,
            &BlockBundleBudgets::default(),
        )
        .expect_err("零文件必须失败");
        // Illegal path (../).
        let air = air_json();
        let mut pack = make_source(&[("definitions/blocks/minecraft/../evil.block.json", &air)]);
        let e = load_block_json_bundle(
            &mut pack,
            &test_manifest(),
            PACK,
            &BlockBundleBudgets::default(),
        )
        .expect_err("穿越路径必须失败");
        assert!(e.to_string().contains("非法"), "实际：{e}");
        // Broken JSON: never skipped, must fail with the file path.
        let mut pack = make_source(&[(air_path(), b"{oops")]);
        let e = load_block_json_bundle(
            &mut pack,
            &test_manifest(),
            PACK,
            &BlockBundleBudgets::default(),
        )
        .expect_err("坏文件必须失败");
        assert!(e.file.contains("air.block.json"), "实际：{e}");
        // Broken schema (missing default state): fails the same way.
        let bad = String::from_utf8(air_json())
            .unwrap()
            .replace("\"sc:default_state\": {},", "");
        let mut pack = make_source(&[(air_path(), bad.as_bytes())]);
        load_block_json_bundle(
            &mut pack,
            &test_manifest(),
            PACK,
            &BlockBundleBudgets::default(),
        )
        .expect_err("坏 schema 必须失败");
        // File-count budget.
        let air = air_json();
        let mut pack = make_source(&[(air_path(), &air)]);
        let mut tight = BlockBundleBudgets::default();
        tight.max_files = 0;
        load_block_json_bundle(&mut pack, &test_manifest(), PACK, &tight).expect_err("文件数预算");
    }

    /// Split-to-reparse round-trip: multi-property cartesian product + protocol id alignment + default-state placement.
    #[test]
    fn split_round_trip_preserves_ids_and_default() {
        let budgets = BlockBundleBudgets::default();
        let entry = |name: &str, states: Vec<(&str, NbtValue)>, network_id: u32| {
            let mut compound = CompoundNbt::new(None);
            for (k, v) in states {
                compound.insert(k, v);
            }
            LegacyPaletteEntry {
                name: name.to_string(),
                states: Some(compound),
                version: 18161159,
                network_id,
                extra: None,
            }
        };
        // Source order differs from canonical on purpose: lit/age combos, protocol ids 8..12.
        let entries = vec![
            entry(
                "test:mixed",
                vec![("age", NbtValue::Int(1)), ("lit", NbtValue::Byte(1))],
                12,
            ),
            entry(
                "test:mixed",
                vec![("age", NbtValue::Int(0)), ("lit", NbtValue::Byte(0))],
                8,
            ),
            entry(
                "test:mixed",
                vec![("age", NbtValue::Int(0)), ("lit", NbtValue::Byte(1))],
                9,
            ),
            entry(
                "test:mixed",
                vec![("age", NbtValue::Int(1)), ("lit", NbtValue::Byte(0))],
                10,
            ),
            entry(
                "test:mixed",
                vec![("age", NbtValue::Int(0)), ("lit", NbtValue::Byte(0))],
                8,
            ),
        ];
        // Duplicated source combos must fail (no silent dedup).
        let err = split_legacy_palette(&entries, &HashMap::new(), &HashMap::new(), &budgets)
            .expect_err("重复组合必须失败");
        assert!(err.to_string().contains("重复"), "实际：{err}");

        // Split after dedup: default state explicitly declares age=1/lit=1 (canonical index 3).
        let combo_key = |e: &LegacyPaletteEntry| {
            let mut parts: Vec<String> = e
                .states
                .as_ref()
                .map(|s| {
                    s.iter()
                        .map(|(k, v)| format!("{k}={}", format!("{v:?}")))
                        .collect()
                })
                .unwrap_or_default();
            parts.sort();
            parts.join(",")
        };
        let unique: Vec<LegacyPaletteEntry> = {
            let mut seen = HashSet::new();
            entries
                .iter()
                .filter(|e| seen.insert(combo_key(e)))
                .cloned()
                .collect()
        };
        assert_eq!(unique.len(), 4);
        let mut defaults = HashMap::new();
        // Multi-property split state key is `state_{source-order index}`: (age=1,lit=1) is item 0 in source order.
        defaults.insert("test:mixed".to_string(), "state_0".to_string());
        let mut hardness = HashMap::new();
        hardness.insert("test:mixed".to_string(), BlockHardness::Breakable(1.5));
        let files =
            split_legacy_palette(&unique, &defaults, &hardness, &budgets).expect("拆分应成功");
        assert_eq!(files.len(), 1);
        let parsed = parse_block_file("<test>", &files[0].zip_path, &files[0].bytes, &budgets)
            .expect("重解析应成功");
        // Canonical order: age(0,1) x lit(false,true), rightmost lit varies fastest.
        assert_eq!(parsed.state_count(), 4);
        assert_eq!(parsed.network_ids, vec![8, 9, 10, 12]);
        assert_eq!(parsed.default_index, 3);
        assert_eq!(parsed.state_key(3), "age=1,lit=1");
        assert_eq!(
            get_block_component::<DestructibleByMining>(&parsed.base).map(|d| d.value),
            Some(1.5)
        );

        // Non-cartesian product (age domain {0,1} but only diagonal combos) must fail; no default fill.
        let sparse = vec![
            entry(
                "test:mixed",
                vec![("age", NbtValue::Int(0)), ("lit", NbtValue::Byte(0))],
                8,
            ),
            entry(
                "test:mixed",
                vec![("age", NbtValue::Int(1)), ("lit", NbtValue::Byte(1))],
                9,
            ),
        ];
        let mut sparse_defaults = HashMap::new();
        sparse_defaults.insert("test:mixed".to_string(), "state_0".to_string());
        let err = split_legacy_palette(&sparse, &sparse_defaults, &HashMap::new(), &budgets)
            .expect_err("非完整笛卡尔积必须失败");
        assert!(err.to_string().contains("笛卡尔积"), "实际：{err}");

        // Multi-state block without a default state goes to the pending list (batch return, never the first state).
        let err = split_legacy_palette(&unique, &HashMap::new(), &HashMap::new(), &budgets)
            .expect_err("缺默认态必须失败");
        match err {
            SplitError::MissingDefaults { ids, .. } => assert_eq!(ids, vec!["test:mixed".to_string()]),
            other => panic!("应进入待补齐清单，实际：{other}"),
        }
    }

    /// Real version-pack stats: measured basis for budget defaults (single-state subset split + reparse).
    #[test]
    fn real_pack_stats_and_single_state_subset_round_trip() {
        let Some(mut source) = crate::test_pack::open_test_pack() else {
            eprintln!("[skip] 版本包缺失，跳过测试");
            return;
        };
        let palette_bytes = {
            let buf = source
                .read_file("definitions/block_palette.nbt")
                .expect("palette 缺失");
            buf
        };
        let entries = parse_legacy_palette_entries(&palette_bytes).expect("旧 palette 解析");
        // Stats: type/state counts, property-type distribution, unknown root fields.
        let mut by_name: HashMap<&str, usize> = HashMap::new();
        let mut prop_tags: HashMap<u8, usize> = HashMap::new();
        let mut extra_keys: HashMap<String, usize> = HashMap::new();
        let mut versions: HashMap<i32, usize> = HashMap::new();
        for e in entries.iter() {
            *by_name.entry(e.name.as_str()).or_default() += 1;
            if let Some(states) = e.states.as_ref() {
                for (_, v) in states.iter() {
                    *prop_tags.entry(v.tag()).or_default() += 1;
                }
            }
            if let Some(extra) = e.extra.as_ref() {
                for (k, _) in extra.iter() {
                    *extra_keys.entry(k.clone()).or_default() += 1;
                }
            }
            *versions.entry(e.version).or_default() += 1;
        }
        let single: Vec<&str> = by_name
            .iter()
            .filter(|(_, c)| **c == 1)
            .map(|(k, _)| *k)
            .collect();
        let max_per_type = by_name.values().copied().max().unwrap_or(0);
        eprintln!(
            "[real-pack] entries={} types={} single_state_types={} max_states_per_type={} prop_tags={:?} extra_keys={:?} versions={:?}",
            entries.len(),
            by_name.len(),
            single.len(),
            max_per_type,
            prop_tags,
            extra_keys,
            versions,
        );
        assert!(entries.len() > 1000, "真实 palette 应有规模");
        assert!(by_name.len() < BlockBundleBudgets::default().max_files);
        assert!(entries.len() < BlockBundleBudgets::default().max_total_states);
        assert!(max_per_type <= BlockBundleBudgets::default().max_states_per_block);
        // Single-state subset: the only state pins the default state (not guessed); split + reparse must succeed.
        let subset: Vec<String> = {
            let mut v: Vec<String> = single.iter().map(|s| s.to_string()).collect();
            v.sort();
            v.into_iter().take(300).collect()
        };
        let subset_entries: Vec<LegacyPaletteEntry> = entries
            .iter()
            .filter(|e| subset.contains(&e.name))
            .cloned()
            .collect();
        assert_eq!(subset_entries.len(), subset.len());
        let budgets = BlockBundleBudgets::default();
        let files =
            split_legacy_palette(&subset_entries, &HashMap::new(), &HashMap::new(), &budgets)
                .expect("单状态子集拆分应成功");
        assert_eq!(files.len(), subset.len());
        for f in files.iter() {
            assert!(
                f.zip_path.starts_with("definitions/blocks/"),
                "{}",
                f.zip_path
            );
        }
    }
}

pub mod bundle;
pub mod parse;
pub mod validate;

pub use bundle::{
    fingerprint_bundle, fingerprint_bundle_with_budgets, load_block_json_bundle,
    parse_hardness_json, parse_legacy_palette_entries, split_legacy_palette, split_state_key,
    BlockHardness, BlockJsonBundle, GeneratedBlockFile, LegacyPaletteEntry, SplitError,
};
pub use parse::{
    canonical_state_count, canonical_state_index, canonical_state_key, canonical_state_props,
    parse_block_file, parse_permutation_condition, ConditionTest, NbtPropType, ParsedBlockFile,
    ParsedPermutation, PermutationCondition, PropValue, PropertyDef,
};
pub use validate::{reject_duplicate_keys, validate_block_path, validate_identifier};
