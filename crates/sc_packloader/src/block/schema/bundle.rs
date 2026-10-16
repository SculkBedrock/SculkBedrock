//! Bundle discovery/load, fingerprinting, and legacy-palette splitting.
//!
//! Split from `schema.rs`; re-exported at `crate::block::schema` and `crate::block`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io::Read;

use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use serde_json::Value;

use crate::version_control::json_budget::BudgetReader;
use super::{
    parse::{
        canonical_state_count, canonical_state_index, canonical_state_key,
        canonical_state_props, parse_block_file, NbtPropType, ParsedBlockFile, PropertyDef,
        PropValue,
    },
    validate::{validate_block_path, validate_identifier},
    BlockBundleBudgets, BlockDataManifest, BlockJsonError, BLOCK_BUDGETS_VERSION,
    BLOCK_JSON_FORMAT_VERSION, BLOCK_JSON_SCHEMA_VERSION, BLOCKS_DIRECTORY,
};
use sc_error::{ScError, ScSite, capture_site};

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
/// Mining/drop rules are already in the file bytes, so content changes alter the fingerprint; budget version
/// and values mix in separately (same content with different budgets means different identity, see `BLOCK_BUDGETS_VERSION`).
pub fn fingerprint_bundle(files_sorted: &[(&str, &[u8])], network_id_mode: &str) -> u64 {
    fingerprint_bundle_with_budgets(
        files_sorted,
        network_id_mode,
        &BlockBundleBudgets::default(),
    )
}

/// Content fingerprint with budget identity (what `load_block_json_bundle` actually uses).
pub fn fingerprint_bundle_with_budgets(
    files_sorted: &[(&str, &[u8])],
    network_id_mode: &str,
    budgets: &BlockBundleBudgets,
) -> u64 {
    let mut seed = Vec::new();
    seed.extend_from_slice(network_id_mode.as_bytes());
    seed.push(0);
    seed.extend_from_slice(&BLOCK_JSON_SCHEMA_VERSION.to_le_bytes());
    seed.extend_from_slice(&BLOCK_BUDGETS_VERSION.to_le_bytes());
    for v in [
        budgets.max_mining_tools_per_state,
        budgets.max_mining_items_per_rule,
        budgets.max_mining_enchants_per_state,
        budgets.max_drop_entries_per_state,
        budgets.max_count_options_per_entry,
        budgets.max_fortune_rules_per_entry,
        budgets.max_requires_items_per_entry,
    ] {
        seed.extend_from_slice(&(v as u64).to_le_bytes());
    }
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
// Bundle: discovery, bounded reads, sorting, parsing
// ---------------------------------------------------------------------------

/// Parsed version-pack block bundle (input to sc_block compilation).
#[derive(Clone, Debug)]
pub struct BlockJsonBundle {
    pub schema_version: u32,
    pub network_id_mode: String,
    pub fingerprint: u64,
    pub files: Vec<ParsedBlockFile>,
}

pub(super) fn read_capped<T: Read>(
    file: T,
    cap: usize,
    pack_id: &str,
    path: &str,
) -> Result<Vec<u8>, BlockJsonError> {
    let mut buf = Vec::new();
    file.take((cap as u64) + 1)
        .read_to_end(&mut buf)
        .map_err(|e| BlockJsonError::new(pack_id, path, "$", format!("文件读取失败: {e}")))?;
    if buf.len() > cap {
        return Err(BlockJsonError::new(
            pack_id,
            path,
            "$",
            format!("单文件 {} 字节超出预算 {cap}", buf.len()),
        ));
    }
    Ok(buf)
}

/// Discover and parse the `.block.json` bundle from a version-pack zip.
///
/// - Without a manifest `block_data` declaration, return `Ok(None)` (legacy palette path);
/// - Once declared: zero files, illegal paths, budget overruns, and JSON syntax/schema errors all return Err;
///   callers must not fall back to the legacy format.
pub fn load_block_json_bundle(
    pack: &mut crate::pack_loader::pack::zipped::PackSource,
    manifest: &BlockDataManifest,
    pack_id: &str,
    budgets: &BlockBundleBudgets,
) -> Result<Option<BlockJsonBundle>, BlockJsonError> {
    manifest.validate(pack_id)?;
    let prefix = format!("{BLOCKS_DIRECTORY}/");
    // Collect candidate entries (case-sensitive prefix; the directory itself is skipped).
    let mut candidates: Vec<(String, u64)> = Vec::new();
    for name in pack.file_names() {
        if !name.starts_with(&prefix) || !name.ends_with(".block.json") {
            continue;
        }
        let size = pack.file_size(&name).ok_or_else(|| {
            BlockJsonError::new(pack_id, "<bundle>", "$", "pack 索引失败".to_string())
        })?;
        candidates.push((name, size));
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    if candidates.is_empty() {
        return Err(BlockJsonError::new(
            pack_id,
            "<bundle>",
            "$",
            "manifest 声明了 block_data，但版本包内未发现任何 definitions/blocks/**/*.block.json",
        ));
    }
    if candidates.len() > budgets.max_files {
        return Err(BlockJsonError::new(
            pack_id,
            "<bundle>",
            "$",
            format!(
                "方块文件数 {} 超出预算 {}",
                candidates.len(),
                budgets.max_files
            ),
        ));
    }
    // Normalized duplicate-entry check.
    {
        let mut seen = BTreeSet::new();
        for (name, _) in candidates.iter() {
            if !seen.insert(name.clone()) {
                return Err(BlockJsonError::new(
                    pack_id,
                    name,
                    "$",
                    "规范化后重复的 ZIP entry",
                ));
            }
        }
    }
    let mut total_bytes: u64 = 0;
    let mut raws: Vec<(String, Vec<u8>)> = Vec::with_capacity(candidates.len());
    for (name, size) in candidates.iter() {
        if *size > budgets.max_file_bytes as u64 {
            return Err(BlockJsonError::new(
                pack_id,
                name,
                "$",
                format!("单文件声明 {size} 字节超出预算 {}", budgets.max_file_bytes),
            ));
        }
        total_bytes = total_bytes.saturating_add(*size);
        if total_bytes > budgets.max_total_bytes {
            return Err(BlockJsonError::new(
                pack_id,
                "<bundle>",
                "$",
                format!("方块定义总字节超限（>{}）", budgets.max_total_bytes),
            ));
        }
        validate_block_path(name, budgets.max_identifier_len)
            .map_err(|m| BlockJsonError::new(pack_id, name, "$", format!("文件路径非法: {m}")))?;
        let bytes = pack
            .read_file(name)
            .ok_or_else(|| BlockJsonError::new(pack_id, name, "$", "pack 读取失败".to_string()))?;
        if bytes.len() as u64 > budgets.max_file_bytes as u64 {
            return Err(BlockJsonError::new(
                pack_id,
                name,
                "$",
                format!(
                    "单文件 {} 字节超出预算 {}",
                    bytes.len(),
                    budgets.max_file_bytes
                ),
            ));
        }
        let bytes = read_capped(&bytes[..], budgets.max_file_bytes, pack_id, name)?;
        // JSON syntax + generic budget pre-check (depth/strings/containers), rejected before full allocation.
        serde_json::from_reader::<_, Value>(BudgetReader::new(&bytes[..])).map_err(|e| {
            BlockJsonError::new(pack_id, name, "$", format!("JSON 语法/预算失败: {e}"))
        })?;
        raws.push((name.clone(), bytes));
    }
    // Re-check actual total bytes (never trust ZIP-declared sizes).
    let actual_total: u64 = raws.iter().map(|(_, b)| b.len() as u64).sum();
    if actual_total > budgets.max_total_bytes {
        return Err(BlockJsonError::new(
            pack_id,
            "<bundle>",
            "$",
            format!(
                "方块定义实际总字节 {actual_total} 超出预算 {}",
                budgets.max_total_bytes
            ),
        ));
    }
    let sorted_refs: Vec<(&str, &[u8])> = raws
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    let fingerprint =
        fingerprint_bundle_with_budgets(&sorted_refs, &manifest.network_id_mode, budgets);
    let mut files = Vec::with_capacity(raws.len());
    for (name, bytes) in raws.iter() {
        files.push(parse_block_file(pack_id, name, bytes, budgets)?);
    }
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
// Legacy palette to .block.json splitting (core shared by the extractor and migration tests)
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

/// Split legacy palette entries by identifier into vanilla-structure `.block.json` files.
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
    budgets: &BlockBundleBudgets,
) -> Result<Vec<GeneratedBlockFile>, SplitError> {
    let mut grouped: BTreeMap<&str, Vec<&LegacyPaletteEntry>> = BTreeMap::new();
    for entry in entries {
        if entry.name.is_empty() {
            continue;
        }
        validate_identifier(&entry.name, budgets.max_identifier_len).map_err(|m| {
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
        if state_count > budgets.max_states_per_block {
            return Err(SplitError::invalid(format!(
                "{identifier} 状态组合数 {state_count} 超出单方块预算 {}",
                budgets.max_states_per_block
            )));
        }
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

        let (ns, sub) = validate_identifier(identifier, budgets.max_identifier_len)
            .map_err(SplitError::invalid)?;
        let zip_path = format!("{BLOCKS_DIRECTORY}/{ns}/{sub}.block.json");

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
        parse_block_file("<split>", &file.zip_path, &file.bytes, budgets)
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
