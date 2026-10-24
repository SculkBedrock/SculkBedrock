//! `.json` 文件的语义编译：产出不可变注册表快照（位于 `sc_block`）。
//!
//! Semantic compilation of `.json` files into an immutable registry snapshot (in `sc_block`).
//!
//! 职责划分：`sc_packloader::block` 域负责发现、配额封顶的读取与 schema 解析
//! （`block::component` 的组件结构注册表，镜像 `item::component`；文件体是原版
//! `format_version` + `minecraft:block` 结构），而本模块负责全部跨文件语义检查
//! （默认状态、id 冲突、哈希碰撞、覆盖重叠），并在私有构建流程完成后原子发布
//! 不可变的 [`BlockJsonSnapshot`]。
//!
//! Responsibility split: the `sc_packloader::block` domain handles discovery, quota-capped reads, and
//! schema parsing (component struct registry in `block::component`, mirroring `item::component`; file body is the
//! vanilla `format_version` + `minecraft:block` structure), while this module handles all cross-file semantic
//! checks (default states, id conflicts, hash collisions, overlay overlaps) and atomically publishes
//! the immutable [`BlockJsonSnapshot`] once the private builder flow completes.
//!
//! Tick 热路径只读快照与稠密表，绝不解析 JSON。
//!
//! Tick hot paths only read the snapshot and dense tables, never parse JSON.
//!
//! 标识契约：内部稠密下标（按标识符与类型化规范状态序分配）、FNV 状态哈希
//! （规范 `sc_world::leveldb::block_hash` 算法）、协议顺序 id
//! （`sc:protocol_runtime_ids` 原样保留，绝不按排序重算）与物品 id 彼此独立。
//! `minecraft:unknown` 保持 `-2` 哨兵；任何哈希碰撞即拒绝整个包。
//!
//! Identity contract: internal dense indices (assigned by identifier and typed canonical state order),
//! FNV state hashes (canonical `sc_world::leveldb::block_hash` algorithm), protocol sequential ids
//! (`sc:protocol_runtime_ids` kept verbatim, never recomputed by sort order), and item ids stay
//! independent. `minecraft:unknown` keeps the `-2` sentinel; any hash collision rejects the whole pack.
//!
//! 本模块绝不猜测玩法数据：未声明的组件编译为 `None`（未声明），
//! 而不继承石头式默认值。
//!
//! This module never guesses gameplay data: undeclared components compile to `None` (undeclared)
//! instead of inheriting stone-like defaults.

use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use sc_packloader::block::PropValue;
#[cfg(test)]
use sc_packloader::block::{BlockBundleBudgets, BlockJsonBundle};

pub mod compile;
pub mod legacy;
pub mod snapshot;

pub use compile::compile_bundle;
pub use legacy::{
    compare_legacy_snapshot, global_preexisting_lookup, publish_legacy_support,
    BlockJsonRegistry, EquivalenceReport,
};
pub use snapshot::{BlockJsonSnapshot, SnapState, SnapType};

// ---------------------------------------------------------------------------
// Typed canonical state keys (unambiguous identity for the new compile path; replaces the old
// state_signature string-concat/byte-bool behavior).
// ---------------------------------------------------------------------------

/// 标识符 + 排序后属性到无歧义键（带长度前缀，防分隔符伪造）。
///
/// identifier plus sorted properties to an unambiguous key (length-prefixed, separator-forgery safe).
///
/// - `identifier`: 方块标识符
/// - `props`: 属性名到属性值列表
pub fn typed_state_key(identifier: &str, props: &[(String, PropValue)]) -> Box<str> {
    let mut out = String::new();
    out.push_str(&format!("{}:{};", identifier.len(), identifier));
    let mut sorted: Vec<(&String, &PropValue)> = props.iter().map(|(k, v)| (k, v)).collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (name, value) in sorted {
        out.push_str(&format!("{}:{};", name.len(), name));
        let mut key = String::new();
        sort_key_for_nbt_value(&mut key, &value.to_nbt());
        out.push_str(&key);
    }
    Box::from(out)
}

/// 属性赋值到等价 states NBT（磁盘调色板/全局字典共享同一属性标识）。
///
/// Property assignments to equivalent states NBT (disk palette / global dictionary share the same property identity).
///
/// - `props`: 属性名到属性值列表
pub fn states_nbt_of(props: &[(String, PropValue)]) -> CompoundNbt {
    let mut out = CompoundNbt::new(None);
    for (name, value) in props.iter() {
        out.insert(name, value.to_nbt());
    }
    out
}

/// 单个查询键值对与类型化快照属性的宽松字符串匹配
/// （规则同全局字典在 NBT 值上的匹配器）。
///
/// Lenient string match of one query pair against typed snapshot props
/// (same rules as the global dictionary matcher over NBT values).
///
/// - `props`: 类型化属性列表
/// - `key`: 属性名
/// - `value`: 查询字符串值
fn snapshot_prop_matches(props: &[(String, PropValue)], key: &str, value: &str) -> bool {
    let Some((_, prop)) = props.iter().find(|(k, _)| k == key) else {
        return false;
    };
    match prop {
        PropValue::String(s) => s == value,
        PropValue::Int(i) => value.parse::<i32>().is_ok_and(|q| q == *i),
        PropValue::Byte(b) => match value {
            "true" => *b != 0,
            "false" => *b == 0,
            _ => value.parse::<i32>().is_ok_and(|q| q == *b as i32),
        },
    }
}

/// 全部查询键值对均匹配才算匹配（缺键即失败）。
///
/// All query pairs match (missing keys fail).
///
/// - `props`: 类型化属性列表
/// - `pairs`: 查询键值对
fn snapshot_props_match(props: &[(String, PropValue)], pairs: &[(&str, &str)]) -> bool {
    pairs
        .iter()
        .all(|(key, value)| snapshot_prop_matches(props, key, value))
}

/// NBT 值到类型化比较键片段。
///
/// NBT value to typed comparison key fragment.
///
/// byte/int/string 与 [`typed_state_key`] **字节一致地**编码（同一函数、同一键），
/// 从而快照状态可与旧调色板/全局字典逐状态比对；
/// 其他旧类型（short/long/float/数组/嵌套）没有对应 `PropValue`，使用 `~t`
/// 前缀 + 类型标签 + Debug 文本，其前缀绝不会与三种标准编码碰撞。
///
/// byte/int/string encode **byte-identically** to [`typed_state_key`] (same function, same key),
/// so snapshot states compare per-state against the legacy palette / global dictionary;
/// other legacy types (short/long/float/array/nested) have no `PropValue`, using a `~t`
/// prefix plus type tag plus Debug text whose prefix can never collide with the three standard encodings.
///
/// - `out`: 输出字符串（追加写入）
/// - `v`: 待编码的 NBT 值
fn sort_key_for_nbt_value(out: &mut String, v: &NbtValue) {
    match v {
        NbtValue::Byte(b) => {
            let repr = b.to_string();
            out.push_str(&format!("b:{}:{};", repr.len(), repr));
        }
        NbtValue::Int(i) => {
            let repr = i.to_string();
            out.push_str(&format!("i:{}:{};", repr.len(), repr));
        }
        NbtValue::String(s) => {
            out.push_str(&format!("s{}:{};", s.len(), s));
        }
        other => out.push_str(&format!("~t{}:{:?};", other.tag(), other)),
    }
}

/// 任意 NBT states（含 byte/int/string 之外的旧类型）到类型化比较键。
/// 在三种属性类型上与 [`typed_state_key`] 一致。
///
/// Arbitrary NBT states (including legacy types beyond byte/int/string) to typed comparison keys.
/// Matches [`typed_state_key`] on the three property types.
///
/// - `identifier`: 方块标识符
/// - `states`: 原始 states NBT（`None` 表示无属性）
pub fn typed_key_for_nbt(identifier: &str, states: Option<&CompoundNbt>) -> Box<str> {
    let mut out = format!("{}:{};", identifier.len(), identifier);
    let Some(states) = states else {
        return Box::from(out);
    };
    let mut keys: Vec<&String> = states.iter().map(|(k, _)| k).collect();
    keys.sort();
    for k in keys {
        if let Some(v) = states.get(k) {
            out.push_str(&format!("{}:{};", k.len(), k));
            sort_key_for_nbt_value(&mut out, v);
        }
    }
    Box::from(out)
}


#[cfg(test)]
mod tests {
    use super::*;
    use sc_packloader::block::{fingerprint_bundle, parse_block_file};

    const PACK: &str = "test-pack";
    const NO_PREEXISTING: &dyn Fn(u32) -> Option<(Box<str>, Box<str>)> = &|_| None;
    const ALLOW_ALL_ITEMS: &dyn Fn(&str) -> bool = &|_| true;

    fn raw_air() -> (String, Vec<u8>) {
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {"sc:replaceable": true},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        (
            "blocks/minecraft/air.json".to_string(),
            json.as_bytes().to_vec(),
        )
    }

    fn raw_log() -> (String, Vec<u8>) {
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "minecraft:oak_log",
                    "states": {"pillar_axis": ["y", "x", "z"]}
                },
                "components": {
                    "minecraft:collision_box": {"origin": [-8, 0, -8], "size": [16, 16, 16]},
                    "minecraft:destructible_by_mining": {"value": 2.0},
                    "minecraft:light_emission": 0,
                    "minecraft:light_dampening": 15
                },
                "permutations": [
                    {
                        "condition": "q.block_property('minecraft:oak_log','pillar_axis') == 'y'",
                        "components": {"minecraft:loot": "loot_tables/blocks/oak_log.json"}
                    }
                ],
                "sc:protocol_runtime_ids": [100, 101, 102]
            }
        }"#;
        (
            "blocks/minecraft/oak_log.json".to_string(),
            json.as_bytes().to_vec(),
        )
    }

    fn bundle_of(raws: &[(String, Vec<u8>)], mode: &str) -> BlockJsonBundle {
        bundle_of_with_tables(raws, mode, std::collections::BTreeMap::new())
    }

    fn bundle_of_with_tables(
        raws: &[(String, Vec<u8>)],
        mode: &str,
        loot_tables: std::collections::BTreeMap<String, serde_json::Value>,
    ) -> BlockJsonBundle {
        let mut refs: Vec<(&str, &[u8])> = raws
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let fingerprint = fingerprint_bundle(&refs, mode);
        let mut files = Vec::with_capacity(refs.len());
        for (path, bytes) in refs {
            files.push(parse_block_file(PACK, path, bytes).expect("夹具应合法"));
        }
        BlockJsonBundle {
            schema_version: 1,
            network_id_mode: mode.to_string(),
            fingerprint,
            files,
            loot_tables,
        }
    }

    fn compile_two() -> BlockJsonSnapshot {
        let bundle = bundle_of(&[raw_air(), raw_log()], "hashed");
        compile_bundle(
            &bundle,
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("基础 bundle 应编译成功")
        .0
    }

    #[test]
    fn log_and_air_cover_all_states_with_unique_default() {
        let snap = compile_two();
        assert_eq!(snap.type_count(), 2);
        assert_eq!(snap.state_count(), 4);
        let log = snap
            .type_by_identifier("minecraft:oak_log")
            .expect("oak_log 类型");
        assert_eq!(log.state_ids.len(), 3);
        let def = snap.state(log.default_state).expect("默认态");
        assert_eq!(def.key.as_ref(), "pillar_axis=y");
        let by_net: Vec<u32> = log
            .state_ids
            .iter()
            .map(|i| snap.state(*i).unwrap().network_id)
            .collect();
        assert!(by_net.contains(&100) && by_net.contains(&101) && by_net.contains(&102));
        let mut states = CompoundNbt::new(None);
        states.insert("pillar_axis", NbtValue::String("y".to_string()));
        assert_eq!(
            def.hash,
            sc_world::leveldb::block_hash::block_state_hash("minecraft:oak_log", Some(&states))
        );
    }

    #[test]
    fn typed_lookup_distinguishes_byte_and_int() {
        // 4 states: lit (2 byte values) x age (2 int values) x s (1 string value).
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "test:mixed",
                    "states": [
                        {"name": "lit", "values": [false, true]},
                        {"name": "age", "values": [0, 1]},
                        {"name": "s", "values": ["a"]}
                    ]
                },
                "sc:protocol_runtime_ids": [1, 2, 3, 4]
            }
        }"#;
        let mixed = (
            "blocks/test/mixed.json".to_string(),
            json.as_bytes().to_vec(),
        );
        let bundle = bundle_of(&[raw_air(), mixed], "hashed");
        let snap = compile_bundle(
            &bundle,
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("混合类型 bundle 应成功")
        .0;
        assert_eq!(snap.state_count(), 5); // air 1 + mixed 4
        let s0 = snap
            .typed_state_of(
                "test:mixed",
                &[
                    ("lit".to_string(), PropValue::Byte(0)),
                    ("age".to_string(), PropValue::Int(0)),
                    ("s".to_string(), PropValue::String("a".to_string())),
                ],
            )
            .expect("s0");
        assert_eq!(
            snap.with_property_typed(s0, "lit", PropValue::Byte(1)),
            snap.typed_state_of(
                "test:mixed",
                &[
                    ("lit".to_string(), PropValue::Byte(1)),
                    ("age".to_string(), PropValue::Int(0)),
                    ("s".to_string(), PropValue::String("a".to_string())),
                ],
            )
        );
        // byte(1) and int(1) are distinct states.
        assert_ne!(
            snap.with_property_typed(s0, "lit", PropValue::Byte(1)),
            snap.with_property_typed(s0, "age", PropValue::Int(1)),
        );
        assert_eq!(
            snap.with_property_typed(s0, "nope", PropValue::Byte(1)),
            None
        );
        // Property identity matches states NBT (shared by disk writes and the dictionary).
        let st = snap.state(s0).unwrap();
        let nbt = states_nbt_of(&st.props);
        assert!(matches!(nbt.get("lit"), Some(NbtValue::Byte(0))));
        assert!(matches!(nbt.get("age"), Some(NbtValue::Int(0))));
        assert!(matches!(nbt.get("s"), Some(NbtValue::String(v)) if v == "a"));
    }

    #[test]
    fn find_state_hash_matches_lenient_string_queries() {
        // Same mixed bundle: lit (byte) x age (int) x s (string), 4 states.
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "test:mixed",
                    "states": [
                        {"name": "lit", "values": [false, true]},
                        {"name": "age", "values": [0, 1]},
                        {"name": "s", "values": ["a"]}
                    ]
                },
                "sc:protocol_runtime_ids": [1, 2, 3, 4]
            }
        }"#;
        let mixed = (
            "blocks/test/mixed.json".to_string(),
            json.as_bytes().to_vec(),
        );
        let bundle = bundle_of(&[raw_air(), mixed], "hashed");
        let snap = compile_bundle(
            &bundle,
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("mixed bundle compiles")
        .0;
        let h = |pairs: &[(&str, &str)]| snap.find_state_hash("test:mixed", pairs);
        // Byte matches false/true and numeric spellings alike.
        let b0 = h(&[("lit", "false"), ("age", "0"), ("s", "a")]).expect("lit=false");
        assert_eq!(b0, h(&[("lit", "0"), ("age", "0"), ("s", "a")]).expect("lit=0"));
        let b1 = h(&[("lit", "true"), ("age", "0"), ("s", "a")]).expect("lit=true");
        assert_ne!(b0, b1);
        // Order-independent; int and string match by value.
        assert_eq!(
            h(&[("age", "1"), ("s", "a"), ("lit", "false")]),
            h(&[("lit", "false"), ("age", "1"), ("s", "a")])
        );
        // Misses: unknown identifier, unknown key, out-of-range value.
        assert_eq!(snap.find_state_hash("test:nope", &[("lit", "false")]), None);
        assert_eq!(snap.find_state_hash("test:mixed", &[("nope", "0")]), None);
        assert_eq!(
            snap.find_state_hash("test:mixed", &[("lit", "false"), ("age", "7"), ("s", "a")]),
            None
        );
        // Full state set helper.
        assert_eq!(snap.state_hashes_of("test:mixed").len(), 4);
        assert!(snap.state_hashes_of("test:nope").is_empty());
        // Agrees with the typed API (which returns the dense index).
        let idx = snap
            .typed_state_of(
                "test:mixed",
                &[
                    ("lit".to_string(), PropValue::Byte(0)),
                    ("age".to_string(), PropValue::Int(0)),
                    ("s".to_string(), PropValue::String("a".to_string())),
                ],
            )
            .expect("typed s0");
        assert_eq!(b0, snap.state(idx).expect("state").hash);
    }

    /// Fixed dense columns: compiles exactly what is declared, undeclared stays None.
    #[test]
    fn dense_columns_follow_declarations_only() {
        let snap = compile_two();
        let log = snap
            .type_by_identifier("minecraft:oak_log")
            .expect("oak_log");
        let axis_y = log
            .state_ids
            .iter()
            .copied()
            .find(|i| snap.state(*i).unwrap().key.as_ref() == "pillar_axis=y")
            .expect("axis_y");
        let axis_x = log
            .state_ids
            .iter()
            .copied()
            .find(|i| snap.state(*i).unwrap().key.as_ref() == "pillar_axis=x")
            .expect("axis_x");

        // Shared components cover all states.
        assert_eq!(snap.mining_seconds_of(axis_y), Some(2.0));
        assert_eq!(snap.mining_seconds_of(axis_x), Some(2.0));
        assert_eq!(snap.light_emission_of(axis_y), Some(0));
        assert_eq!(snap.light_dampening_of(axis_y), Some(15));
        assert!(!snap.is_unbreakable(axis_y));
        assert_eq!(
            snap.collision_of(axis_y).map(|b| b.as_slice()),
            Some(&[[-8.0, 0.0, -8.0, 16.0, 16.0, 16.0]][..])
        );
        // Permutations apply only to matched states.
        assert_eq!(
            snap.loot_of(axis_y),
            Some("loot_tables/blocks/oak_log.json")
        );
        assert_eq!(snap.loot_of(axis_x), None);
        // air declares only replaceable, the rest undeclared.
        let air = snap
            .type_by_identifier("minecraft:air")
            .expect("air")
            .default_state;
        assert_eq!(snap.replaceable_of(air), Some(true));
        assert_eq!(snap.mining_seconds_of(air), None);
        assert_eq!(snap.light_emission_of(air), None);
        assert_eq!(snap.loot_of(air), None);
        // Flags for air.
        assert!(snap.flag_bits(air) & crate::state::flags::IS_AIR != 0);
        assert!(snap.flag_bits(air) & crate::state::flags::HAS_STATES == 0);
        assert!(snap.flag_bits(axis_y) & crate::state::flags::HAS_STATES != 0);
        // Column length matches the state count (fixed dense-column invariant).
        assert_eq!(snap.collision.len(), snap.state_count());
        assert_eq!(snap.mining_seconds.len(), snap.state_count());
        assert_eq!(snap.unbreakable.len(), snap.state_count());
        assert_eq!(snap.light_emission.len(), snap.state_count());
        assert_eq!(snap.light_dampening.len(), snap.state_count());
        assert_eq!(snap.loot.len(), snap.state_count());
        assert_eq!(snap.replaceable.len(), snap.state_count());
        assert_eq!(snap.liquid.len(), snap.state_count());
        assert_eq!(snap.random_tick.len(), snap.state_count());
        assert_eq!(snap.needs_support.len(), snap.state_count());
        assert_eq!(snap.can_contain_liquid.len(), snap.state_count());
        assert_eq!(snap.mining.len(), snap.state_count());
        assert_eq!(snap.drops.len(), snap.state_count());
        assert_eq!(snap.plugin_components.len(), snap.state_count());
        // None when mining/drops are undeclared (nothing guessed).
        assert!(snap.mining_of(axis_y).is_none());
        assert!(snap.drops_of(axis_y).is_none());
    }

    /// Unbreakable and mine seconds are mutually exclusive (rejected at parse time), with no combined bit at compile time.
    #[test]
    fn unbreakable_state_has_no_mining_seconds() {
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {"sc:unbreakable": {}},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let raw = (
            "blocks/minecraft/air.json".to_string(),
            json.as_bytes().to_vec(),
        );
        let snap = compile_bundle(
            &bundle_of(&[raw], "hashed"),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("应编译成功")
        .0;
        let air = snap
            .type_by_identifier("minecraft:air")
            .unwrap()
            .default_state;
        assert!(snap.is_unbreakable(air));
        assert_eq!(snap.mining_seconds_of(air), None);
        assert!(snap.flag_bits(air) & crate::state::flags::UNBREAKABLE != 0);
    }

    /// `sc:mining` inheritance, overlay, double-missing rejection, and conflicting-value warnings (decided per final state).
    #[test]
    fn mining_inheritance_override_and_warnings() {
        use sc_packloader::block::BlockBundleBudgets;
        fn bundle_with(mining_json: &str, destructible: Option<&str>) -> BlockJsonBundle {
            let mut comps = Vec::new();
            if let Some(d) = destructible {
                comps.push(format!("\"minecraft:destructible_by_mining\": {d}"));
            }
            comps.push(format!("\"sc:mining\": {mining_json}"));
            let json = format!(
                r#"{{"format_version": "1.10.0","minecraft:block": {{"description": {{"identifier": "minecraft:stone", "states": {{}}}},"components": {{{}}},"sc:protocol_runtime_ids": [7]}}}}"#,
                comps.join(",")
            );
            let air = raw_air();
            let stone = (
                "blocks/minecraft/stone.json".to_string(),
                json.as_bytes().to_vec(),
            );
            bundle_of(&[air, stone], "hashed")
        }
        // Explicit base wins.
        let (snap, warnings) = compile_bundle(
            &bundle_with(r#"{"formula_version": 1,"base_time_seconds": 3.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}"#, Some(r#"{"value": 3.0}"#)),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("显式 base 应编译");
        let stone = snap
            .type_by_identifier("minecraft:stone")
            .unwrap()
            .default_state;
        let m = snap.mining_of(stone).expect("mining profile");
        assert_eq!(m.base_seconds, 3.0);
        assert!(!m.base_from_destructible);
        assert!(warnings.is_empty());
        // Inherits destructible.
        let (snap, _) = compile_bundle(
            &bundle_with(
                r#"{"formula_version": 1,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}"#,
                Some(r#"{"value": 1.5}"#),
            ),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("继承应编译");
        let stone = snap
            .type_by_identifier("minecraft:stone")
            .unwrap()
            .default_state;
        let m = snap.mining_of(stone).expect("mining profile");
        assert_eq!(m.base_seconds, 1.5);
        assert!(m.base_from_destructible);
        // Double-missing rejected.
        let e = compile_bundle(
            &bundle_with(
                r#"{"formula_version": 1,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}"#,
                None,
            ),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect_err("双缺失必须拒绝");
        assert!(e.to_string().contains("继承源"), "实际：{e}");
        // Conflicting values warn (base wins, no rejection).
        let (snap, warnings) = compile_bundle(
            &bundle_with(r#"{"formula_version": 1,"base_time_seconds": 3.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}"#, Some(r#"{"value": 1.5}"#)),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("不一致应警告通过");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("不一致"), "实际：{:?}", warnings);
        let stone = snap
            .type_by_identifier("minecraft:stone")
            .unwrap()
            .default_state;
        assert_eq!(snap.mining_of(stone).unwrap().base_seconds, 3.0);
    }

    /// Permutations compile independent mining/drops per block state (never shared across states).
    #[test]
    fn permutation_compiles_per_state_mining_drops() {
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "minecraft:oak_log",
                    "states": {"pillar_axis": ["x", "y"]}
                },
                "components": {
                    "minecraft:destructible_by_mining": {"value": 2.0},
                    "sc:mining": {
                        "formula_version": 1,
                        "base_time_seconds": 2.0,
                        "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0},
                        "tools": [{"items": ["minecraft:iron_pickaxe"], "can_mine": true, "harvest": true, "speed_multiplier": 4.0}]
                    }
                },
                "permutations": [
                    {
                        "condition": "q.block_property('minecraft:oak_log','pillar_axis') == 'y'",
                        "components": {
                            "sc:mining": {
                                "formula_version": 1,
                                "base_time_seconds": 3.0,
                                "default": {"can_mine": false, "harvest": false, "speed_multiplier": 1.0}
                            },
                            "minecraft:loot": {"table": "loot_tables/blocks/oak_log_y.json"}
                        }
                    }
                ],
                "sc:protocol_runtime_ids": [100, 101]
            }
        }"#;
        let raw = (
            "blocks/minecraft/oak_log.json".to_string(),
            json.as_bytes().to_vec(),
        );
        // The permutation overrides `minecraft:loot` for axis y; the referenced
        // table content rides on the bundle.
        let mut tables = std::collections::BTreeMap::new();
        tables.insert(
            "loot_tables/blocks/oak_log_y.json".to_string(),
            serde_json::json!({
                "pools": [{"rolls": 1, "entries": [
                    {"type": "item", "name": "minecraft:coal", "count": 1}
                ]}]
            }),
        );
        let (snap, _) = compile_bundle(
            &bundle_of_with_tables(&[raw_air(), raw], "hashed", tables),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("permutation 应编译");
        let log = snap.type_by_identifier("minecraft:oak_log").unwrap();
        let find = |repr: &str| {
            log.state_ids
                .iter()
                .copied()
                .find(|i| snap.state(*i).unwrap().key.as_ref() == repr)
                .unwrap()
        };
        let axis_x = find("pillar_axis=x");
        let axis_y = find("pillar_axis=y");
        // x takes shared mining (mineable, iron pickaxe 4x), no loot (no drops).
        let mx = snap.mining_of(axis_x).expect("x mining");
        assert_eq!(mx.base_seconds, 2.0);
        assert!(mx.default_rule.can_mine);
        assert!(snap.drops_of(axis_x).is_none());
        // y takes overlay mining (unmineable) plus the overridden loot table (drops enabled).
        let my = snap.mining_of(axis_y).expect("y mining");
        assert_eq!(my.base_seconds, 3.0);
        assert!(!my.default_rule.can_mine);
        let dy = snap.drops_of(axis_y).expect("y drops");
        assert!(dy.enabled);
        assert_eq!(dy.entries.len(), 1);
        assert_eq!(&*dy.entries[0].item, "minecraft:coal");
    }

    /// Conflicting components are rejected per final state (compile time, on the merged map;
    /// the packloader load path is a direct load and does not pre-reject).
    #[test]
    fn conflicting_components_are_rejected_per_state() {
        // Single layer: mining plus unbreakable coexist.
        let bad = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {
                    "minecraft:destructible_by_mining": {"value": 1.0},
                    "sc:unbreakable": {},
                    "sc:mining": {"formula_version": 1,"base_time_seconds": 1.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}
                },
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        // Single layers are rejected at compile time (unbreakable+mining on the merged map).
        let raw = (
            "blocks/minecraft/stone.json".to_string(),
            bad.as_bytes().to_vec(),
        );
        let e = compile_bundle(
            &bundle_of(&[raw_air(), raw], "hashed"),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect_err("单层互斥必须拒绝");
        assert!(e.to_string().contains("互斥"), "实际：{e}");

        // Cross layer: shared unbreakable plus permutation mining (coexist after merge).
        let cross = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:oak_log", "states": {"pillar_axis": ["x", "y"]}},
                "components": {"sc:unbreakable": {}},
                "permutations": [
                    {"condition": "q.block_property('minecraft:oak_log','pillar_axis') == 'x'",
                     "components": {"sc:mining": {"formula_version": 1,"base_time_seconds": 1.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}}}
                ],
                "sc:protocol_runtime_ids": [100, 101]
            }
        }"#;
        let raw = (
            "blocks/minecraft/oak_log.json".to_string(),
            cross.as_bytes().to_vec(),
        );
        let e = compile_bundle(
            &bundle_of(&[raw_air(), raw], "hashed"),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect_err("跨层互斥必须整包拒绝");
        assert!(e.to_string().contains("互斥"), "实际：{e}");
    }

    /// Illegal tool/drop references and budgets are rejected at compile time (including missing items).
    #[test]
    fn bad_refs_and_budgets_are_rejected() {
        // Unknown item (mining tool).
        let unknown_item = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {
                    "sc:mining": {"formula_version": 1,"base_time_seconds": 1.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0},"tools": [{"items": ["minecraft:nope_not_an_item"],"can_mine": true,"harvest": true,"speed_multiplier": 2.0}]}
                },
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let raw = (
            "blocks/minecraft/stone.json".to_string(),
            unknown_item.as_bytes().to_vec(),
        );
        let e = compile_bundle(
            &bundle_of(&[raw_air(), raw], "hashed"),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            &|id: &str| id != "minecraft:nope_not_an_item",
        )
        .expect_err("未知物品必须拒绝");
        assert!(
            e.to_string().contains("不在当前版本包物品注册表"),
            "实际：{e}"
        );

        // Unknown item (loot-table drop entry; the table rides on the bundle).
        let loot_unknown = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {"minecraft:loot": "loot_tables/blocks/stone.json"},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let raw = (
            "blocks/minecraft/stone.json".to_string(),
            loot_unknown.as_bytes().to_vec(),
        );
        let mut tables = std::collections::BTreeMap::new();
        tables.insert(
            "loot_tables/blocks/stone.json".to_string(),
            serde_json::json!({
                "pools": [{"rolls": 1, "entries": [
                    {"type": "item", "name": "minecraft:nope_not_an_item", "count": 1}
                ]}]
            }),
        );
        let e = compile_bundle(
            &bundle_of_with_tables(&[raw_air(), raw], "hashed", tables),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            &|id: &str| id != "minecraft:nope_not_an_item",
        )
        .expect_err("战利品表未知物品必须拒绝");
        assert!(
            e.to_string().contains("不在当前版本包物品注册表"),
            "实际：{e}"
        );

        // Budget saturated: too many tools (compile-time check).
        let mut tight = BlockBundleBudgets::default();
        tight.max_mining_tools_per_state = 0;
        let many_tools = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {
                    "sc:mining": {"formula_version": 1,"base_time_seconds": 1.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0},"tools": [{"items": ["minecraft:stone"],"can_mine": true,"harvest": true,"speed_multiplier": 2.0}]}
                },
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let raw = (
            "blocks/minecraft/stone.json".to_string(),
            many_tools.as_bytes().to_vec(),
        );
        // Rejected at compile time for budget (tool count 1 > 0).
        let e = compile_bundle(
            &bundle_of(&[raw_air(), raw], "hashed"),
            PACK,
            &tight,
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect_err("预算超限必须拒绝");
        assert!(e.to_string().contains("超出预算"), "实际：{e}");
    }

    /// Failed compiles publish no partial snapshot (atomic publish; the old snapshot stays readable).
    #[test]
    fn failed_compile_publishes_nothing() {
        use std::sync::Arc;
        let good = bundle_of(&[raw_air(), raw_log()], "hashed");
        let (good_snap, _) = compile_bundle(
            &good,
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("好 bundle 应编译");
        let registry = BlockJsonRegistry::new();
        registry.publish(Arc::new(good_snap));
        let before = registry.get().expect("应有旧快照");
        assert_eq!(before.type_count(), 2);

        // Bad bundle (duplicate tools) fails to compile and must not publish.
        let bad_json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {
                    "sc:mining": {"formula_version": 1,"base_time_seconds": 1.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0},"tools": [
                        {"items": ["minecraft:stone"],"can_mine": true,"harvest": true,"speed_multiplier": 2.0},
                        {"items": ["minecraft:stone"],"can_mine": true,"harvest": true,"speed_multiplier": 3.0}
                    ]}
                },
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let bad_raw = (
            "blocks/minecraft/stone.json".to_string(),
            bad_json.as_bytes().to_vec(),
        );
        // Rejected at compile time (duplicate item), so no snapshot is published.
        let e = compile_bundle(
            &bundle_of(&[raw_air(), bad_raw], "hashed"),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect_err("重复工具必须编译失败");
        assert!(e.to_string().contains("重复出现"), "实际：{e}");
        // Registry still holds the old snapshot (never partially modified).
        let after = registry.get().expect("旧快照仍可读");
        assert_eq!(after.type_count(), 2);
        assert_eq!(after.fingerprint, before.fingerprint);
    }

    /// Fingerprint tracks mining/drops content (rule identity belongs to version identity).
    #[test]
    fn fingerprint_changes_with_mining_drops() {
        use sc_packloader::block::fingerprint_bundle;
        let a = r#"{"format_version": "1.10.0","minecraft:block": {"description": {"identifier": "minecraft:stone", "states": {}},"components": {"sc:mining": {"formula_version": 1,"base_time_seconds": 1.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}},"sc:protocol_runtime_ids": [7]}}"#;
        let b = r#"{"format_version": "1.10.0","minecraft:block": {"description": {"identifier": "minecraft:stone", "states": {}},"components": {"sc:mining": {"formula_version": 1,"base_time_seconds": 2.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}},"sc:protocol_runtime_ids": [7]}}"#;
        let air = raw_air();
        let refs_a: Vec<(&str, &[u8])> = vec![
            (air.0.as_str(), air.1.as_slice()),
            (
                "blocks/minecraft/stone.json",
                a.as_bytes(),
            ),
        ];
        let refs_b: Vec<(&str, &[u8])> = vec![
            (air.0.as_str(), air.1.as_slice()),
            (
                "blocks/minecraft/stone.json",
                b.as_bytes(),
            ),
        ];
        assert_ne!(
            fingerprint_bundle(&refs_a, "hashed"),
            fingerprint_bundle(&refs_b, "hashed")
        );
    }

    /// Example files load (`examples/blocks/example_compressed_ore.block.json`).
    #[test]
    fn example_compressed_ore_loads() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/blocks/example_compressed_ore.block.json"
        ))
        .expect("示例文件缺失");
        // JSON syntax valid (all examples syntax-checked).
        let _: serde_json::Value = serde_json::from_slice(&bytes).expect("示例 JSON 语法非法");
        let air = raw_air();
        let ore = (
            "blocks/example/compressed_ore.json".to_string(),
            bytes,
        );
        // Note: the example identifier is example:compressed_ore and the file path
        // example/compressed_ore (single path segment) matches it.
        let bundle = bundle_of_with_tables(
            &[air, ore],
            "hashed",
            std::iter::once((
                "loot_tables/blocks/example_compressed_ore.json".to_string(),
                serde_json::from_slice(
                    &std::fs::read(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../examples/blocks/loot_tables/example_compressed_ore.json"
                    ))
                    .expect("示例战利品表缺失"),
                )
                .expect("示例战利品表 JSON 非法"),
            ))
            .collect(),
        );
        let (snap, warnings) = compile_bundle(
            &bundle,
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            &|id: &str| {
                matches!(
                    id,
                    "minecraft:wooden_pickaxe"
                        | "minecraft:iron_pickaxe"
                        | "minecraft:diamond_pickaxe"
                        | "minecraft:cobblestone"
                        | "minecraft:coal"
                )
            },
        )
        .expect("示例应编译");
        assert!(warnings.is_empty());
        let ore_idx = snap
            .type_by_identifier("example:compressed_ore")
            .expect("ore 类型")
            .default_state;
        let m = snap.mining_of(ore_idx).expect("mining");
        assert_eq!(m.base_seconds, 3.0);
        // Drops compile from the referenced table (two independent pools).
        assert_eq!(
            snap.loot_of(ore_idx),
            Some("loot_tables/blocks/example_compressed_ore.json")
        );
        let d = snap.drops_of(ore_idx).expect("drops");
        assert!(d.enabled);
        assert_eq!(d.entries.len(), 2);
        assert_eq!(&*d.entries[0].item, "minecraft:cobblestone");
        assert_eq!(d.entries[0].chance, 1.0);
        assert_eq!(&*d.entries[1].item, "minecraft:coal");
        assert_eq!(d.entries[1].chance, 0.25);
    }

    /// Weighted one_of pools and empty alternatives compile per the extraction-pipeline
    /// table shape (gilded_blackstone / gravel / mangrove_propagule semantics).
    #[test]
    fn loot_table_one_of_weight_and_empty_alternatives() {
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "test:weighted", "states": {}},
                "components": {"minecraft:loot": {"table": "loot_tables/blocks/weighted.json"}},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let raw = (
            "blocks/test/weighted.json".to_string(),
            json.as_bytes().to_vec(),
        );
        let mut tables = std::collections::BTreeMap::new();
        tables.insert(
            "loot_tables/blocks/weighted.json".to_string(),
            serde_json::json!({
                "pools": [
                    // 90% self, 10% gold_nugget with a count distribution (weighted one_of).
                    {"rolls": 1, "entries": [
                        {"type": "item", "name": "minecraft:gilded_blackstone", "weight": 900000, "count": 1},
                        {"type": "item", "name": "minecraft:gold_nugget", "weight": 100000, "count": [
                            {"value": 2, "chance": 0.25}, {"value": 3, "chance": 0.25},
                            {"value": 4, "chance": 0.25}, {"value": 5, "chance": 0.25}
                        ]}
                    ]}
                ]
            }),
        );
        let snap = compile_bundle(
            &bundle_of_with_tables(&[raw_air(), raw], "hashed", tables),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("加权表应编译")
        .0;
        let idx = snap
            .type_by_identifier("test:weighted")
            .unwrap()
            .default_state;
        let d = snap.drops_of(idx).expect("加权 drops");
        assert_eq!(d.mode, crate::mining_drops::DropMode::OneOf);
        assert_eq!(d.entries.len(), 2);
        // Selection chances normalize over the pool weight total.
        assert_eq!(d.entries[0].chance, 0.9);
        assert_eq!(d.entries[1].chance, 0.1);
        assert_eq!(&*d.entries[0].item, "minecraft:gilded_blackstone");
        assert_eq!(&*d.entries[1].item, "minecraft:gold_nugget");

        // 50/50 empty alternative: the empty branch counts toward the weight total.
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "test:empty_alt", "states": {}},
                "components": {"minecraft:loot": "loot_tables/blocks/empty_alt.json"},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let raw = (
            "blocks/test/empty_alt.json".to_string(),
            json.as_bytes().to_vec(),
        );
        let mut tables = std::collections::BTreeMap::new();
        tables.insert(
            "loot_tables/blocks/empty_alt.json".to_string(),
            serde_json::json!({
                "pools": [{"rolls": 1, "entries": [
                    {"type": "item", "name": "minecraft:flint", "weight": 1, "count": 1},
                    {"type": "empty", "weight": 1}
                ]}]
            }),
        );
        let snap = compile_bundle(
            &bundle_of_with_tables(&[raw_air(), raw], "hashed", tables),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("50/50 empty 表应编译")
        .0;
        let idx = snap
            .type_by_identifier("test:empty_alt")
            .unwrap()
            .default_state;
        let d = snap.drops_of(idx).expect("50/50 drops");
        assert_eq!(d.mode, crate::mining_drops::DropMode::OneOf);
        assert_eq!(d.entries.len(), 1);
        assert_eq!(d.entries[0].chance, 0.5);
        assert_eq!(&*d.entries[0].item, "minecraft:flint");

        // A pure-empty pool compiles to no drops at all.
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "test:empty_pool", "states": {}},
                "components": {"minecraft:loot": "loot_tables/blocks/empty.json"},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let raw = (
            "blocks/test/empty_pool.json".to_string(),
            json.as_bytes().to_vec(),
        );
        let mut tables = std::collections::BTreeMap::new();
        tables.insert(
            "loot_tables/blocks/empty.json".to_string(),
            serde_json::json!({
                "pools": [{"rolls": 1, "entries": [{"type": "empty", "weight": 1}]}]
            }),
        );
        let snap = compile_bundle(
            &bundle_of_with_tables(&[raw_air(), raw], "hashed", tables),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect("纯 empty 表应编译")
        .0;
        let idx = snap
            .type_by_identifier("test:empty_pool")
            .unwrap()
            .default_state;
        assert!(snap.drops_of(idx).is_none());
    }

    /// `ur:*` keys load raw (direct load; namespace semantics belong to consumers,
    /// unknown components flow into the plugin bucket at compile time).
    #[test]
    fn ur_keys_load_raw_without_interpretation() {
        // Component-level ur:* loads raw.
        let legacy = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {"ur:replaceable": true},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let parsed = sc_packloader::block::parse_block_file(
            PACK,
            "blocks/minecraft/air.json",
            legacy.as_bytes(),
        )
        .expect("直接加载不拒绝 ur:*");
        assert_eq!(
            parsed.base.get("ur:replaceable"),
            Some(&serde_json::json!(true))
        );
        // Top-level ur:* is ignored by envelope extraction (data still loads).
        let top_legacy = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "ur:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let parsed = sc_packloader::block::parse_block_file(
            PACK,
            "blocks/minecraft/air.json",
            top_legacy.as_bytes(),
        )
        .expect("顶层未知字段直接加载");
        assert_eq!(parsed.identifier, "minecraft:air");
        // Unregistered components stay out of the known set (compile routes them to plugins).
        assert!(!sc_packloader::block::BlockComponents::is_known("ur:replaceable"));
    }

    /// Overlapping permutation overlays fail the whole pack (array-order independent).
    #[test]
    fn overlapping_permutation_overrides_are_rejected() {
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "permutations": [
                    {"condition": "q.block_property('minecraft:air','x') == 0",
                     "components": {"sc:random_tick": true}},
                    {"condition": "q.block_property('minecraft:air','x') == 0",
                     "components": {"sc:random_tick": false}}
                ],
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        // air has no properties, so conditions referencing undeclared properties are rejected at parse time.
        let e = parse_block_file(
            PACK,
            "blocks/minecraft/air.json",
            json.as_bytes(),
        )
        .expect_err("引用未声明属性必须拒绝");
        assert!(
            e.to_string().contains("未声明的属性"),
            "实际：{e}"
        );

        // True overlap: two conditions hit the same state and overlay the same component.
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "minecraft:oak_log",
                    "states": {"pillar_axis": ["x", "y"]}
                },
                "components": {},
                "permutations": [
                    {"condition": "q.block_property('minecraft:oak_log','pillar_axis') == 'x'",
                     "components": {"sc:random_tick": true}},
                    {"condition": "q.block_property('minecraft:oak_log','pillar_axis') == 'x'",
                     "components": {"sc:random_tick": false}}
                ],
                "sc:protocol_runtime_ids": [100, 101]
            }
        }"#;
        let raw = (
            "blocks/minecraft/oak_log.json".to_string(),
            json.as_bytes().to_vec(),
        );
        let e = compile_bundle(
            &bundle_of(&[raw_air(), raw], "hashed"),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect_err("重叠覆盖必须整包失败");
        assert!(e.to_string().contains("已被规则"), "实际：{e}");
    }

    /// Conflicting protocol runtime ids with differing canonical states fail the whole pack.
    #[test]
    fn duplicate_network_id_with_different_state_is_rejected() {
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:oak_log", "states": {}},
                "components": {},
                "sc:protocol_runtime_ids": [7, 7]
            }
        }"#;
        // A single-state block cannot carry two runtime ids (length mismatch rejected at parse time).
        let e = parse_block_file(
            PACK,
            "blocks/minecraft/oak_log.json",
            json.as_bytes(),
        )
        .expect_err("长度不符必须拒绝");
        assert!(e.to_string().contains("一一对应"), "实际：{e}");

        // Cross-type conflict: two types with one state each sharing a runtime id.
        let a = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:oak_log", "states": {}},
                "components": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let b = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:birch_log", "states": {}},
                "components": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let e = compile_bundle(
            &bundle_of(
                &[
                    raw_air(),
                    (
                        "blocks/minecraft/oak_log.json".to_string(),
                        a.as_bytes().to_vec(),
                    ),
                    (
                        "blocks/minecraft/birch_log.json".to_string(),
                        b.as_bytes().to_vec(),
                    ),
                ],
                "hashed",
            ),
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect_err("跨类型 runtime id 冲突必须拒绝");
        assert!(e.to_string().contains("重复"), "实际：{e}");
    }

    /// Missing minecraft:air fails the whole pack (never silently adds air).
    #[test]
    fn missing_air_is_rejected() {
        let bundle = bundle_of(&[raw_log()], "hashed");
        let e = compile_bundle(
            &bundle,
            PACK,
            &BlockBundleBudgets::default(),
            NO_PREEXISTING,
            ALLOW_ALL_ITEMS,
        )
        .expect_err("缺 air 必须拒绝");
        assert!(e.to_string().contains("缺少 minecraft:air"), "实际：{e}");
    }

}
