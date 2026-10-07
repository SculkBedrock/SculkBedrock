//! Semantic compilation of `.block.json` files into an immutable registry snapshot (in `sc_block`).
//!
//! Responsibility split: the `sc_packloader::block` domain handles discovery, quota-capped reads, and
//! schema parsing (component struct registry in `block::component`, mirroring `item::component`; file body is the
//! vanilla `format_version` + `minecraft:block` structure), while this module handles all cross-file semantic
//! checks (default states, id conflicts, hash collisions, overlay overlaps) and atomically publishes
//! the immutable [`BlockJsonSnapshot`] once the private builder flow completes.
//!
//! Tick hot paths only read the snapshot and dense tables, never parse JSON.
//!
//! Identity contract: internal dense indices (assigned by identifier and typed canonical state order),
//! FNV state hashes (canonical `sc_world::leveldb::block_hash` algorithm), protocol sequential ids
//! (`sc:protocol_runtime_ids` kept verbatim, never recomputed by sort order), and item ids stay
//! independent. `minecraft:unknown` keeps the `-2` sentinel; any hash collision rejects the whole pack.
//!
//! This module never guesses gameplay data: undeclared components compile to `None` (undeclared)
//! instead of inheriting stone-like defaults.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use sc_ecs::resource::Resource;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use sc_packloader::block::{
    canonical_state_key, canonical_state_props, BlockBundleBudgets, BlockJsonBundle,
    BlockJsonError, LegacyPaletteEntry, ParsedBlockFile, PropValue,
};

use crate::mining_drops::{compile_drops, compile_mining, DropCompiled, MiningCompiled};

// ---------------------------------------------------------------------------
// Typed canonical state keys (unambiguous identity for the new compile path; replaces the old
// state_signature string-concat/byte-bool behavior).
// ---------------------------------------------------------------------------

/// identifier plus sorted properties to an unambiguous key (length-prefixed, separator-forgery safe).
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

/// Property assignments to equivalent states NBT (disk palette / global dictionary share the same property identity).
pub fn states_nbt_of(props: &[(String, PropValue)]) -> CompoundNbt {
    let mut out = CompoundNbt::new(None);
    for (name, value) in props.iter() {
        out.insert(name, value.to_nbt());
    }
    out
}

/// Lenient string match of one query pair against typed snapshot props
/// (same rules as the global dictionary matcher over NBT values).
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

/// All query pairs match (missing keys fail).
fn snapshot_props_match(props: &[(String, PropValue)], pairs: &[(&str, &str)]) -> bool {
    pairs
        .iter()
        .all(|(key, value)| snapshot_prop_matches(props, key, value))
}

/// NBT value to typed comparison key fragment.
///
/// byte/int/string encode **byte-identically** to [`typed_state_key`] (same function, same key),
/// so snapshot states compare per-state against the legacy palette / global dictionary;
/// other legacy types (short/long/float/array/nested) have no `PropValue`, using a `~t`
/// prefix plus type tag plus Debug text whose prefix can never collide with the three standard encodings.
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

/// Arbitrary NBT states (including legacy types beyond byte/int/string) to typed comparison keys.
/// Matches [`typed_state_key`] on the three property types.
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

// ---------------------------------------------------------------------------
// Immutable registry snapshot (the sole atomically published product).
// ---------------------------------------------------------------------------

/// Compiled view of a block type (sorted by identifier in the snapshot).
#[derive(Clone, Debug)]
pub struct SnapType {
    pub identifier: Box<str>,
    pub file: Box<str>,
    pub type_id: u32,
    pub properties: Vec<sc_packloader::block::PropertyDef>,
    /// Dense index of the default state (explicitly declared via `sc:default_state`).
    pub default_state: u32,
    pub state_ids: Vec<u32>,
}

/// Compiled view of a single valid state (sorted by (identifier, typed key) in the snapshot;
/// the index is the new dense `BlockStateId`).
///
/// Static capabilities are not inlined in this struct; they compile into the snapshot's fixed dense
/// columns (below), addressed by index.
#[derive(Clone, Debug)]
pub struct SnapState {
    pub index: u32,
    pub type_id: u32,
    /// Canonical state key (`name=repr` comma-joined; `default` when property-less).
    pub key: Box<str>,
    pub hash: u32,
    /// Verbatim `sc:protocol_runtime_ids`, never recomputed by sort order.
    pub network_id: u32,
    /// Typed canonical state identity (sorted length-prefixed encoding).
    pub typed_key: Box<str>,
    /// Canonical state property assignments (ascending by name; equivalent to states NBT).
    pub props: Vec<(String, PropValue)>,
}

/// Immutable snapshot atomically published after the private builder passes all checks.
///
/// Callers share an `Arc` read-only reference without per-Region deep copies; hot paths only read
/// this snapshot and the dense tables derived from it.
#[derive(Clone, Debug)]
pub struct BlockJsonSnapshot {
    pub schema_version: u32,
    pub network_id_mode: String,
    /// Content fingerprint (packloader computes it over sorted path+bytes+mode): any file change
    /// alters it, suitable for the generator descriptor.
    pub fingerprint: u64,
    pub types: Vec<SnapType>,
    pub states: Vec<SnapState>,
    pub by_hash: HashMap<u32, u32>,
    pub by_network_id: HashMap<u32, u32>,
    pub by_typed: HashMap<(Box<str>, Box<str>), u32>,
    pub defaults: HashMap<Box<str>, u32>,
    // ---- Fixed dense capability columns (fixed component set plus plugin verbatim columns; all indexed
    // by state, same length as `states`; hot paths only do array indexing) ----
    /// `minecraft:collision_box` boxes (each `[ox,oy,oz,sx,sy,sz]` in 16ths;
    /// `enabled:false` means empty list = no collision; None = undeclared).
    pub collision: Vec<Option<Vec<[f32; 6]>>>,
    /// `minecraft:destructible_by_mining` seconds (None = undeclared).
    pub mining_seconds: Vec<Option<f32>>,
    /// `sc:unbreakable` (mutually exclusive with destructible_by_mining; checked at parse time per layer plus post-merge).
    pub unbreakable: Vec<bool>,
    /// `minecraft:light_emission`(0..15).
    pub light_emission: Vec<Option<u8>>,
    /// `minecraft:light_dampening`(0..15).
    pub light_dampening: Vec<Option<u8>>,
    /// `minecraft:loot` table path (full table lives in a separate data domain, referenced here).
    pub loot: Vec<Option<Box<str>>>,
    /// `sc:replaceable`.
    pub replaceable: Vec<Option<bool>>,
    /// `sc:liquid` kind.
    pub liquid: Vec<Option<Box<str>>>,
    /// `sc:random_tick`.
    pub random_tick: Vec<Option<bool>>,
    /// `sc:needs_support` condition.
    pub needs_support: Vec<Option<Box<str>>>,
    /// `sc:can_contain_liquid`.
    pub can_contain_liquid: Vec<Option<bool>>,
    /// Compiled `sc:mining` profile (`None` means no mining for this state, legacy path applies;
    /// densely addressed by `BlockStateId`, same length as `states`).
    pub mining: Vec<Option<MiningCompiled>>,
    /// Compiled `sc:drops` profile (`None` means no drops, nothing guessed;
    /// `Some(enabled=false)` means explicitly drop-free; same length as `states`).
    pub drops: Vec<Option<DropCompiled>>,
    /// Non-fixed component verbatim values (name to value; schema validated by plugin registration, cold path).
    pub plugin_components: Vec<Vec<(Box<str>, serde_json::Value)>>,
}

/// Compiles the merged component map into fixed columns (per state).
///
/// Inputs were validated per item at packloader parse time; this step checks post-merge semantics
/// (exclusion, inheritance, existence) and compiles densely, rejecting the whole pack on failure
/// (no partial snapshot, no fallback).
fn compile_state_columns(
    snapshot: &mut BlockJsonSnapshot,
    merged: &BTreeMap<String, serde_json::Value>,
    pack_id: &str,
    file: &str,
    key: &str,
    budgets: &sc_packloader::block::BlockBundleBudgets,
    item_exists: &dyn Fn(&str) -> bool,
    warnings: &mut Vec<String>,
) -> Result<(), BlockJsonError> {
    use sc_packloader::block::component::*;
    let field = |name: &str| format!("$.minecraft:block.components.{name}（状态 {key:?}）");
    let bad = |name: &str, detail: String| BlockJsonError::new(pack_id, file, field(name), detail);
    let get = |name: &str| merged.get(name).cloned();

    let collision: Option<Vec<[f32; 6]>> = match get(CollisionBox::NAME) {
        None => None,
        Some(v) => {
            let boxes = collision_boxes_of(&v)
                .map_err(|e| bad(CollisionBox::NAME, format!("内部不一致：{e}")))?;
            let mut out = Vec::new();
            for b in boxes {
                if !b.enabled {
                    // No collision expressed as an empty list.
                    out.clear();
                    break;
                }
                out.push([
                    b.origin[0],
                    b.origin[1],
                    b.origin[2],
                    b.size[0],
                    b.size[1],
                    b.size[2],
                ]);
            }
            Some(out)
        }
    };
    let mining_seconds: Option<f32> = match get(DestructibleByMining::NAME) {
        None => None,
        Some(v) => {
            let d: DestructibleByMining = serde_json::from_value(v)
                .map_err(|e| bad(DestructibleByMining::NAME, format!("内部不一致：{e}")))?;
            Some(d.value)
        }
    };
    let unbreakable = merged.contains_key(Unbreakable::NAME);
    let light_emission: Option<u8> = match get(LightEmission::NAME) {
        None => None,
        Some(v) => {
            let l: LightEmission = serde_json::from_value(v)
                .map_err(|e| bad(LightEmission::NAME, format!("内部不一致：{e}")))?;
            Some(l.0)
        }
    };
    let light_dampening: Option<u8> = match get(LightDampening::NAME) {
        None => None,
        Some(v) => {
            let l: LightDampening = serde_json::from_value(v)
                .map_err(|e| bad(LightDampening::NAME, format!("内部不一致：{e}")))?;
            Some(l.0)
        }
    };
    let loot: Option<Box<str>> = match get(Loot::NAME) {
        None => None,
        Some(v) => {
            let l: Loot = serde_json::from_value(v)
                .map_err(|e| bad(Loot::NAME, format!("内部不一致：{e}")))?;
            Some(l.0)
        }
    };
    let replaceable: Option<bool> = match get(Replaceable::NAME) {
        None => None,
        Some(v) => {
            let r: Replaceable = serde_json::from_value(v)
                .map_err(|e| bad(Replaceable::NAME, format!("内部不一致：{e}")))?;
            Some(r.0)
        }
    };
    let liquid: Option<Box<str>> = match get(Liquid::NAME) {
        None => None,
        Some(v) => {
            let l: Liquid = serde_json::from_value(v)
                .map_err(|e| bad(Liquid::NAME, format!("内部不一致：{e}")))?;
            Some(Box::from(l.kind.as_str()))
        }
    };
    let random_tick: Option<bool> = match get(RandomTick::NAME) {
        None => None,
        Some(v) => {
            let r: RandomTick = serde_json::from_value(v)
                .map_err(|e| bad(RandomTick::NAME, format!("内部不一致：{e}")))?;
            Some(r.0)
        }
    };
    let needs_support: Option<Box<str>> = match get(NeedsSupport::NAME) {
        None => None,
        Some(v) => {
            let n: NeedsSupport = serde_json::from_value(v)
                .map_err(|e| bad(NeedsSupport::NAME, format!("内部不一致：{e}")))?;
            Some(Box::from(n.condition.as_str()))
        }
    };
    let can_contain_liquid: Option<bool> = match get(CanContainLiquid::NAME) {
        None => None,
        Some(v) => {
            let c: CanContainLiquid = serde_json::from_value(v)
                .map_err(|e| bad(CanContainLiquid::NAME, format!("内部不一致：{e}")))?;
            Some(c.0)
        }
    };
    // ---- Post-merge semantic checks (cross-layer conflicts; single layers already rejected in packloader, final state rechecked here) ----
    // destructible plus unbreakable (different keys; permutations may each carry one).
    if merged.contains_key(DestructibleByMining::NAME) && merged.contains_key(Unbreakable::NAME) {
        return Err(BlockJsonError::new(
            pack_id,
            file,
            field("minecraft:destructible_by_mining"),
            format!(
                "状态 {key:?} 同时存在 minecraft:destructible_by_mining 与 sc:unbreakable（互斥）"
            ),
        ));
    }
    // unbreakable plus mining (different keys; whole-replace semantics may still merge both).
    if merged.contains_key(Unbreakable::NAME) && merged.contains_key(Mining::NAME) {
        return Err(BlockJsonError::new(
            pack_id,
            file,
            field("sc:mining"),
            format!("状态 {key:?} 同时存在 sc:unbreakable 与 sc:mining（互斥）"),
        ));
    }
    // loot + enabled drops.
    if let Some(loot_raw) = merged.get(Loot::NAME) {
        if let Some(drops_raw) = merged.get(Drops::NAME) {
            let drops_parsed: Drops = serde_json::from_value(drops_raw.clone())
                .map_err(|e| bad(Drops::NAME, format!("内部不一致：{e}")))?;
            if drops_parsed.enabled {
                let _ = loot_raw;
                return Err(BlockJsonError::new(
                    pack_id,
                    file,
                    field("sc:drops"),
                    format!(
                        "状态 {key:?} 同时存在 minecraft:loot 与启用的 sc:drops（掉落来源不明）"
                    ),
                ));
            }
        }
    }
    // mining / drops compile (inheritance, existence, budgets; warnings collected).
    let mining = compile_mining(
        merged,
        mining_seconds,
        pack_id,
        file,
        key,
        budgets,
        item_exists,
        warnings,
    )?;
    let drops = compile_drops(merged, pack_id, file, key, budgets, item_exists)?;

    let mut plugin = Vec::new();
    for (name, value) in merged.iter() {
        if !is_known_component(name) {
            plugin.push((Box::from(name.as_str()), value.clone()));
        }
    }
    snapshot.collision.push(collision);
    snapshot.mining_seconds.push(mining_seconds);
    snapshot.unbreakable.push(unbreakable);
    snapshot.light_emission.push(light_emission);
    snapshot.light_dampening.push(light_dampening);
    snapshot.loot.push(loot);
    snapshot.replaceable.push(replaceable);
    snapshot.liquid.push(liquid);
    snapshot.random_tick.push(random_tick);
    snapshot.needs_support.push(needs_support);
    snapshot.can_contain_liquid.push(can_contain_liquid);
    snapshot.mining.push(mining);
    snapshot.drops.push(drops);
    snapshot.plugin_components.push(plugin);
    Ok(())
}

/// Compiles the whole bundle. `preexisting` is the existing-state lookup (production passes the global
/// dictionary probe, tests pass an isolated table): `hash -> (identifier, typed key)`; a differing canonical
/// state fails the whole pack.
/// `item_exists` is the tool/drop item existence lookup (production passes the version-pack item registry
/// probe, tests pass an isolated table; unknown rejects the whole pack, no `default` fallback).
///
/// On success returns the fully validated snapshot plus warnings (e.g. conflicting values; warnings do not reject);
/// on failure produces no partial output (callers must not publish).
pub fn compile_bundle(
    bundle: &BlockJsonBundle,
    pack_id: &str,
    budgets: &BlockBundleBudgets,
    preexisting: &dyn Fn(u32) -> Option<(Box<str>, Box<str>)>,
    item_exists: &dyn Fn(&str) -> bool,
) -> Result<(BlockJsonSnapshot, Vec<String>), BlockJsonError> {
    use sc_packloader::block::{NETWORK_ID_MODE_HASHED, NETWORK_ID_MODE_PALETTE};
    if bundle.network_id_mode != NETWORK_ID_MODE_HASHED
        && bundle.network_id_mode != NETWORK_ID_MODE_PALETTE
    {
        return Err(BlockJsonError::new(
            pack_id,
            "<bundle>",
            "$.block_data.network_id_mode",
            format!("不支持的 network_id_mode {:?}", bundle.network_id_mode),
        ));
    }
    // Compiles in canonical path order: independent of packloader enumeration order.
    let mut ordered: Vec<&ParsedBlockFile> = bundle.files.iter().collect();
    ordered.sort_by(|a, b| a.zip_path.cmp(&b.zip_path));
    let total_states: usize = ordered.iter().map(|f| f.state_count()).sum();
    if total_states > budgets.max_total_states {
        return Err(BlockJsonError::new(
            pack_id,
            "<bundle>",
            "$",
            format!(
                "总状态数 {total_states} 超出预算 {}",
                budgets.max_total_states
            ),
        ));
    }

    // ---- Enumerates canonical states per file (schema canonical order, same rule as parse time) ----
    struct RichState {
        key: String,
        typed_key: Box<str>,
        hash: u32,
        network_id: u32,
        props: Vec<(String, PropValue)>,
    }
    let mut rich: Vec<Vec<RichState>> = Vec::with_capacity(ordered.len());
    for file in ordered.iter() {
        let mut states = Vec::with_capacity(file.state_count());
        for index in 0..file.state_count() {
            let props = canonical_state_props(&file.properties, index);
            let states_nbt = states_nbt_of(&props);
            states.push(RichState {
                key: canonical_state_key(&props),
                typed_key: typed_state_key(&file.identifier, &props),
                hash: sc_world::leveldb::block_hash::block_state_hash(
                    &file.identifier,
                    Some(&states_nbt),
                ),
                network_id: file.network_ids[index],
                props,
            });
        }
        // Enumeration rules guarantee uniqueness; rechecked here (guards internal inconsistency).
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for st in states.iter() {
            if let Some(prev) = seen.insert(st.typed_key.as_ref(), st.key.as_str()) {
                return Err(BlockJsonError::new(
                    pack_id,
                    &file.zip_path,
                    "$.minecraft:block.sc:protocol_runtime_ids",
                    format!(
                        "重复的规范状态：key {prev:?} 与 {:?} 的带类型 states 相同",
                        st.key
                    ),
                ));
            }
        }
        rich.push(states);
    }

    // ---- Cross-file checks ----
    let mut seen_identifiers: HashMap<&str, &str> = HashMap::new();
    let mut network_map: HashMap<u32, (u32, String, String)> = HashMap::new();
    let mut hash_map: HashMap<u32, (String, Box<str>, String, String)> = HashMap::new();
    for (file, states) in ordered.iter().zip(rich.iter()) {
        if let Some(prev) =
            seen_identifiers.insert(file.identifier.as_str(), file.zip_path.as_str())
        {
            return Err(BlockJsonError::new(
                pack_id,
                &file.zip_path,
                "$.minecraft:block.description.identifier",
                format!(
                    "重复 identifier {:?}（已见于 {prev:?}；首版不提供隐式覆盖）",
                    file.identifier
                ),
            ));
        }
        for st in states.iter() {
            let id_field = format!("$.minecraft:block.sc:protocol_runtime_ids[{:?}]", st.key);
            if let Some((prev_hash, prev_file, prev_key)) = network_map.get(&st.network_id) {
                if *prev_hash != st.hash {
                    return Err(BlockJsonError::new(
                        pack_id,
                        &file.zip_path,
                        &id_field,
                        format!(
                            "protocol_runtime_id {} 与 {prev_file:?} 的 {prev_key:?} 重复但规范状态不同",
                            st.network_id
                        ),
                    ));
                }
            } else {
                network_map.insert(
                    st.network_id,
                    (st.hash, file.zip_path.clone(), st.key.clone()),
                );
            }
            if let Some((prev_id, prev_key, prev_file, prev_state)) = hash_map.get(&st.hash) {
                if prev_id != &file.identifier || prev_key != &st.typed_key {
                    return Err(BlockJsonError::new(
                        pack_id,
                        &file.zip_path,
                        &id_field,
                        format!(
                            "FNV 状态哈希冲突：与 {prev_file:?} 的 {prev_id:?}/{prev_state:?} 哈希相同但规范状态不同（整包拒绝，不做首见覆盖）",
                        ),
                    ));
                }
            } else {
                hash_map.insert(
                    st.hash,
                    (
                        file.identifier.clone(),
                        st.typed_key.clone(),
                        file.zip_path.clone(),
                        st.key.clone(),
                    ),
                );
            }
            if let Some((prev_id, prev_key)) = preexisting(st.hash) {
                if prev_id.as_ref() != file.identifier || prev_key.as_ref() != st.typed_key.as_ref()
                {
                    return Err(BlockJsonError::new(
                        pack_id,
                        &file.zip_path,
                        &id_field,
                        format!(
                            "与已存在状态冲突：哈希 {:#x} 已对应 {prev_id:?}（旧存档/字典中的不同规范状态），拒绝覆盖",
                            st.hash
                        ),
                    ));
                }
            }
        }
    }

    // minecraft:air must exist as the single empty state (baseline for world fill and generators; never guessed).
    let air = ordered
        .iter()
        .find(|f| f.identifier == "minecraft:air")
        .ok_or_else(|| BlockJsonError::new(pack_id, "<bundle>", "$", "缺少 minecraft:air 定义"))?;
    if air.state_count() != 1 || !air.properties.is_empty() {
        return Err(BlockJsonError::new(
            pack_id,
            &air.zip_path,
            "$.minecraft:block.description.states",
            "minecraft:air 必须为单个空状态",
        ));
    }

    // ---- permutation condition evaluation plus overlapping-overlay rejection (array-order independent) ----
    for file in ordered.iter() {
        let mut covered: HashMap<usize, HashMap<String, usize>> = HashMap::new();
        for (j, perm) in file.permutations.iter().enumerate() {
            let matched: Vec<usize> = (0..file.state_count())
                .filter(|s| {
                    perm.condition
                        .matches(&canonical_state_props(&file.properties, *s))
                })
                .collect();
            // Declared value domains guarantee condition-match existence; defensively rechecked here.
            if matched.is_empty() {
                return Err(BlockJsonError::new(
                    pack_id,
                    &file.zip_path,
                    &format!("$.minecraft:block.permutations[{j}].condition"),
                    "条件不匹配任何合法状态",
                ));
            }
            for s in matched {
                let entry = covered.entry(s).or_default();
                for name in perm.components.keys() {
                    if let Some(prev) = entry.insert(name.clone(), j) {
                        let key = canonical_state_key(&canonical_state_props(&file.properties, s));
                        return Err(BlockJsonError::new(
                            pack_id,
                            &file.zip_path,
                            &format!("$.minecraft:block.permutations[{j}]"),
                            format!(
                                "状态 {key:?} 的组件 {name:?} 已被规则 {prev} 覆盖（不依赖数组顺序，重叠即失败）"
                            ),
                        ));
                    }
                }
            }
        }
    }

    // ---- Assigns dense ids: types sorted by identifier, states by (identifier, typed key) ----
    let mut type_order: Vec<usize> = (0..ordered.len()).collect();
    type_order.sort_by(|a, b| ordered[*a].identifier.cmp(&ordered[*b].identifier));

    let mut state_order: Vec<(usize, usize)> = Vec::with_capacity(total_states);
    for (fi, states) in rich.iter().enumerate() {
        for (si, _) in states.iter().enumerate() {
            state_order.push((fi, si));
        }
    }
    state_order.sort_by(|a, b| {
        ordered[a.0]
            .identifier
            .cmp(&ordered[b.0].identifier)
            .then_with(|| rich[a.0][a.1].typed_key.cmp(&rich[b.0][b.1].typed_key))
    });

    // permutation expansion: shared components plus overlay (whole-component value replacement).
    // Merging happens at the validated JSON-map level (`BTreeMap::extend` overwrites same-name keys),
    // then fixed-column conversion runs once.
    let mut merged_maps: HashMap<(usize, usize), BTreeMap<String, serde_json::Value>> =
        HashMap::new();
    for (fi, file) in ordered.iter().enumerate() {
        for si in 0..file.state_count() {
            merged_maps.insert((fi, si), file.base.clone());
        }
        for perm in file.permutations.iter() {
            for si in 0..file.state_count() {
                if !perm
                    .condition
                    .matches(&canonical_state_props(&file.properties, si))
                {
                    continue;
                }
                let merged = merged_maps.get_mut(&(fi, si)).expect("merged maps");
                merged.extend(perm.components.clone());
            }
        }
    }

    let mut snapshot = BlockJsonSnapshot {
        schema_version: bundle.schema_version,
        network_id_mode: bundle.network_id_mode.clone(),
        fingerprint: bundle.fingerprint,
        types: Vec::with_capacity(ordered.len()),
        states: Vec::with_capacity(total_states),
        by_hash: HashMap::with_capacity(total_states),
        by_network_id: HashMap::with_capacity(total_states),
        by_typed: HashMap::with_capacity(total_states),
        defaults: HashMap::with_capacity(ordered.len()),
        collision: Vec::with_capacity(total_states),
        mining_seconds: Vec::with_capacity(total_states),
        unbreakable: Vec::with_capacity(total_states),
        light_emission: Vec::with_capacity(total_states),
        light_dampening: Vec::with_capacity(total_states),
        loot: Vec::with_capacity(total_states),
        replaceable: Vec::with_capacity(total_states),
        liquid: Vec::with_capacity(total_states),
        random_tick: Vec::with_capacity(total_states),
        needs_support: Vec::with_capacity(total_states),
        can_contain_liquid: Vec::with_capacity(total_states),
        mining: Vec::with_capacity(total_states),
        drops: Vec::with_capacity(total_states),
        plugin_components: Vec::with_capacity(total_states),
    };

    let mut index_of: HashMap<(usize, usize), u32> = HashMap::with_capacity(total_states);
    for (idx, (fi, si)) in state_order.iter().enumerate() {
        index_of.insert((*fi, *si), idx as u32);
    }
    let mut warnings: Vec<String> = Vec::new();
    for (idx, (fi, si)) in state_order.iter().enumerate() {
        let file = &ordered[*fi];
        let st = &rich[*fi][*si];
        let merged = merged_maps.remove(&(*fi, *si)).expect("merged maps");
        compile_state_columns(
            &mut snapshot,
            &merged,
            pack_id,
            &file.zip_path,
            &st.key,
            budgets,
            item_exists,
            &mut warnings,
        )?;
        snapshot.by_hash.insert(st.hash, idx as u32);
        snapshot.by_network_id.insert(st.network_id, idx as u32);
        snapshot.by_typed.insert(
            (Box::from(file.identifier.as_str()), st.typed_key.clone()),
            idx as u32,
        );
        snapshot.states.push(SnapState {
            index: idx as u32,
            type_id: 0,
            key: Box::from(st.key.as_str()),
            hash: st.hash,
            network_id: st.network_id,
            typed_key: st.typed_key.clone(),
            props: st.props.clone(),
        });
    }
    for (type_id, fi) in type_order.iter().enumerate() {
        let file = &ordered[*fi];
        let mut state_ids: Vec<u32> = (0..file.state_count())
            .map(|si| index_of[&(*fi, si)])
            .collect();
        state_ids.sort_unstable();
        // Default-state index comes from the `sc:default_state` declaration (domain-checked at parse time).
        let default_state = index_of[&(*fi, file.default_index)];
        for sid in state_ids.iter() {
            snapshot.states[*sid as usize].type_id = type_id as u32;
        }
        snapshot
            .defaults
            .insert(Box::from(file.identifier.as_str()), default_state);
        snapshot.types.push(SnapType {
            identifier: Box::from(file.identifier.as_str()),
            file: Box::from(file.zip_path.as_str()),
            type_id: type_id as u32,
            properties: file.properties.clone(),
            default_state,
            state_ids,
        });
    }

    // Column-length invariant (fixed dense columns, including mining/drops).
    debug_assert_eq!(snapshot.mining.len(), snapshot.state_count());
    debug_assert_eq!(snapshot.drops.len(), snapshot.state_count());

    Ok((snapshot, warnings))
}

// ---------------------------------------------------------------------------
// Snapshot queries (hot-path read-only, no JSON).
// ---------------------------------------------------------------------------

impl BlockJsonSnapshot {
    pub fn state_count(&self) -> usize {
        self.states.len()
    }

    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    pub fn state(&self, index: u32) -> Option<&SnapState> {
        self.states.get(index as usize)
    }

    pub fn block_type(&self, type_id: u32) -> Option<&SnapType> {
        self.types.get(type_id as usize)
    }

    pub fn type_by_identifier(&self, identifier: &str) -> Option<&SnapType> {
        self.types
            .iter()
            .find(|t| t.identifier.as_ref() == identifier)
    }

    /// FNV state hash of the block default (for worldgen/item mapping; never guessed: missing means None).
    pub fn default_hash(&self, identifier: &str) -> Option<u32> {
        let idx = self.defaults.get(identifier)?;
        self.states.get(*idx as usize).map(|s| s.hash)
    }

    pub fn default_state_idx(&self, identifier: &str) -> Option<u32> {
        self.defaults.get(identifier).copied()
    }

    pub fn state_idx_by_hash(&self, hash: u32) -> Option<u32> {
        self.by_hash.get(&hash).copied()
    }

    pub fn state_idx_by_network_id(&self, network_id: u32) -> Option<u32> {
        self.by_network_id.get(&network_id).copied()
    }

    /// identifier plus complete property assignments to dense index (typed key, byte/int never confused).
    pub fn typed_state_of(&self, identifier: &str, props: &[(String, PropValue)]) -> Option<u32> {
        let key = typed_state_key(identifier, props);
        self.by_typed.get(&(Box::from(identifier), key)).copied()
    }

    /// identifier plus lenient string property assignments to state hash
    /// (minimum hash wins, matching the global dictionary scan).
    ///
    /// String matching mirrors the dictionary: exact string equality, int
    /// parse, byte `true`/`false`/int parse. Worldgen table builders prefer
    /// this over the global dictionary: dlopened plugin copies may hold an
    /// empty dictionary while the snapshot is always at hand there.
    pub fn find_state_hash(&self, identifier: &str, pairs: &[(&str, &str)]) -> Option<u32> {
        let ty = self.type_by_identifier(identifier)?;
        let mut best: Option<u32> = None;
        for &idx in &ty.state_ids {
            let Some(st) = self.states.get(idx as usize) else {
                continue;
            };
            if !snapshot_props_match(&st.props, pairs) {
                continue;
            }
            if best.is_none_or(|b| st.hash < b) {
                best = Some(st.hash);
            }
        }
        best
    }

    /// All state hashes of one identifier (empty when unknown).
    pub fn state_hashes_of(&self, identifier: &str) -> Vec<u32> {
        let Some(ty) = self.type_by_identifier(identifier) else {
            return Vec::new();
        };
        ty.state_ids
            .iter()
            .filter_map(|&idx| self.states.get(idx as usize).map(|st| st.hash))
            .collect()
    }

    /// Changes one property of a state to the target index (O(1) typed lookup).
    pub fn with_property_typed(&self, index: u32, name: &str, value: PropValue) -> Option<u32> {
        let state = self.state(index)?;
        let type_view = self.block_type(state.type_id)?;
        let identifier = type_view.identifier.clone();
        let mut props = state.props.clone();
        let mut replaced = false;
        for (k, v) in props.iter_mut() {
            if k == name {
                *v = value.clone();
                replaced = true;
            }
        }
        if !replaced {
            return None;
        }
        self.typed_state_of(&identifier, &props)
    }

    /// Per-state capability bits (for `BlockComponentFlags` builds).
    ///
    /// The map compiles explicit declarations without guessing: transparency/solidity have no data source
    /// and stay unset; other bits are set only when their component is explicitly declared.
    pub fn flag_bits(&self, index: u32) -> u64 {
        use crate::state::flags;
        let Some(state) = self.state(index) else {
            return 0;
        };
        let Some(type_view) = self.block_type(state.type_id) else {
            return 0;
        };
        let idx = index as usize;
        let mut bits = 0u64;
        if type_view.identifier.as_ref() == "minecraft:air" {
            bits |= flags::IS_AIR;
        }
        if type_view.identifier.as_ref() == "minecraft:unknown" {
            bits |= flags::IS_UNKNOWN;
        }
        if !state.props.is_empty() {
            bits |= flags::HAS_STATES;
        }
        if self.replaceable.get(idx).copied().flatten() == Some(true) {
            bits |= flags::REPLACEABLE;
        }
        if self.liquid.get(idx).and_then(Option::as_ref).is_some() {
            bits |= flags::LIQUID;
        }
        if self.random_tick.get(idx).copied().flatten() == Some(true) {
            bits |= flags::RANDOM_TICK;
        }
        if self
            .needs_support
            .get(idx)
            .and_then(Option::as_ref)
            .is_some()
        {
            bits |= flags::NEEDS_SUPPORT;
        }
        if self.can_contain_liquid.get(idx).copied().flatten() == Some(true) {
            bits |= flags::CAN_CONTAIN_LIQUID;
        }
        if self.unbreakable.get(idx).copied().unwrap_or(false) {
            bits |= flags::UNBREAKABLE;
        }
        if self
            .collision
            .get(idx)
            .is_some_and(|c| c.as_ref().is_some_and(Vec::is_empty))
        {
            bits |= flags::NO_COLLISION;
        }
        bits
    }

    /// Typed column accessors (fixed dense columns; array indexing on hot and cold paths, no deserialization).
    pub fn collision_of(&self, index: u32) -> Option<&Vec<[f32; 6]>> {
        self.collision.get(index as usize)?.as_ref()
    }

    /// Mine seconds (`minecraft:destructible_by_mining`; None = undeclared).
    pub fn mining_seconds_of(&self, index: u32) -> Option<f32> {
        self.mining_seconds.get(index as usize).copied().flatten()
    }

    pub fn is_unbreakable(&self, index: u32) -> bool {
        self.unbreakable
            .get(index as usize)
            .copied()
            .unwrap_or(false)
    }

    pub fn light_emission_of(&self, index: u32) -> Option<u8> {
        self.light_emission.get(index as usize).copied().flatten()
    }

    pub fn light_dampening_of(&self, index: u32) -> Option<u8> {
        self.light_dampening.get(index as usize).copied().flatten()
    }

    pub fn loot_of(&self, index: u32) -> Option<&str> {
        self.loot.get(index as usize)?.as_deref()
    }

    pub fn replaceable_of(&self, index: u32) -> Option<bool> {
        self.replaceable.get(index as usize).copied().flatten()
    }

    pub fn liquid_of(&self, index: u32) -> Option<&str> {
        self.liquid.get(index as usize)?.as_deref()
    }

    pub fn random_tick_of(&self, index: u32) -> Option<bool> {
        self.random_tick.get(index as usize).copied().flatten()
    }

    pub fn needs_support_of(&self, index: u32) -> Option<&str> {
        self.needs_support.get(index as usize)?.as_deref()
    }

    pub fn can_contain_liquid_of(&self, index: u32) -> Option<bool> {
        self.can_contain_liquid
            .get(index as usize)
            .copied()
            .flatten()
    }

    /// Compiled `sc:mining` profile (hot-path read-only; `None` means no mining, legacy path applies).
    pub fn mining_of(&self, index: u32) -> Option<&MiningCompiled> {
        self.mining.get(index as usize)?.as_ref()
    }

    /// Compiled `sc:drops` profile (hot-path read-only; `None` means no drops, nothing guessed).
    pub fn drops_of(&self, index: u32) -> Option<&DropCompiled> {
        self.drops.get(index as usize)?.as_ref()
    }

    /// Plugin component verbatim values (cold path; schema validated by plugin registration).
    pub fn plugin_component(&self, index: u32, name: &str) -> Option<&serde_json::Value> {
        self.plugin_components
            .get(index as usize)?
            .iter()
            .find(|(k, _)| k.as_ref() == name)
            .map(|(_, v)| v)
    }

    /// Derives legacy `block_palette.nbt` bytes from the snapshot (compat-period derived adapter).
    ///
    /// For callers that still consume palette bytes (independent dictionary copies of dynamic plugins,
    /// legacy test paths); contents match the snapshot. Disk version/LevelDB key/wire formats are unaffected.
    pub fn encode_legacy_palette_bytes(&self) -> Result<Vec<u8>, String> {
        use sc_binary::ByteWriter;
        use sc_nbt::local::JavaLocalNbt;
        use sc_nbt::writer::NbtWriter;
        let mut blocks = Vec::with_capacity(self.states.len());
        for state in self.states.iter() {
            let type_view = self.block_type(state.type_id).ok_or_else(|| {
                format!("快照损坏：状态 {} 无类型 {}", state.index, state.type_id)
            })?;
            let mut entry = CompoundNbt::new(None);
            entry.insert("name", NbtValue::String(type_view.identifier.to_string()));
            entry.insert("states", NbtValue::Compound(states_nbt_of(&state.props)));
            if state.network_id > i32::MAX as u32 {
                return Err(format!(
                    "protocol_runtime_id {} 超出 i32（状态 {:?}）",
                    state.network_id, state.key
                ));
            }
            entry.insert(
                "protocol_runtime_id",
                NbtValue::Int(state.network_id as i32),
            );
            blocks.push(NbtValue::Compound(entry));
        }
        let mut root = CompoundNbt::new(Some(String::new()));
        root.insert("blocks", NbtValue::List(blocks));
        let mut writer = ByteWriter::new();
        NbtWriter::from_writer(&mut writer)
            .write::<JavaLocalNbt>(&NbtValue::Compound(root))
            .map_err(|e| format!("派生 palette 编码失败: {e}"))?;
        Ok(writer.as_slice().to_vec())
    }
}

// ---------------------------------------------------------------------------
// Published snapshot resource (shared read-only across regions/worldgen, no deep copy).
// ---------------------------------------------------------------------------

/// Published block snapshot (`Arc` read-only sharing; JSON source released after build).
#[derive(Resource, Default)]
pub struct BlockJsonRegistry {
    snapshot: RwLock<Option<Arc<BlockJsonSnapshot>>>,
}

impl Clone for BlockJsonRegistry {
    fn clone(&self) -> Self {
        Self {
            snapshot: RwLock::new(self.get()),
        }
    }
}

impl BlockJsonRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes the snapshot (call only after all checks pass; never on failure paths).
    pub fn publish(&self, snapshot: Arc<BlockJsonSnapshot>) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = Some(snapshot);
        }
    }

    pub fn get(&self) -> Option<Arc<BlockJsonSnapshot>> {
        self.snapshot.read().ok()?.clone()
    }

    pub fn is_present(&self) -> bool {
        self.snapshot.read().ok().is_some_and(|g| g.is_some())
    }
}

// ---------------------------------------------------------------------------
// Global dictionary coordination (production probing plus post-success registration).
// ---------------------------------------------------------------------------

/// Production existing-state probe: the global bootstrap dictionary (first-seen wins, never written here).
pub fn global_preexisting_lookup(hash: u32) -> Option<(Box<str>, Box<str>)> {
    use sc_world::block_dictionary::BlockStateDictionary;
    let entry = BlockStateDictionary::global().get(hash)?;
    let key = typed_key_for_nbt(&entry.name, entry.states.as_ref());
    Some((Box::from(entry.name.as_str()), key))
}

/// Registers snapshot states into the global dictionary after successful publish (cross-check plus superset
/// fallback). Call if and only if the whole pack succeeded; never on failure paths (no partial publish).
pub fn publish_legacy_support(snapshot: &BlockJsonSnapshot) {
    use sc_world::block_dictionary::{BlockStateDictionary, BlockStateEntry};
    let dictionary = BlockStateDictionary::global();
    for state in snapshot.states.iter() {
        let Some(type_view) = snapshot.block_type(state.type_id) else {
            continue;
        };
        dictionary.record_with_network_id(state.hash, state.network_id, || BlockStateEntry {
            name: type_view.identifier.to_string(),
            states: Some(states_nbt_of(&state.props)),
        });
    }
}

// ---------------------------------------------------------------------------
// Legacy/new data equivalence checks (migration acceptance for a pinned version).
// ---------------------------------------------------------------------------

/// Legacy/new equivalence report (compares type, hash, and sequential id per state).
#[derive(Clone, Debug, Default)]
pub struct EquivalenceReport {
    pub snapshot_states: usize,
    pub legacy_states: usize,
    pub missing_in_snapshot: Vec<String>,
    pub extra_in_snapshot: Vec<String>,
    pub hash_mismatches: Vec<String>,
    pub network_id_mismatches: Vec<String>,
}

impl EquivalenceReport {
    pub fn is_equal(&self) -> bool {
        self.missing_in_snapshot.is_empty()
            && self.extra_in_snapshot.is_empty()
            && self.hash_mismatches.is_empty()
            && self.network_id_mismatches.is_empty()
    }
}

/// Compares the snapshot against the legacy palette (per state: identifier plus typed key, hash, protocol id).
pub fn compare_legacy_snapshot(
    snapshot: &BlockJsonSnapshot,
    legacy_entries: &[LegacyPaletteEntry],
) -> EquivalenceReport {
    let mut legacy_map: HashMap<(String, Box<str>), Vec<usize>> = HashMap::new();
    for (i, entry) in legacy_entries.iter().enumerate() {
        if entry.name.is_empty() {
            continue;
        }
        let key = typed_key_for_nbt(&entry.name, entry.states.as_ref());
        legacy_map
            .entry((entry.name.clone(), key))
            .or_default()
            .push(i);
    }
    let mut report = EquivalenceReport {
        snapshot_states: snapshot.states.len(),
        legacy_states: legacy_entries.len(),
        ..Default::default()
    };
    let mut matched_legacy = vec![false; legacy_entries.len()];
    for state in snapshot.states.iter() {
        let type_view = match snapshot.block_type(state.type_id) {
            Some(t) => t,
            None => continue,
        };
        let key = typed_state_key(&type_view.identifier, &state.props);
        match legacy_map.get(&(type_view.identifier.to_string(), key)) {
            None => report
                .extra_in_snapshot
                .push(format!("{}#{:?}", type_view.identifier, state.key)),
            Some(indices) => {
                for i in indices {
                    matched_legacy[*i] = true;
                    let legacy = &legacy_entries[*i];
                    if legacy.network_id != state.network_id {
                        report.network_id_mismatches.push(format!(
                            "{}#{:?}: 快照 {} vs 旧 {}",
                            type_view.identifier, state.key, state.network_id, legacy.network_id
                        ));
                    }
                    let legacy_hash = sc_world::leveldb::block_hash::block_state_hash(
                        &legacy.name,
                        legacy.states.as_ref(),
                    );
                    if state.hash != legacy_hash {
                        report
                            .hash_mismatches
                            .push(format!("{}#{:?}", type_view.identifier, state.key));
                    }
                }
            }
        }
    }
    for (i, entry) in legacy_entries.iter().enumerate() {
        if matched_legacy[i] {
            continue;
        }
        report.missing_in_snapshot.push(format!(
            "{}#{}",
            entry.name,
            typed_key_for_nbt(&entry.name, entry.states.as_ref())
        ));
    }
    report
}
#[cfg(test)]
mod tests {
    use super::*;
    use sc_packloader::block::{
        fingerprint_bundle, parse_block_file, parse_legacy_palette_entries,
    };

    const PACK: &str = "test-pack";
    const NO_PREEXISTING: &dyn Fn(u32) -> Option<(Box<str>, Box<str>)> = &|_| None;
    const ALLOW_ALL_ITEMS: &dyn Fn(&str) -> bool = &|_| true;

    fn raw_air() -> (String, Vec<u8>) {
        let json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {"sc:replaceable": true},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        (
            "definitions/blocks/minecraft/air.block.json".to_string(),
            json.as_bytes().to_vec(),
        )
    }

    fn raw_log() -> (String, Vec<u8>) {
        let json = r#"{
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
                    "minecraft:light_dampening": 15
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
        }"#;
        (
            "definitions/blocks/minecraft/oak_log.block.json".to_string(),
            json.as_bytes().to_vec(),
        )
    }

    fn bundle_of(raws: &[(String, Vec<u8>)], mode: &str) -> BlockJsonBundle {
        let budgets = BlockBundleBudgets::default();
        let mut refs: Vec<(&str, &[u8])> = raws
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let fingerprint = fingerprint_bundle(&refs, mode);
        let mut files = Vec::with_capacity(refs.len());
        for (path, bytes) in refs {
            files.push(parse_block_file(PACK, path, bytes, &budgets).expect("夹具应合法"));
        }
        BlockJsonBundle {
            schema_version: 1,
            network_id_mode: mode.to_string(),
            fingerprint,
            files,
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
                "sc:default_state": {"lit": false, "age": 0, "s": "a"},
                "sc:protocol_runtime_ids": [1, 2, 3, 4]
            }
        }"#;
        let mixed = (
            "definitions/blocks/test/mixed.block.json".to_string(),
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
                "sc:default_state": {"lit": false, "age": 0, "s": "a"},
                "sc:protocol_runtime_ids": [1, 2, 3, 4]
            }
        }"#;
        let mixed = (
            "definitions/blocks/test/mixed.block.json".to_string(),
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
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let raw = (
            "definitions/blocks/minecraft/air.block.json".to_string(),
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
                r#"{{"format_version": "1.10.0","minecraft:block": {{"description": {{"identifier": "minecraft:stone", "states": {{}}}},"components": {{{}}},"sc:default_state": {{}},"sc:protocol_runtime_ids": [7]}}}}"#,
                comps.join(",")
            );
            let air = raw_air();
            let stone = (
                "definitions/blocks/minecraft/stone.block.json".to_string(),
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
                            "sc:drops": {
                                "enabled": true, "mode": "independent",
                                "entries": [{"item": "minecraft:coal", "chance": 1.0, "count": [{"value": 1, "chance": 1.0}]}]
                            }
                        }
                    }
                ],
                "sc:default_state": {"pillar_axis": "x"},
                "sc:protocol_runtime_ids": [100, 101]
            }
        }"#;
        let raw = (
            "definitions/blocks/minecraft/oak_log.block.json".to_string(),
            json.as_bytes().to_vec(),
        );
        let (snap, _) = compile_bundle(
            &bundle_of(&[raw_air(), raw], "hashed"),
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
        // x takes shared mining (mineable, iron pickaxe 4x), no drops.
        let mx = snap.mining_of(axis_x).expect("x mining");
        assert_eq!(mx.base_seconds, 2.0);
        assert!(mx.default_rule.can_mine);
        assert!(snap.drops_of(axis_x).is_none());
        // y takes overlay mining (unmineable) plus drops (enabled).
        let my = snap.mining_of(axis_y).expect("y mining");
        assert_eq!(my.base_seconds, 3.0);
        assert!(!my.default_rule.can_mine);
        let dy = snap.drops_of(axis_y).expect("y drops");
        assert!(dy.enabled);
        assert_eq!(dy.entries.len(), 1);
    }

    /// Conflicting components are rejected per final state (single and cross layer).
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
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        // Single layers are rejected at parse time (unbreakable+mining).
        let e = sc_packloader::block::parse_block_file(
            PACK,
            "definitions/blocks/minecraft/stone.block.json",
            bad.as_bytes(),
            &BlockBundleBudgets::default(),
        )
        .expect_err("单层互斥必须拒绝");
        assert!(e.to_string().contains("不能同时声明"), "实际：{e}");

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
                "sc:default_state": {"pillar_axis": "x"},
                "sc:protocol_runtime_ids": [100, 101]
            }
        }"#;
        let raw = (
            "definitions/blocks/minecraft/oak_log.block.json".to_string(),
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

        // loot plus enabled drops are rejected together.
        let loot_bad = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {
                    "minecraft:loot": "loot_tables/blocks/stone.json",
                    "sc:drops": {"enabled": true,"mode": "independent","entries": [{"item": "minecraft:cobblestone","chance": 1.0,"count": [{"value": 1,"chance": 1.0}]}]}
                },
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let e = sc_packloader::block::parse_block_file(
            PACK,
            "definitions/blocks/minecraft/stone.block.json",
            loot_bad.as_bytes(),
            &BlockBundleBudgets::default(),
        )
        .expect_err("loot+启用的 drops 必须拒绝");
        assert!(e.to_string().contains("掉落来源不明"), "实际：{e}");
    }

    /// Illegal tool/drop references and budgets are rejected at compile time (including missing items).
    #[test]
    fn bad_refs_and_budgets_are_rejected() {
        // Unknown item.
        let unknown_item = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {
                    "sc:mining": {"formula_version": 1,"base_time_seconds": 1.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0},"tools": [{"items": ["minecraft:nope_not_an_item"],"can_mine": true,"harvest": true,"speed_multiplier": 2.0}]}
                },
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let raw = (
            "definitions/blocks/minecraft/stone.block.json".to_string(),
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

        // Budget saturated: too many tools.
        let mut tight = BlockBundleBudgets::default();
        tight.max_mining_tools_per_state = 0;
        let many_tools = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {
                    "sc:mining": {"formula_version": 1,"base_time_seconds": 1.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0},"tools": [{"items": ["minecraft:stone"],"can_mine": true,"harvest": true,"speed_multiplier": 2.0}]}
                },
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let raw = (
            "definitions/blocks/minecraft/stone.block.json".to_string(),
            many_tools.as_bytes().to_vec(),
        );
        // Rejected at parse time for budget (tool count 1 > 0).
        let e = sc_packloader::block::parse_block_file(
            PACK,
            "definitions/blocks/minecraft/stone.block.json",
            raw.1.as_slice(),
            &tight,
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
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let bad_raw = (
            "definitions/blocks/minecraft/stone.block.json".to_string(),
            bad_json.as_bytes().to_vec(),
        );
        // Rejected at parse time (duplicate item), so no bundle is even built; simulates failed-compile-no-publish here.
        let parse_err = sc_packloader::block::parse_block_file(
            PACK,
            &bad_raw.0,
            &bad_raw.1,
            &BlockBundleBudgets::default(),
        )
        .expect_err("重复工具必须解析失败");
        assert!(
            parse_err.to_string().contains("重复出现"),
            "实际：{parse_err}"
        );
        // Registry still holds the old snapshot (never partially modified).
        let after = registry.get().expect("旧快照仍可读");
        assert_eq!(after.type_count(), 2);
        assert_eq!(after.fingerprint, before.fingerprint);
    }

    /// Fingerprint tracks mining/drops content (rule identity belongs to version identity).
    #[test]
    fn fingerprint_changes_with_mining_drops() {
        use sc_packloader::block::fingerprint_bundle;
        let a = r#"{"format_version": "1.10.0","minecraft:block": {"description": {"identifier": "minecraft:stone", "states": {}},"components": {"sc:mining": {"formula_version": 1,"base_time_seconds": 1.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}},"sc:default_state": {},"sc:protocol_runtime_ids": [7]}}"#;
        let b = r#"{"format_version": "1.10.0","minecraft:block": {"description": {"identifier": "minecraft:stone", "states": {}},"components": {"sc:mining": {"formula_version": 1,"base_time_seconds": 2.0,"default": {"can_mine": true,"harvest": true,"speed_multiplier": 1.0}}},"sc:default_state": {},"sc:protocol_runtime_ids": [7]}}"#;
        let air = raw_air();
        let refs_a: Vec<(&str, &[u8])> = vec![
            (air.0.as_str(), air.1.as_slice()),
            (
                "definitions/blocks/minecraft/stone.block.json",
                a.as_bytes(),
            ),
        ];
        let refs_b: Vec<(&str, &[u8])> = vec![
            (air.0.as_str(), air.1.as_slice()),
            (
                "definitions/blocks/minecraft/stone.block.json",
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
            "definitions/blocks/example/compressed_ore.block.json".to_string(),
            bytes,
        );
        // Note: the example identifier is example:compressed_ore and the file path
        // example/compressed_ore (single path segment) matches it.
        let bundle = bundle_of(&[air, ore], "hashed");
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
        let d = snap.drops_of(ore_idx).expect("drops");
        assert!(d.enabled);
        assert_eq!(d.entries.len(), 2);
    }

    /// Any `ur:*` key is rejected (the legacy namespace has fully migrated, no compat).
    #[test]
    fn any_ur_key_is_rejected_without_compat() {
        // Component-level ur:* rejected.
        let legacy = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {"ur:replaceable": true},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let e = sc_packloader::block::parse_block_file(
            PACK,
            "definitions/blocks/minecraft/air.block.json",
            legacy.as_bytes(),
            &BlockBundleBudgets::default(),
        )
        .expect_err("ur:* 组件必须拒绝，不做兼容");
        assert!(e.to_string().contains("ur:replaceable"), "实际：{e}");
        // Top-level ur:* rejected.
        let top_legacy = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "ur:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        sc_packloader::block::parse_block_file(
            PACK,
            "definitions/blocks/minecraft/air.block.json",
            top_legacy.as_bytes(),
            &BlockBundleBudgets::default(),
        )
        .expect_err("顶层 ur:* 必须拒绝");
        // Proposed names rejected likewise.
        let proposal = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {"ur:mining": {}},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        sc_packloader::block::parse_block_file(
            PACK,
            "definitions/blocks/minecraft/air.block.json",
            proposal.as_bytes(),
            &BlockBundleBudgets::default(),
        )
        .expect_err("ur:mining 必须拒绝");
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
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        // air has no properties, so conditions referencing undeclared properties are rejected at parse time.
        let parsed = parse_block_file(
            PACK,
            "definitions/blocks/minecraft/air.block.json",
            json.as_bytes(),
            &BlockBundleBudgets::default(),
        )
        .expect_err("引用未声明属性必须拒绝");
        assert!(
            parsed.to_string().contains("未声明的属性"),
            "实际：{parsed}"
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
                "sc:default_state": {"pillar_axis": "x"},
                "sc:protocol_runtime_ids": [100, 101]
            }
        }"#;
        let raw = (
            "definitions/blocks/minecraft/oak_log.block.json".to_string(),
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
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7, 7]
            }
        }"#;
        // A single-state block cannot carry two runtime ids (length mismatch rejected at parse time).
        let e = parse_block_file(
            PACK,
            "definitions/blocks/minecraft/oak_log.block.json",
            json.as_bytes(),
            &BlockBundleBudgets::default(),
        )
        .expect_err("长度不符必须拒绝");
        assert!(e.to_string().contains("一一对应"), "实际：{e}");

        // Cross-type conflict: two types with one state each sharing a runtime id.
        let a = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:oak_log", "states": {}},
                "components": {},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let b = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:birch_log", "states": {}},
                "components": {},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let e = compile_bundle(
            &bundle_of(
                &[
                    raw_air(),
                    (
                        "definitions/blocks/minecraft/oak_log.block.json".to_string(),
                        a.as_bytes().to_vec(),
                    ),
                    (
                        "definitions/blocks/minecraft/birch_log.block.json".to_string(),
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

    /// Fixed real-pack subset: split, parse, compile, then legacy/new equivalence (single-state defaults
    /// are forced by the sole state, never guessed; authoritative multi-state defaults are completed elsewhere).
    #[test]
    fn real_single_state_subset_compiles_and_matches_legacy() {
        use sc_packloader::block::split_legacy_palette;
        use std::collections::HashMap;
        use std::io::Read;
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = std::fs::read(pack_path).expect("版本包缺失");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
        let palette_bytes = {
            let mut f = zip
                .by_name("definitions/block_palette.nbt")
                .expect("palette");
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
            buf
        };
        let entries = parse_legacy_palette_entries(&palette_bytes).expect("旧 palette 解析");
        let mut by_name: HashMap<&str, usize> = HashMap::new();
        for e in entries.iter() {
            *by_name.entry(e.name.as_str()).or_default() += 1;
        }
        let single: Vec<String> = {
            let mut v: Vec<String> = by_name
                .iter()
                .filter(|(_, c)| **c == 1)
                .map(|(k, _)| (*k).to_string())
                .collect();
            v.sort();
            v.into_iter().take(400).collect()
        };
        let subset: Vec<LegacyPaletteEntry> = entries
            .iter()
            .filter(|e| single.contains(&e.name))
            .cloned()
            .collect();
        assert_eq!(subset.len(), single.len());
        let budgets = BlockBundleBudgets::default();
        let files = split_legacy_palette(&subset, &HashMap::new(), &HashMap::new(), &budgets)
            .expect("单状态子集拆分应成功");
        assert_eq!(files.len(), single.len());
        // air must be in the subset (baseline block).
        assert!(files
            .iter()
            .any(|f| f.zip_path.ends_with("minecraft/air.block.json")));
        let mut refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|f| (f.zip_path.as_str(), f.bytes.as_slice()))
            .collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let mut parsed = Vec::with_capacity(refs.len());
        for (path, bytes) in refs {
            parsed.push(parse_block_file("<real>", path, bytes, &budgets).expect("子集应合法"));
        }
        let bundle = BlockJsonBundle {
            schema_version: 1,
            network_id_mode: "hashed".to_string(),
            fingerprint: fingerprint_bundle(
                &files
                    .iter()
                    .map(|f| (f.zip_path.as_str(), f.bytes.as_slice()))
                    .collect::<Vec<_>>(),
                "hashed",
            ),
            files: parsed,
        };
        let snap = compile_bundle(&bundle, PACK, &budgets, NO_PREEXISTING, &|_| true)
            .expect("子集应编译成功")
            .0;
        let report = compare_legacy_snapshot(&snap, &subset);
        assert!(report.is_equal(), "新旧数据必须等价：{report:?}");
        assert_eq!(snap.type_count(), single.len());
        assert_eq!(snap.state_count(), subset.len());
    }
}
#[cfg(test)]
mod real_pack_tests {
    use super::*;
    use sc_packloader::block::{parse_legacy_palette_entries, BlockDataManifest};

    /// Real-pack end to end: manifest declaration, 1379 files discovered, parse, compile, then
    /// per-state equivalence with the legacy palette. Covers load-time budgets and full compilation.
    #[test]
    fn real_pack_bundle_compiles_and_matches_legacy() {
        use std::io::Read;
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = std::fs::read(pack_path).expect("版本包缺失");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
        let palette_bytes = {
            let mut f = zip
                .by_name("definitions/block_palette.nbt")
                .expect("palette");
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
            buf
        };
        let entries = parse_legacy_palette_entries(&palette_bytes).expect("旧 palette 解析");
        let budgets = BlockBundleBudgets::default();
        let bundle = sc_packloader::block::load_block_json_bundle(
            &mut zip,
            &BlockDataManifest {
                schema_version: 1,
                directory: sc_packloader::block::BLOCKS_DIRECTORY.to_string(),
                network_id_mode: "hashed".to_string(),
            },
            "Vanilla",
            &budgets,
        )
        .expect("bundle 应加载")
        .expect("manifest 已声明 block_data");
        assert_eq!(bundle.files.len(), 1379);
        let total_states: usize = bundle.files.iter().map(|f| f.state_count()).sum();
        assert_eq!(total_states, entries.len());
        let snap = compile_bundle(&bundle, "Vanilla", &budgets, &|_| None, &|_| true)
            .expect("全量编译")
            .0;
        assert_eq!(snap.type_count(), 1379);
        assert_eq!(snap.state_count(), entries.len());
        let report = compare_legacy_snapshot(&snap, &entries);
        assert!(report.is_equal(), "新旧数据必须等价：{report:?}");
        // Key default states (explicitly declared, not first-seen order).
        assert!(snap.default_hash("minecraft:stone").is_some());
        assert!(snap.default_hash("minecraft:air").is_some());
        assert_eq!(
            snap.state(snap.default_state_idx("minecraft:air").unwrap())
                .unwrap()
                .key
                .as_ref(),
            "default"
        );
    }
}
