//! Bundle discovery/load from behavior packs, fingerprinting, and legacy-palette splitting.
//!
//! Loading is direct: for each behavior pack (in the caller's order), discover
//! `blocks/**/*.json`, read, parse. No per-file budgets or path contracts —
//! only the manifest declaration is validated and an empty declared bundle
//! stays a loud error. When a block file's `minecraft:loot` references a
//! loot-table file, the loader calls `crate::loot_tables::parse_loot_tables`
//! and inserts the parsed [`crate::loot_tables::LootTables`] component into
//! the block's component map (base and permutation layers alike); the
//! reference resolves inside the same behavior pack (its `loot_tables/`
//! directory), and unresolvable references are left untouched.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use serde_json::Value;

use super::parse::{
    canonical_state_count, canonical_state_index, canonical_state_key, canonical_state_props,
    parse_block_file, NbtPropType, ParsedBlockFile, PropertyDef, PropValue, validate_identifier,
};
use super::{BlockDataManifest, BlockJsonError, BLOCKS_DIRECTORY, BLOCK_JSON_FORMAT_VERSION};
use sc_error::{ScError, ScSite, capture_site};
use crate::loot_tables;

pub(super) fn fnv1a64(data: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = OFFSET;
    for b in data {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Content fingerprint (sorted path+bytes+mode, FNV1a-64): default state, valid states, and tags
/// all feed the fingerprint; it can feed the generator descriptor (consumed by worldgen).
///
/// Mining/drop rules are already in the file bytes, so content changes alter the fingerprint.
pub fn fingerprint_bundle(files_sorted: &[(&str, &[u8])], network_id_mode: &str) -> u64 {
    let mut seed = Vec::new();
    seed.extend_from_slice(network_id_mode.as_bytes());
    seed.push(0);
    let mut fp = fnv1a64(&seed);
    for (path, bytes) in files_sorted {
        let mut chunk = fp.to_le_bytes().to_vec();
        chunk.extend_from_slice(path.as_bytes());
        chunk.push(0);
        fp = fnv1a64(&chunk);
        let mut chunk = fp.to_le_bytes().to_vec();
        chunk.extend_from_slice(bytes);
        chunk.push(0);
        fp = fnv1a64(&chunk);
    }
    fp
}

// ---------------------------------------------------------------------------
// Bundle: discovery, reads, sorting, parsing
// ---------------------------------------------------------------------------

/// Parsed version-pack block bundle (input to sc_block compilation).
#[derive(Clone, Debug)]
pub struct BlockJsonBundle {
    pub schema_version: u32,
    pub network_id_mode: String,
    pub fingerprint: u64,
    pub files: Vec<ParsedBlockFile>,
}

/// Discover and parse the block bundle from the nested behavior packs.
///
/// - Without a manifest `block_data` declaration, callers pass nothing here (legacy palette path);
/// - Once declared: zero files and JSON errors return Err; callers must not fall back to the legacy format.
/// - Pack order is caller-owned (low → high version); files inside one pack sort by path,
///   and equal paths keep pack order (stable sort), so the result is deterministic.
pub fn load_block_json_bundle(
    behavior_packs: &mut [crate::pack_loader::pack::zipped::PackSource],
    manifest: &BlockDataManifest,
    pack_id: &str,
) -> Result<Option<BlockJsonBundle>, BlockJsonError> {
    manifest.validate(pack_id)?;
    let prefix = format!("{BLOCKS_DIRECTORY}/");
    let mut entries: Vec<(String, Vec<u8>, ParsedBlockFile)> = Vec::new();
    for pack in behavior_packs.iter_mut() {
        let mut names: Vec<String> = pack
            .file_names()
            .into_iter()
            .filter(|name| name.starts_with(&prefix) && name.ends_with(".json"))
            .collect();
        names.sort();
        for name in names {
            let bytes = pack.read_file(&name).ok_or_else(|| {
                BlockJsonError::new(pack_id, &name, "$", "pack 读取失败".to_string())
            })?;
            let mut parsed = parse_block_file(pack_id, &name, &bytes)?;
            // Loot references resolve inside the same behavior pack.
            loot_tables::attach_loot_tables(&mut parsed.base, &mut |path| pack.read_file(path));
            for permutation in parsed.permutations.iter_mut() {
                loot_tables::attach_loot_tables(&mut permutation.components, &mut |path| {
                    pack.read_file(path)
                });
            }
            entries.push((name, bytes, parsed));
        }
    }
    if entries.is_empty() {
        return Err(BlockJsonError::new(
            pack_id,
            "<bundle>",
            "$",
            "manifest 声明了 block_data，但 behavior_packs 内未发现任何 blocks/**/*.json",
        ));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let sorted_refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(p, b, _)| (p.as_str(), b.as_slice()))
        .collect();
    let fingerprint = fingerprint_bundle(&sorted_refs, &manifest.network_id_mode);
    let files = entries.into_iter().map(|(_, _, parsed)| parsed).collect();
    Ok(Some(BlockJsonBundle {
        schema_version: manifest.schema_version,
        network_id_mode: manifest.network_id_mode.clone(),
        fingerprint,
        files,
    }))
}

// ---------------------------------------------------------------------------
// Legacy palette parsing (input to equivalence checks and splitting; touches no global state)
// ---------------------------------------------------------------------------

/// Parse output for one legacy palette entry.
#[derive(Clone, Debug)]
pub struct LegacyPaletteEntry {
    pub name: String,
    pub states: Option<CompoundNbt>,
    pub version: i32,
    pub network_id: u32,
    pub extra: Option<CompoundNbt>,
}

/// Legacy palette metadata (remaining root fields stay in `extra` for extractor diagnostics, never entering
/// the block JSON model, which has no vanilla counterpart slot).
pub(super) const RESERVED_META_KEYS: &[&str] = &[
    "name",
    "states",
    "version",
    "protocol_runtime_id",
    "runtime_id",
    "runtimeId",
    "network_id",
    "block_id",
    "id",
    "data",
    "stateOverload",
    "name_hash",
];

/// Parse the legacy `block_palette.nbt`.
pub fn parse_legacy_palette_entries(bytes: &[u8]) -> Result<Vec<LegacyPaletteEntry>, String> {
    use sc_binary::ByteReader;
    use sc_nbt::local::JavaLocalNbt;
    use sc_nbt::reader::NbtReader;
    let mut reader = ByteReader::from(bytes.to_vec());
    let root = NbtReader::from_reader(&mut reader)
        .read::<JavaLocalNbt>()
        .map_err(|e| format!("旧 palette NBT 读取失败: {e}"))?;
    let NbtValue::Compound(root) = &root else {
        return Err("旧 palette 根不是 Compound".to_string());
    };
    let Some(NbtValue::List(blocks)) = root.get("blocks") else {
        return Err("旧 palette 缺少 blocks 列表".to_string());
    };
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        let NbtValue::Compound(entry) = block else {
            continue;
        };
        let Some(name) = entry.get("name").and_then(NbtValue::as_string) else {
            continue;
        };
        let states = entry.get("states").and_then(NbtValue::as_compound).cloned();
        let version = entry.get("version").and_then(NbtValue::as_i32).unwrap_or(0);
        let network_id = entry
            .get("protocol_runtime_id")
            .or_else(|| entry.get("runtime_id"))
            .or_else(|| entry.get("runtimeId"))
            .and_then(NbtValue::as_i32)
            .ok_or_else(|| format!("旧状态缺少 protocol_runtime_id: {name}"))?;
        if network_id < 0 {
            return Err(format!("旧 protocol_runtime_id 为负：{name}"));
        }
        let mut extra = CompoundNbt::new(None);
        for (k, v) in entry.iter() {
            if RESERVED_META_KEYS.contains(&k.as_str()) {
                continue;
            }
            extra.insert(k, v.clone());
        }
        out.push(LegacyPaletteEntry {
            name: name.clone(),
            states,
            version,
            network_id: network_id as u32,
            extra: if extra.is_empty() { None } else { Some(extra) },
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Legacy palette to .json splitting (core shared by the extractor and migration tests)
// ---------------------------------------------------------------------------

/// Block hardness (data source for components).
///
/// - `Breakable(h)` to `minecraft:destructible_by_mining: {"value": h}` (h non-negative finite);
/// - `Unbreakable` to `sc:unbreakable: {}` (source `-1` convention).
///
/// Orthogonal to default state and other capability bits; [`split_legacy_palette`] only writes when declared,
/// keeping the component absent when undeclared (never guessed).
#[derive(Clone, Debug)]
pub enum BlockHardness {
    Breakable(f32),
    Unbreakable,
}

/// Parse hardness JSON (`extract-hardness` output):
/// `{identifier: {"hardness": <non-negative finite number>} | {"unbreakable": true}}`.
pub fn parse_hardness_json(value: &Value) -> Result<HashMap<String, BlockHardness>, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "hardness 根应为对象".to_string())?;
    let mut out = HashMap::with_capacity(obj.len());
    for (id, item) in obj.iter() {
        let entry = item.as_object().ok_or_else(|| format!("{id} 应为对象"))?;
        let (breaking, unbreaking) = (entry.get("hardness"), entry.get("unbreakable"));
        match (breaking, unbreaking) {
            (Some(h), None) => {
                let f = h
                    .as_f64()
                    .ok_or_else(|| format!("{id}.hardness 应为数值"))?;
                if !f.is_finite() || f < 0.0 {
                    return Err(format!("{id}.hardness 必须为非负有限数值"));
                }
                if f > f32::MAX as f64 {
                    return Err(format!("{id}.hardness 超出 f32"));
                }
                out.insert(id.clone(), BlockHardness::Breakable(f as f32));
            }
            (None, Some(u)) => {
                if u.as_bool() != Some(true) {
                    return Err(format!("{id}.unbreakable 应为 true"));
                }
                out.insert(id.clone(), BlockHardness::Unbreakable);
            }
            _ => {
                return Err(format!("{id} 须为恰一种形式（hardness 或 unbreakable）"));
            }
        }
    }
    Ok(out)
}

/// Split output: zip path + canonical JSON bytes.
#[derive(Clone, Debug)]
pub struct GeneratedBlockFile {
    pub zip_path: String,
    pub bytes: Vec<u8>,
}

/// Split error: missing default state (explicitly pending, never replaced by the first state) or illegal input.
#[derive(Clone, Debug, ScError)]
pub enum SplitError {
    /// Multi-state blocks without an authoritative default.
    #[sc_error(code = "packloader.split.missing_defaults")]
    MissingDefaults {
        /// Identifiers awaiting a default declaration.
        ids: Vec<String>,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Illegal split input.
    #[sc_error(code = "packloader.split.invalid")]
    Invalid {
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
}

impl SplitError {
    /// Build a missing-defaults error, capturing the call site.
    #[track_caller]
    pub fn missing_defaults(ids: Vec<String>) -> Self {
        let detail = format!(
            "missing authoritative default states for {} block(s): {}",
            ids.len(),
            ids.join(", ")
        );
        Self::MissingDefaults {
            ids,
            detail,
            site: capture_site(),
        }
    }

    /// Build an invalid-input error, capturing the call site.
    #[track_caller]
    pub fn invalid(detail: String) -> Self {
        Self::Invalid {
            detail,
            site: capture_site(),
        }
    }
}

impl fmt::Display for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDefaults { detail, site, .. } => {
                write!(f, "[packloader.split.missing_defaults] {detail} (at {site})")
            }
            Self::Invalid { detail, site } => {
                write!(f, "[packloader.split.invalid] {detail} (at {site})")
            }
        }
    }
}

impl std::error::Error for SplitError {}


pub(super) fn nbt_states_to_prop(
    identifier: &str,
    prop: &str,
    v: &NbtValue,
) -> Result<(NbtPropType, PropValue), String> {
    match v {
        NbtValue::Byte(b) => Ok((NbtPropType::Byte, PropValue::Byte(*b))),
        NbtValue::Int(i) => Ok((NbtPropType::Int, PropValue::Int(*i))),
        NbtValue::String(s) => Ok((NbtPropType::String, PropValue::String(s.clone()))),
        other => Err(format!(
            "{identifier} 的属性 {prop:?} 类型 tag={} 不在首版 byte|int|string 内（不删属性硬导）",
            other.tag()
        )),
    }
}

/// Split state key (deterministic; shared by `extract-defaults` output and [`split_legacy_palette`]).
///
/// - No properties to `default`;
/// - One property to `{prop}_{repr}` (repr: byte/int decimal, string raw value);
/// - Many properties to `state_{index}` (index is the extraction-source order).
pub fn split_state_key(
    prop_count: usize,
    single_name_repr: Option<(String, String)>,
    index: usize,
) -> String {
    if prop_count == 0 {
        "default".to_string()
    } else if let Some((name, repr)) = single_name_repr {
        format!("{name}_{repr}")
    } else {
        format!("state_{index}")
    }
}

/// Split legacy palette entries by identifier into vanilla-structure `.json` files
/// (`blocks/<namespace>/<path>.json`, the behavior-pack layout).
///
/// - Property types come from the extraction-source NBT tag (authoritative, never guessed); conflicting types fail;
/// - `description.states` value order = deterministic order of that property value set in the extraction source
///   (ascending by (type tag, text)), matching canonical state enumeration;
/// - `sc:protocol_runtime_ids` aligns in canonical state order (each state taken from the source
///   `protocol_runtime_id`, never recomputed); non-cartesian or duplicated source combinations fail;
/// - `sc:default_state` is written explicitly; single/zero-state blocks are pinned by the only state, multi-state blocks
///   without a `defaults` declaration go to the pending list (returned as a batch, never replaced by the first state);
/// - Components are only written with an authoritative declaration (currently hardness); the rest stay undeclared.
pub fn split_legacy_palette(
    entries: &[LegacyPaletteEntry],
    defaults: &HashMap<String, String>,
    hardness: &HashMap<String, BlockHardness>,
) -> Result<Vec<GeneratedBlockFile>, SplitError> {
    let max_identifier_len = 256;
    let mut grouped: BTreeMap<&str, Vec<&LegacyPaletteEntry>> = BTreeMap::new();
    for entry in entries {
        if entry.name.is_empty() {
            continue;
        }
        validate_identifier(&entry.name, max_identifier_len).map_err(|m| {
            SplitError::invalid(format!("旧条目 identifier 非法 {:?}：{m}", entry.name))
        })?;
        grouped.entry(entry.name.as_str()).or_default().push(entry);
    }
    let mut missing_defaults = Vec::new();
    let mut out = Vec::with_capacity(grouped.len());
    let empty_states = CompoundNbt::new(None);
    for (identifier, states) in grouped.iter() {
        // ---- Property declarations (value sets ascending by (tag, repr), deterministic) ----
        let mut prop_types: BTreeMap<&str, NbtPropType> = BTreeMap::new();
        let mut prop_values: BTreeMap<&str, BTreeSet<(char, String)>> = BTreeMap::new();
        // Source state to canonical key (aligns with the defaults-side declaration).
        let mut source_props: Vec<BTreeMap<String, PropValue>> = Vec::with_capacity(states.len());
        let mut source_network: Vec<u32> = Vec::with_capacity(states.len());
        for entry in states.iter() {
            let map = entry.states.as_ref().unwrap_or(&empty_states);
            let mut assignment: BTreeMap<String, PropValue> = BTreeMap::new();
            for (k, v) in map.iter() {
                let (t, pv) = nbt_states_to_prop(identifier, k, v).map_err(SplitError::invalid)?;
                if let PropValue::Byte(b) = pv {
                    if b != 0 && b != 1 {
                        return Err(SplitError::invalid(format!(
                            "{identifier} 的属性 {k:?} 取 byte 值 {b}（JSON 侧只能表达 0/1 布尔，不做有损转换）"
                        )));
                    }
                }
                if let Some(prev) = prop_types.get(k.as_str()) {
                    if *prev != t {
                        return Err(SplitError::invalid(format!(
                            "{identifier} 的属性 {k:?} 在提取源中类型冲突（不归一化）"
                        )));
                    }
                } else {
                    prop_types.insert(k.as_str(), t);
                }
                prop_values
                    .entry(k.as_str())
                    .or_default()
                    .insert((t.tag(), pv.repr()));
                assignment.insert(k.clone(), pv);
            }
            source_props.push(assignment);
            source_network.push(entry.network_id);
        }
        // Every property (including property-less blocks with empty states) must appear in each state.
        for (i, assignment) in source_props.iter().enumerate() {
            if assignment.len() != prop_types.len() {
                return Err(SplitError::invalid(format!(
                    "{identifier} 第 {i} 个源状态缺少属性（源 states 组合不一致，不补默认值）"
                )));
            }
        }
        let mut properties: Vec<PropertyDef> = prop_types
            .iter()
            .map(|(name, t)| PropertyDef {
                name: (*name).to_string(),
                nbt_type: *t,
                values: prop_values
                    .get(*name)
                    .expect("value set")
                    .iter()
                    .map(|(tag, repr)| match tag {
                        'b' => PropValue::Byte(repr.parse::<i8>().unwrap_or(0)),
                        'i' => PropValue::Int(repr.parse::<i32>().unwrap_or(0)),
                        _ => PropValue::String(repr.clone()),
                    })
                    .collect(),
            })
            .collect();
        properties.sort_by(|a, b| a.name.cmp(&b.name));

        // ---- Canonical state order to protocol runtime id (taken from source, never recomputed) ----
        let state_count = canonical_state_count(&properties);
        let mut by_assignment: BTreeMap<Vec<String>, u32> = BTreeMap::new();
        for (assignment, network_id) in source_props.iter().zip(source_network.iter()) {
            let mut key: Vec<String> = assignment
                .iter()
                .map(|(k, v)| format!("{k}={}", v.repr()))
                .collect();
            key.sort();
            if by_assignment.insert(key, *network_id).is_some() {
                return Err(SplitError::invalid(format!(
                    "{identifier} 的源 states 出现重复属性组合"
                )));
            }
        }
        if by_assignment.len() != state_count {
            return Err(SplitError::invalid(format!(
                "{identifier} 源状态组合非完整笛卡尔积（{} 个组合 vs 笛卡尔积 {state_count}），不猜测缺失状态",
                by_assignment.len()
            )));
        }
        let mut network_ids = Vec::with_capacity(state_count);
        for index in 0..state_count {
            let props = canonical_state_props(&properties, index);
            let mut key: Vec<String> = props
                .iter()
                .map(|(k, v)| format!("{k}={}", v.repr()))
                .collect();
            key.sort();
            let network_id = *by_assignment.get(&key).ok_or_else(|| {
                SplitError::invalid(format!(
                    "{identifier} 规范状态 {} 在源 palette 中缺失",
                    canonical_state_key(&props)
                ))
            })?;
            network_ids.push(network_id);
        }

        // ---- Default state (explicit) ----
        let prop_count = properties.len();
        let mut keys = Vec::with_capacity(states.len());
        for (i, assignment) in source_props.iter().enumerate() {
            let single: Option<(String, String)> = if prop_count == 1 {
                assignment.iter().next().map(|(k, v)| (k.clone(), v.repr()))
            } else {
                None
            };
            keys.push(split_state_key(prop_count, single, i));
        }
        let default_source_index: usize = if states.len() == 1 {
            0
        } else if let Some(d) = defaults.get(*identifier) {
            keys.iter().position(|k| k == d).ok_or_else(|| {
                SplitError::invalid(format!(
                    "{identifier} 的默认态 {d:?} 不在拆分状态键内（键：{}）",
                    keys.join(", ")
                ))
            })?
        } else {
            missing_defaults.push(identifier.to_string());
            continue;
        };
        let default_state = canonical_state_props(
            &properties,
            default_state_index_of(&properties, &source_props[default_source_index], identifier)?,
        );

        let (ns, sub) = validate_identifier(identifier, max_identifier_len)
            .map_err(SplitError::invalid)?;
        let zip_path = format!("{BLOCKS_DIRECTORY}/{ns}/{sub}.json");

        // ---- description.states (value order = declaration order) ----
        let mut states_json = serde_json::Map::new();
        for prop in properties.iter() {
            let mut arr = Vec::with_capacity(prop.values.len());
            for value in prop.values.iter() {
                arr.push(value.to_json());
            }
            states_json.insert(prop.name.clone(), Value::Array(arr));
        }
        let mut description = serde_json::Map::new();
        description.insert(
            "identifier".to_string(),
            Value::String(identifier.to_string()),
        );
        description.insert("states".to_string(), Value::Object(states_json));

        // ---- sc:default_state ----
        let mut default_json = serde_json::Map::new();
        for (name, value) in default_state.iter() {
            default_json.insert(name.clone(), value.to_json());
        }

        // ---- components (authoritative declarations only; hardness semantics see BlockHardness) ----
        let mut components_json = serde_json::Map::new();
        if let Some(h) = hardness.get(*identifier) {
            match h {
                BlockHardness::Breakable(hardness) => {
                    let mut mining = serde_json::Map::new();
                    let value = serde_json::Number::from_f64(*hardness as f64)
                        .ok_or_else(|| SplitError::invalid(format!("{identifier} 硬度非有限值")))?;
                    mining.insert("value".to_string(), Value::Number(value));
                    components_json.insert(
                        "minecraft:destructible_by_mining".to_string(),
                        Value::Object(mining),
                    );
                }
                BlockHardness::Unbreakable => {
                    components_json.insert(
                        "sc:unbreakable".to_string(),
                        Value::Object(Default::default()),
                    );
                }
            }
        }

        let mut block = serde_json::Map::new();
        block.insert("description".to_string(), Value::Object(description));
        block.insert("components".to_string(), Value::Object(components_json));
        block.insert("permutations".to_string(), Value::Array(Vec::new()));
        block.insert("sc:default_state".to_string(), Value::Object(default_json));
        block.insert(
            "sc:protocol_runtime_ids".to_string(),
            Value::Array(
                network_ids
                    .iter()
                    .map(|id| Value::Number((*id as u64).into()))
                    .collect(),
            ),
        );

        let mut root = serde_json::Map::new();
        root.insert(
            "format_version".to_string(),
            Value::String(BLOCK_JSON_FORMAT_VERSION.to_string()),
        );
        root.insert("minecraft:block".to_string(), Value::Object(block));
        let bytes = serde_json::to_vec_pretty(&Value::Object(root))
            .map_err(|e| SplitError::invalid(format!("JSON 序列化失败：{e}")))?;
        out.push(GeneratedBlockFile { zip_path, bytes });
    }
    if !missing_defaults.is_empty() {
        missing_defaults.sort();
        return Err(SplitError::missing_defaults(missing_defaults));
    }
    // Self-check: generated files must load (catches splitter bugs).
    for file in out.iter() {
        parse_block_file("<split>", &file.zip_path, &file.bytes)
            .map_err(|e| SplitError::invalid(format!("拆分自检失败 {}：{e}", file.zip_path)))?;
    }
    Ok(out)
}

/// Source property assignment to canonical index (fails when a value is outside the declared set).
pub(super) fn default_state_index_of(
    properties: &[PropertyDef],
    assignment: &BTreeMap<String, PropValue>,
    identifier: &str,
) -> Result<usize, SplitError> {
    canonical_state_index(properties, assignment).ok_or_else(|| {
        SplitError::invalid(format!(
            "{identifier} 的默认态属性组合不在声明值域内（不猜测）"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::component::{DestructibleByMining, LightDampening};
    use crate::macros::ComponentKey;
    use crate::block::{BlockComponents, BlockDataManifest, BLOCKS_DIRECTORY};
    use crate::loot_tables::LootTables;
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
                    "minecraft:destructible_by_mining": {"value": 2.0},
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
        }"#
        .as_bytes()
        .to_vec()
    }

    fn log_path() -> &'static str {
        "blocks/minecraft/oak_log.json"
    }

    fn air_path() -> &'static str {
        "blocks/minecraft/air.json"
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
    fn manifest_validation_rejects_unknown_schema_and_mode() {
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
                directory: BLOCKS_DIRECTORY.to_string(),
                network_id_mode: "ordered".to_string(),
            },
        ] {
            bad.validate("pack").expect_err("非法声明必须拒绝");
        }
    }

    #[test]
    fn discovery_sorts_and_parses_with_file_paths() {
        // Inserted in reverse on purpose: scan results must sort by canonical path.
        let air = air_json();
        let log = log_json();
        let mut behavior_packs = vec![make_source(&[
            (log_path(), &log),
            (air_path(), &air),
            ("other/notes.txt", b"ignore me"),
        ])];
        let bundle = load_block_json_bundle(&mut behavior_packs, &test_manifest(), PACK)
            .expect("发现应成功")
            .expect("bundle 应存在");
        assert_eq!(bundle.files.len(), 2);
        assert_eq!(bundle.files[0].identifier, "minecraft:air");
        assert_eq!(bundle.files[1].identifier, "minecraft:oak_log");
        assert_eq!(bundle.network_id_mode, "hashed");
        assert_ne!(bundle.fingerprint, 0);
    }

    /// `minecraft:loot` references resolve at bundle load against the same
    /// behavior pack: the parsed `LootTables` component is inserted into the
    /// referencing component layer.
    #[test]
    fn bundle_resolves_loot_references_into_components() {
        let table = br#"{"pools": [{"rolls": 1, "entries": [{"type": "item", "name": "minecraft:stick"}]}]}"#;
        let air = air_json();
        let log = log_json();
        let mut behavior_packs = vec![make_source(&[
            (log_path(), &log),
            (air_path(), &air),
            ("loot_tables/blocks/oak_log.json", table),
        ])];
        let bundle = load_block_json_bundle(&mut behavior_packs, &test_manifest(), PACK)
            .expect("发现应成功")
            .expect("bundle 应存在");
        let log = bundle
            .files
            .iter()
            .find(|f| f.identifier == "minecraft:oak_log")
            .expect("oak_log");
        let perm = &log.permutations[0];
        // Parsed table attached to the permutation layer.
        let table_component = perm.components.get(LootTables::NAME).expect("应插入 LootTables");
        assert_eq!(table_component["pools"][0]["rolls"], 1);
        // Raw vanilla reference stays untouched.
        assert_eq!(
            perm.components.get("minecraft:loot"),
            Some(&serde_json::json!("loot_tables/blocks/oak_log.json"))
        );
        // Unreferenced files gain nothing.
        let air = bundle
            .files
            .iter()
            .find(|f| f.identifier == "minecraft:air")
            .expect("air");
        assert!(!air.base.contains_key(LootTables::NAME));
        // Typed reads still work on the merged (base) layer.
        let _ = BlockComponents::get_from::<DestructibleByMining>(&log.base);
        let _ = BlockComponents::get_from::<LightDampening>(&log.base);
    }

    #[test]
    fn discovery_fails_loud_on_empty_and_broken() {
        // Zero files.
        let mut behavior_packs = vec![make_source(&[("manifest.json", b"{}")])];
        load_block_json_bundle(&mut behavior_packs, &test_manifest(), PACK)
            .expect_err("零文件必须失败");
        // Broken JSON: never skipped, must fail with the file path.
        let mut behavior_packs = vec![make_source(&[(air_path(), b"{oops")])];
        let e = load_block_json_bundle(&mut behavior_packs, &test_manifest(), PACK)
            .expect_err("坏文件必须失败");
        assert!(e.file.contains("air.json"), "实际：{e}");
    }

    /// Split-to-reparse round-trip: multi-property cartesian product + protocol id alignment + default-state placement.
    #[test]
    fn split_round_trip_preserves_ids_and_default() {
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
        let err = split_legacy_palette(&entries, &HashMap::new(), &HashMap::new())
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
            let mut seen = std::collections::HashSet::new();
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
        let files = split_legacy_palette(&unique, &defaults, &hardness).expect("拆分应成功");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].zip_path, "blocks/test/mixed.json");
        let parsed = parse_block_file("<test>", &files[0].zip_path, &files[0].bytes)
            .expect("重解析应成功");
        // Canonical order: age(0,1) x lit(false,true), rightmost lit varies fastest.
        assert_eq!(parsed.state_count(), 4);
        assert_eq!(parsed.network_ids, vec![8, 9, 10, 12]);
        assert_eq!(parsed.default_index, 3);
        assert_eq!(parsed.state_key(3), "age=1,lit=1");
        assert_eq!(
            BlockComponents::get_from::<DestructibleByMining>(&parsed.base).map(|d| d.value),
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
        let err = split_legacy_palette(&sparse, &sparse_defaults, &HashMap::new())
            .expect_err("非完整笛卡尔积必须失败");
        assert!(err.to_string().contains("笛卡尔积"), "实际：{err}");

        // Multi-state block without a default state goes to the pending list (batch return, never the first state).
        let err = split_legacy_palette(&unique, &HashMap::new(), &HashMap::new())
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
        let palette_bytes = source
            .read_file("definitions/block_palette.nbt")
            .expect("palette 缺失");
        let entries = parse_legacy_palette_entries(&palette_bytes).expect("旧 palette 解析");
        // Stats: type/state counts, property-type distribution.
        let mut by_name: HashMap<&str, usize> = HashMap::new();
        let mut prop_tags: HashMap<u8, usize> = HashMap::new();
        for e in entries.iter() {
            *by_name.entry(e.name.as_str()).or_default() += 1;
            if let Some(states) = e.states.as_ref() {
                for (_, v) in states.iter() {
                    *prop_tags.entry(v.tag()).or_default() += 1;
                }
            }
        }
        let single: Vec<&str> = by_name
            .iter()
            .filter(|(_, c)| **c == 1)
            .map(|(k, _)| *k)
            .collect();
        let max_per_type = by_name.values().copied().max().unwrap_or(0);
        eprintln!(
            "[real-pack] entries={} types={} single_state_types={} max_states_per_type={} prop_tags={:?}",
            entries.len(),
            by_name.len(),
            single.len(),
            max_per_type,
            prop_tags,
        );
        assert!(entries.len() > 1000, "真实 palette 应有规模");
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
        let files =
            split_legacy_palette(&subset_entries, &HashMap::new(), &HashMap::new())
                .expect("单状态子集拆分应成功");
        assert_eq!(files.len(), subset.len());
        for f in files.iter() {
            assert!(
                f.zip_path.starts_with("blocks/"),
                "{}",
                f.zip_path
            );
        }
    }
}
