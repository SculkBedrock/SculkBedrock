//! 解析后的方块 bundle 到快照的语义编译。
//!
//! Semantic compilation of parsed block bundles into snapshots.
//!
//! 由 [`super`] 再导出，既有引用路径保持不变。
//!
//! Re-exported from [`super`] so existing paths keep compiling.

use super::*;
use std::collections::{BTreeMap, HashMap};
use sc_packloader::block::{
    canonical_state_key, canonical_state_props, BlockBundleBudgets, BlockJsonBundle,
    BlockJsonError, ParsedBlockFile, PropValue,
};
use sc_packloader::block::BlockComponents;
use sc_packloader::macros::ComponentKey;
use sc_packloader::loot_tables::Loot;
use crate::mining_drops::{compile_drops_from_loot, compile_mining};
use crate::workstation::{compile_chest, compile_crafting_table, compile_furnace};

/// 把合并后的组件映射编译为固定列（逐状态）。
///
/// Compiles the merged component map into fixed columns (per state).
///
/// 输入在 packloader 解析期已逐项校验；本步骤检查合并后语义
/// （互斥、继承、存在性）并稠密编译，失败即拒绝整个包
/// （无部分快照、无回退）。
///
/// Inputs were validated per item at packloader parse time; this step checks post-merge semantics
/// (exclusion, inheritance, existence) and compiles densely, rejecting the whole pack on failure
/// (no partial snapshot, no fallback).
///
/// - `snapshot`: 目标快照（列追加写入）
/// - `merged`: 合并后的组件映射
/// - `loot_tables`: 包内战利品表
/// - `pack_id` / `file` / `key`: 定位用的包 id、文件名与状态键
/// - `budgets`: bundle 预算
/// - `item_exists`: 物品存在性判定
/// - `warnings`: 告警收集器
fn compile_state_columns(
    snapshot: &mut BlockJsonSnapshot,
    merged: &BTreeMap<String, serde_json::Value>,
    loot_tables: &BTreeMap<String, serde_json::Value>,
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

    let collision: Option<Vec<[f32; 6]>> = match get(CollisionBoxes::NAME) {
        None => None,
        Some(v) => {
            let boxes = collision_boxes_of(&v)
                .map_err(|e| bad(CollisionBoxes::NAME, format!("内部不一致：{e}")))?;
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
    let drops = compile_drops_from_loot(
        loot.as_deref(),
        loot_tables,
        pack_id,
        file,
        key,
        budgets,
        item_exists,
    )?;
    let chest = compile_chest(merged, pack_id, file, key, budgets)?;
    let furnace = compile_furnace(merged, pack_id, file, key, budgets)?;
    let crafting_table = compile_crafting_table(merged, pack_id, file, key)?;

    let mut plugin = Vec::new();
    for (name, value) in merged.iter() {
        if !BlockComponents::is_known(name) {
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
    snapshot.chests.push(chest);
    snapshot.furnaces.push(furnace);
    snapshot.crafting_tables.push(crafting_table);
    snapshot.plugin_components.push(plugin);
    Ok(())
}

/// 编译整个 bundle。`preexisting` 是既有状态查询（生产传全局字典探测，
/// 测试传隔离表）：`hash -> (标识符, 类型化键)`；规范状态不同即使整包失败。
/// `item_exists` 是工具/掉落物品存在性查询（生产传版本包物品注册表探测，
/// 测试传隔离表；未知即拒绝整个包，无 `default` 回退）。
///
/// Compiles the whole bundle. `preexisting` is the existing-state lookup (production passes the global
/// dictionary probe, tests pass an isolated table): `hash -> (identifier, typed key)`; a differing canonical
/// state fails the whole pack.
/// `item_exists` is the tool/drop item existence lookup (production passes the version-pack item registry
/// probe, tests pass an isolated table; unknown rejects the whole pack, no `default` fallback).
///
/// 成功时返回完整校验的快照加告警（如值冲突；告警不拒绝）；
/// 失败时不产出部分结果（调用方不得发布）。
///
/// On success returns the fully validated snapshot plus warnings (e.g. conflicting values; warnings do not reject);
/// on failure produces no partial output (callers must not publish).
///
/// - `bundle`: 解析后的方块 bundle
/// - `pack_id`: 包标识（错误定位用）
/// - `budgets`: bundle 预算
/// - `preexisting`: 既有状态查询
/// - `item_exists`: 物品存在性判定
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
    /// 富状态中间表示（规范键、类型化键、哈希、network id 与属性）。
    ///
    /// Rich-state intermediate representation (canonical key, typed key, hash,
    /// network id and props).
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
        chests: Vec::with_capacity(total_states),
        furnaces: Vec::with_capacity(total_states),
        crafting_tables: Vec::with_capacity(total_states),
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
            &bundle.loot_tables,
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
        // Default-state index = canonical index 0 (first declared value of every property).
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