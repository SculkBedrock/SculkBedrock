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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::io::{Read, Seek};

use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::version_control::json_budget::BudgetReader;

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
        }
    }
}

/// Budget version (part of the fingerprint input; default changes alter the fingerprint).
pub const BLOCK_BUDGETS_VERSION: u32 = 3;

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

// ---------------------------------------------------------------------------
// Identifier/path contract
// ---------------------------------------------------------------------------

fn is_ns_char(b: u8) -> bool {
    matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-')
}

/// Validate `namespace:path`. Returns (namespace, path), or the reason on failure.
pub fn validate_identifier(id: &str, max_len: usize) -> Result<(String, String), String> {
    if id.is_empty() || id.len() > max_len {
        return Err(format!("identifier 长度非法（1..={max_len}）"));
    }
    let mut parts = id.splitn(2, ':');
    let ns = parts.next().unwrap_or("");
    let path = parts
        .next()
        .ok_or_else(|| "identifier 缺少 ':'".to_string())?;
    if ns.is_empty() || !ns.bytes().all(is_ns_char) {
        return Err(format!("namespace 非法: {ns:?}（只允许 [a-z0-9_.-]）"));
    }
    if path.is_empty() {
        return Err("path 为空".to_string());
    }
    let mut segments = 0usize;
    for seg in path.split('/') {
        segments += 1;
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(format!("path 含非法段: {seg:?}"));
        }
        if !seg.bytes().all(is_ns_char) {
            return Err(format!("path 段字符非法: {seg:?}（只允许 [a-z0-9_.-]）"));
        }
    }
    if segments == 0 {
        return Err("path 为空".to_string());
    }
    Ok((ns.to_string(), path.to_string()))
}

/// Validate a block file path inside the zip and map it back to an identifier.
///
/// Requires `definitions/blocks/<namespace>/<path>.block.json`, all-lowercase ASCII;
/// rejects backslashes, empty segments, and `.`/`..`. Returns the identifier.
pub fn validate_block_path(path: &str, max_len: usize) -> Result<String, String> {
    if !path.is_ascii() || path != path.to_ascii_lowercase() {
        return Err("路径必须为全小写 ASCII".to_string());
    }
    if path.contains('\\') {
        return Err("路径不允许反斜线".to_string());
    }
    let prefix = format!("{BLOCKS_DIRECTORY}/");
    let Some(rest) = path.strip_prefix(&prefix) else {
        return Err(format!("路径必须以 {prefix:?} 开头"));
    };
    let Some(rest) = rest.strip_suffix(".block.json") else {
        return Err("路径必须以 .block.json 结尾".to_string());
    };
    if rest.is_empty() {
        return Err("路径缺少方块相对路径".to_string());
    }
    let mut segments = rest.split('/');
    let ns = segments.next().unwrap_or("");
    let sub: Vec<&str> = segments.collect();
    if ns.is_empty() || sub.is_empty() {
        return Err("路径缺少 namespace 或 path".to_string());
    }
    let identifier = format!("{ns}:{}", sub.join("/"));
    validate_identifier(&identifier, max_len)
        .map(|_| identifier)
        .map_err(|e| format!("路径映射的 identifier 非法: {e}"))
}

// ---------------------------------------------------------------------------
// Duplicate JSON key scan (serde_json lets later keys win, so duplicates must be rejected up front)
// ---------------------------------------------------------------------------

/// Scan JSON text for duplicate object keys. Tracks precisely when the input is valid JSON;
/// invalid JSON is reported by later serde parsing; this function only best-effort scans (returns Ok and defers to serde).
pub fn reject_duplicate_keys(bytes: &[u8]) -> Result<(), String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Frame {
        Obj { need_key: bool },
        Arr,
    }
    const MAX_DEPTH: usize = 128;
    let mut stack: Vec<Frame> = Vec::new();
    let mut keys: Vec<HashSet<Vec<u8>>> = Vec::new();
    let mut i = 0usize;
    let n = bytes.len();
    let parse_string = |i: &mut usize| -> Result<Vec<u8>, ()> {
        // bytes[*i] == b'"'
        *i += 1;
        let mut out = Vec::new();
        while *i < n {
            let b = bytes[*i];
            if b == b'"' {
                *i += 1;
                return Ok(out);
            }
            if b == b'\\' {
                *i += 1;
                if *i >= n {
                    return Err(());
                }
                let e = bytes[*i];
                match e {
                    b'u' => {
                        // \uXXXX: mix decoded chars with raw escape bytes so equivalent escapes compare equal.
                        if *i + 4 >= n {
                            return Err(());
                        }
                        let hex = &bytes[*i + 1..*i + 5];
                        let cp = std::str::from_utf8(hex)
                            .ok()
                            .and_then(|s| u32::from_str_radix(s, 16).ok())
                            .ok_or(())?;
                        let ch = char::from_u32(cp).ok_or(())?;
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        out.push(0xFF);
                        out.extend_from_slice(hex);
                        *i += 5;
                    }
                    b'"' => {
                        out.push(b'"');
                        *i += 1;
                    }
                    b'\\' => {
                        out.push(b'\\');
                        *i += 1;
                    }
                    b'n' => {
                        out.push(b'\n');
                        *i += 1;
                    }
                    b't' => {
                        out.push(b'\t');
                        *i += 1;
                    }
                    b'r' => {
                        out.push(b'\r');
                        *i += 1;
                    }
                    b'b' => {
                        out.push(0x08);
                        *i += 1;
                    }
                    b'f' => {
                        out.push(0x0C);
                        *i += 1;
                    }
                    b'/' => {
                        out.push(b'/');
                        *i += 1;
                    }
                    _ => return Err(()),
                }
                continue;
            }
            out.push(b);
            *i += 1;
        }
        Err(())
    };
    while i < n {
        let b = bytes[i];
        match b {
            b'"' => {
                let s = parse_string(&mut i).map_err(|_| "JSON 字符串未闭合".to_string())?;
                let is_key_position = matches!(stack.last(), Some(Frame::Obj { need_key: true }));
                if is_key_position {
                    let depth = stack.len() - 1;
                    if !keys[depth].insert(s) {
                        return Err("JSON 对象存在重复 key".to_string());
                    }
                    if let Some(Frame::Obj { need_key }) = stack.last_mut() {
                        *need_key = false;
                    }
                }
            }
            b'{' => {
                if stack.len() >= MAX_DEPTH {
                    return Err("JSON 嵌套过深".to_string());
                }
                stack.push(Frame::Obj { need_key: true });
                keys.push(HashSet::new());
                i += 1;
            }
            b'[' => {
                if stack.len() >= MAX_DEPTH {
                    return Err("JSON 嵌套过深".to_string());
                }
                stack.push(Frame::Arr);
                keys.push(HashSet::new());
                i += 1;
            }
            b'}' | b']' => {
                stack.pop();
                keys.pop();
                i += 1;
            }
            b',' => {
                if let Some(Frame::Obj { need_key }) = stack.last_mut() {
                    *need_key = true;
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property types and values (byte/int/string stay distinct, never unified)
// ---------------------------------------------------------------------------

/// NBT type of a block property. `byte(1)` and `int(1)` are different state identities, never unified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NbtPropType {
    Byte,
    Int,
    String,
}

impl NbtPropType {
    pub fn tag(self) -> char {
        match self {
            NbtPropType::Byte => 'b',
            NbtPropType::Int => 'i',
            NbtPropType::String => 's',
        }
    }
}

/// Typed property value (parse time).
#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    Byte(i8),
    Int(i32),
    String(String),
}

impl PropValue {
    pub fn prop_type(&self) -> NbtPropType {
        match self {
            PropValue::Byte(_) => NbtPropType::Byte,
            PropValue::Int(_) => NbtPropType::Int,
            PropValue::String(_) => NbtPropType::String,
        }
    }

    /// Value text (byte/int decimal, string raw value).
    pub fn repr(&self) -> String {
        match self {
            PropValue::Byte(v) => v.to_string(),
            PropValue::Int(v) => v.to_string(),
            PropValue::String(v) => v.clone(),
        }
    }

    /// Typed equality token (`byte(0)` differs from `int(0)`).
    pub fn token(&self) -> String {
        format!("{}:{}", self.prop_type().tag(), self.repr())
    }

    pub fn to_nbt(&self) -> NbtValue {
        match self {
            PropValue::Byte(v) => NbtValue::Byte(*v),
            PropValue::Int(v) => NbtValue::Int(*v),
            PropValue::String(v) => NbtValue::String(v.clone()),
        }
    }

    /// JSON mapping: `byte` to bool (Bedrock JSON has no i8; observed source byte values cover
    /// only `{0,1}`; non-0/1 byte state values are loudly rejected at split time).
    pub fn to_json(&self) -> Value {
        match self {
            PropValue::Byte(v) => Value::Bool(*v != 0),
            PropValue::Int(v) => Value::Number((*v as i64).into()),
            PropValue::String(v) => Value::String(v.clone()),
        }
    }

    /// JSON to typed value: bool to byte, integer to int, string to string;
    /// floats/compounds/out-of-range integers all map to `None` (callers fail loudly, no lenient conversion).
    pub fn from_json(v: &Value) -> Option<PropValue> {
        match v {
            Value::Bool(b) => Some(PropValue::Byte(if *b { 1 } else { 0 })),
            Value::Number(n) => n
                .as_i64()
                .filter(|i| *i >= i32::MIN as i64 && *i <= i32::MAX as i64)
                .map(|i| PropValue::Int(i as i32)),
            Value::String(s) => Some(PropValue::String(s.clone())),
            _ => None,
        }
    }

    /// Property value to canonical-JSON molang literal text (byte to `true`/`false`).
    pub fn molang_literal(&self) -> String {
        match self {
            PropValue::Byte(v) => {
                if *v != 0 {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            PropValue::Int(v) => v.to_string(),
            PropValue::String(v) => format!("'{v}'"),
        }
    }

    /// Molang literal text to property value (inverse of [`PropValue::molang_literal`]).
    pub fn from_molang_literal(src: &str) -> Option<PropValue> {
        if src == "true" {
            return Some(PropValue::Byte(1));
        }
        if src == "false" {
            return Some(PropValue::Byte(0));
        }
        if let Some(inner) = src.strip_prefix('\'') {
            return inner
                .strip_suffix('\'')
                .map(|v| PropValue::String(v.to_string()));
        }
        src.parse::<i32>().ok().map(PropValue::Int)
    }
}

/// Declaration of one property (one entry of `description.states`).
#[derive(Clone, Debug)]
pub struct PropertyDef {
    pub name: String,
    pub nbt_type: NbtPropType,
    /// Declaration order is value order (canonical state enumeration takes values in this order, never re-sorted).
    pub values: Vec<PropValue>,
}

impl PropertyDef {
    pub fn value_index(&self, value: &PropValue) -> Option<usize> {
        self.values.iter().position(|v| v == value)
    }
}

// ---------------------------------------------------------------------------
// Canonical state enumeration (one deterministic order shared by parsing and compilation)
// ---------------------------------------------------------------------------

/// Canonical state count (a block without properties has 1 empty state).
pub fn canonical_state_count(properties: &[PropertyDef]) -> usize {
    properties.iter().map(|p| p.values.len()).product()
}

/// Stride of property `slot` in canonical order (rightmost property has stride 1).
fn canonical_stride(properties: &[PropertyDef], slot: usize) -> usize {
    properties[slot + 1..]
        .iter()
        .map(|p| p.values.len())
        .product::<usize>()
        .max(1)
}

/// Canonical state enumeration (deterministic rules):
///
/// 1. Properties sorted by name ascending (`PropertyDef` is already sorted);
/// 2. Property values in `description.states` declaration order (never re-sorted);
/// 3. Row-major order, rightmost property varies fastest (`u = sum idx_i * stride_i`).
///
/// The rule is fixed here; index math at parse time shares the `sc_block` state enumeration, no duplicate implementations.
pub fn canonical_state_props(properties: &[PropertyDef], index: usize) -> Vec<(String, PropValue)> {
    if properties.is_empty() {
        return Vec::new();
    }
    debug_assert!(index < canonical_state_count(properties));
    let mut rest = index;
    let mut out: Vec<(String, PropValue)> = properties
        .iter()
        .map(|p| (p.name.clone(), p.values[0].clone()))
        .collect();
    for slot in (0..properties.len()).rev() {
        let n = properties[slot].values.len();
        let idx = rest % n;
        rest /= n;
        out[slot] = (
            properties[slot].name.clone(),
            properties[slot].values[idx].clone(),
        );
    }
    out
}

/// Property assignment to canonical index (`None` when the set mismatches or a value is undeclared).
pub fn canonical_state_index(
    properties: &[PropertyDef],
    assignment: &BTreeMap<String, PropValue>,
) -> Option<usize> {
    if properties.is_empty() {
        return assignment.is_empty().then_some(0);
    }
    if assignment.len() != properties.len() {
        return None;
    }
    let mut index = 0usize;
    for (slot, prop) in properties.iter().enumerate() {
        let value = assignment.get(&prop.name)?;
        let idx = prop.value_index(value)?;
        index += idx * canonical_stride(properties, slot);
    }
    Some(index)
}

/// Human-readable canonical state key (`default` without properties, else `name=repr` by ascending
/// property name joined by commas). Property names are in the key, so same-valued byte/int entries stay distinct; same
/// origin as the [`split_state_key`] `{prop}_{repr}` rule.
pub fn canonical_state_key(props: &[(String, PropValue)]) -> String {
    if props.is_empty() {
        return "default".to_string();
    }
    let mut parts: Vec<String> = props
        .iter()
        .map(|(k, v)| format!("{k}={}", v.repr()))
        .collect();
    parts.sort();
    parts.join(",")
}

// ---------------------------------------------------------------------------
// Component maps and parse-time helpers
// ---------------------------------------------------------------------------

fn err(
    pack: &str,
    file: &str,
    field: impl Into<String>,
    message: impl Into<String>,
) -> BlockJsonError {
    BlockJsonError::new(pack, file, field.into(), message.into())
}

fn as_object<'a>(
    pack: &str,
    file: &str,
    field: &str,
    v: &'a Value,
) -> Result<&'a serde_json::Map<String, Value>, BlockJsonError> {
    v.as_object()
        .ok_or_else(|| err(pack, file, field, "应为 JSON 对象"))
}

fn check_no_unknown_keys(
    pack: &str,
    file: &str,
    field: &str,
    obj: &serde_json::Map<String, Value>,
    allowed: &[&str],
) -> Result<(), BlockJsonError> {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(err(pack, file, field, format!("未知字段: {key:?}")));
        }
    }
    Ok(())
}

/// Validate the component map, returning validated raw JSON (name to value).
///
/// - Fixed components go through the [`crate::block::component`] registry (shape + ranges);
/// - `IGNORED_COMPONENTS` (vanilla client-side entries) are shape-checked then dropped, never entering snapshots;
/// - Other namespaced components keep their raw text (under the byte budget); plugins register schema checks;
/// - Missing namespaces are rejected; any `ur:*` is rejected with no compatibility handling;
/// - `minecraft:destructible_by_mining` and `sc:unbreakable` are mutually exclusive (rejected within one layer;
///   cross-layer merge conflicts are re-checked by `sc_block` on the final state);
/// - `sc:mining`/`sc:drops` get an extra budget check (entry/element counts, identifier lengths, component bytes).
fn validate_components_map(
    pack: &str,
    file: &str,
    field: &str,
    v: &Value,
    budgets: &BlockBundleBudgets,
) -> Result<BTreeMap<String, Value>, BlockJsonError> {
    use crate::block::component::{
        is_ignored_component, is_known_component, validate_block_component, Drops, Mining,
    };
    let obj = as_object(pack, file, field, v)?;
    let mut out = BTreeMap::new();
    for (name, value) in obj.iter() {
        let item_field = format!("{field}.{name}");
        // Canonical names (including the new sc:mining/sc:drops).
        if is_known_component(name) {
            validate_block_component(pack, file, &item_field, name, value)?;
            check_component_budgets(pack, file, &item_field, name, value, budgets)?;
            if out.contains_key(name) {
                return Err(err(pack, file, &item_field, format!("组件重复：{name:?}")));
            }
            out.insert(name.clone(), value.clone());
            continue;
        }
        if is_ignored_component(name) {
            // Legal vanilla file content (shapes vary: string/object/array): volume-bounded only,
            // dropped raw, no semantic interpretation, never enters snapshots.
            let bytes = value.to_string().len();
            if bytes > budgets.max_component_bytes {
                return Err(err(
                    pack,
                    file,
                    &item_field,
                    format!(
                        "忽略名单组件 {name} 过大（{bytes} > {} 字节）",
                        budgets.max_component_bytes
                    ),
                ));
            }
            continue;
        }
        // Only `sc:*` is accepted; no compatibility handling.
        if name.starts_with("ur:") {
            return Err(err(
                pack,
                file,
                &item_field,
                format!(
                    "未知组件 {name}（`ur:*` 已整体迁移为 `sc:*`，不做兼容；请改为 `sc:*` 规范名）"
                ),
            ));
        }
        if name.starts_with("sc:") {
            return Err(err(
                pack,
                file,
                &item_field,
                format!("未知 sc 组件: {name}"),
            ));
        }
        if name.contains(':') {
            let bytes = value.to_string().len();
            if bytes > budgets.max_component_bytes {
                return Err(err(
                    pack,
                    file,
                    &item_field,
                    format!(
                        "插件组件过大（{bytes} > {} 字节）",
                        budgets.max_component_bytes
                    ),
                ));
            }
            out.insert(name.clone(), value.clone());
        } else {
            return Err(err(
                pack,
                file,
                &item_field,
                format!("组件名缺少命名空间: {name:?}"),
            ));
        }
    }
    if out.contains_key("minecraft:destructible_by_mining") && out.contains_key("sc:unbreakable") {
        return Err(err(
            pack,
            file,
            field,
            "minecraft:destructible_by_mining 与 sc:unbreakable 不能同时声明",
        ));
    }
    // Rejected within one layer: loot coexisting with enabled drops (cross-layer conflicts re-checked by sc_block).
    if out.contains_key("minecraft:loot") {
        if let Some(drops_value) = out.get("sc:drops") {
            if let Ok(drops) = serde_json::from_value::<Drops>(drops_value.clone()) {
                if drops.enabled {
                    return Err(err(
                        pack,
                        file,
                        field,
                        "minecraft:loot 与启用的 sc:drops 共存，掉落来源不明",
                    ));
                }
            }
        }
    }
    // Rejected within one layer: unbreakable coexisting with mining (cross-layer conflicts re-checked by sc_block).
    if out.contains_key("sc:unbreakable") && out.contains_key("sc:mining") {
        return Err(err(pack, file, field, "sc:unbreakable 与 sc:mining 互斥"));
    }
    // A layer-local mining entry without base inherits via sc_block on the final state (common-layer destructible
    // can be inherited by permutation-layer mining); not rejected here, so cross-layer inheritance is not broken.
    Ok(out)
}

/// Budget check for `sc:mining`/`sc:drops` (entry/element counts, identifier lengths, component bytes).
fn check_component_budgets(
    pack: &str,
    file: &str,
    field: &str,
    name: &str,
    value: &Value,
    budgets: &BlockBundleBudgets,
) -> Result<(), BlockJsonError> {
    use crate::block::component::{BlockComponentSchema, Drops, Mining};
    let bytes = value.to_string().len();
    if bytes > budgets.max_component_bytes {
        return Err(err(
            pack,
            file,
            field,
            format!(
                "组件 {name} 过大（{bytes} > {} 字节）",
                budgets.max_component_bytes
            ),
        ));
    }
    if name == Mining::NAME {
        let mining: Mining = serde_json::from_value(value.clone()).map_err(|e| {
            err(
                pack,
                file,
                field,
                format!("内部不一致（mining 预算复核）：{e}"),
            )
        })?;
        if mining.tools.len() > budgets.max_mining_tools_per_state {
            return Err(err(
                pack,
                file,
                field,
                format!(
                    "sc:mining.tools 数量 {} 超出预算 {}",
                    mining.tools.len(),
                    budgets.max_mining_tools_per_state
                ),
            ));
        }
        for (i, entry) in mining.tools.iter().enumerate() {
            if entry.items.len() > budgets.max_mining_items_per_rule {
                return Err(err(
                    pack,
                    file,
                    field,
                    format!(
                        "sc:mining.tools[{i}].items 数量 {} 超出预算 {}",
                        entry.items.len(),
                        budgets.max_mining_items_per_rule
                    ),
                ));
            }
            for item in entry.items.iter() {
                if item.len() > budgets.max_identifier_len {
                    return Err(err(
                        pack,
                        file,
                        field,
                        format!(
                            "sc:mining.tools[{i}].items {item:?} 过长（{} > {}）",
                            item.len(),
                            budgets.max_identifier_len
                        ),
                    ));
                }
            }
        }
        if mining.enchantments.len() > budgets.max_mining_enchants_per_state {
            return Err(err(
                pack,
                file,
                field,
                format!(
                    "sc:mining.enchantments 数量 {} 超出预算 {}",
                    mining.enchantments.len(),
                    budgets.max_mining_enchants_per_state
                ),
            ));
        }
        for key in mining.enchantments.keys() {
            if key.len() > budgets.max_identifier_len {
                return Err(err(
                    pack,
                    file,
                    field,
                    format!(
                        "sc:mining.enchantments 键 {key:?} 过长（{} > {}）",
                        key.len(),
                        budgets.max_identifier_len
                    ),
                ));
            }
        }
    }
    if name == Drops::NAME {
        let drops: Drops = serde_json::from_value(value.clone()).map_err(|e| {
            err(
                pack,
                file,
                field,
                format!("内部不一致（drops 预算复核）：{e}"),
            )
        })?;
        if drops.entries.len() > budgets.max_drop_entries_per_state {
            return Err(err(
                pack,
                file,
                field,
                format!(
                    "sc:drops.entries 数量 {} 超出预算 {}",
                    drops.entries.len(),
                    budgets.max_drop_entries_per_state
                ),
            ));
        }
        for (i, entry) in drops.entries.iter().enumerate() {
            if entry.item.len() > budgets.max_identifier_len {
                return Err(err(
                    pack,
                    file,
                    field,
                    format!(
                        "sc:drops.entries[{i}].item {:?} 过长（{} > {}）",
                        entry.item,
                        entry.item.len(),
                        budgets.max_identifier_len
                    ),
                ));
            }
            if entry.count.len() > budgets.max_count_options_per_entry {
                return Err(err(
                    pack,
                    file,
                    field,
                    format!(
                        "sc:drops.entries[{i}].count 选项数 {} 超出预算 {}",
                        entry.count.len(),
                        budgets.max_count_options_per_entry
                    ),
                ));
            }
            for (key, rule) in entry.enchantments.iter() {
                if key.len() > budgets.max_identifier_len {
                    return Err(err(
                        pack,
                        file,
                        field,
                        format!(
                            "sc:drops.entries[{i}].enchantments 键 {key:?} 过长（{} > {}）",
                            key.len(),
                            budgets.max_identifier_len
                        ),
                    ));
                }
                if rule.bonus_by_level.len() > budgets.max_fortune_rules_per_entry {
                    return Err(err(
                        pack,
                        file,
                        field,
                        format!(
                            "sc:drops.entries[{i}].bonus_by_level 行数 {} 超出预算 {}",
                            rule.bonus_by_level.len(),
                            budgets.max_fortune_rules_per_entry
                        ),
                    ));
                }
            }
            if let Some(requires) = entry.requires.as_ref() {
                if requires.items.len() > budgets.max_requires_items_per_entry {
                    return Err(err(
                        pack,
                        file,
                        field,
                        format!(
                            "sc:drops.entries[{i}].requires.items 数量 {} 超出预算 {}",
                            requires.items.len(),
                            budgets.max_requires_items_per_entry
                        ),
                    ));
                }
                for item in requires.items.iter() {
                    if item.len() > budgets.max_identifier_len {
                        return Err(err(
                            pack,
                            file,
                            field,
                            format!(
                                "sc:drops.entries[{i}].requires.items {item:?} 过长（{} > {}）",
                                item.len(),
                                budgets.max_identifier_len
                            ),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Forbidden-char check for property names/values: whitespace and key separators.
///
/// Namespace prefixes are allowed (real palettes prefix names such as `minecraft:block_face`);
/// forbidden chars are the separators that state-key building and molang round-trips depend on: whitespace, `,`, `=`,
/// single/double quotes and backslash.
fn forbidden_key_char(c: char) -> bool {
    c.is_whitespace() || ",='\"\\".contains(c)
}

/// Property-name contract: non-empty, length-bounded, no forbidden chars.
fn check_property_name(
    pack: &str,
    file: &str,
    field: &str,
    name: &str,
    budgets: &BlockBundleBudgets,
) -> Result<(), BlockJsonError> {
    if name.is_empty() {
        return Err(err(pack, file, field, "属性名不能为空"));
    }
    if name.len() > budgets.max_prop_string_len {
        return Err(err(
            pack,
            file,
            field,
            format!(
                "属性名过长（{} > {} 字节）",
                name.len(),
                budgets.max_prop_string_len
            ),
        ));
    }
    if let Some(bad) = name.chars().find(|c| forbidden_key_char(*c)) {
        return Err(err(
            pack,
            file,
            field,
            format!("属性名含非法字符 {bad:?}（空白与 , = ' \" \\ 不允许）"),
        ));
    }
    Ok(())
}

/// String property-value contract (also bans key separators, so molang literals round-trip).
fn check_prop_string_value(
    pack: &str,
    file: &str,
    field: &str,
    value: &str,
    budgets: &BlockBundleBudgets,
) -> Result<(), BlockJsonError> {
    if value.len() > budgets.max_prop_string_len {
        return Err(err(
            pack,
            file,
            field,
            format!(
                "string 属性过长（{} > {} 字节）",
                value.len(),
                budgets.max_prop_string_len
            ),
        ));
    }
    if let Some(bad) = value.chars().find(|c| forbidden_key_char(*c)) {
        return Err(err(
            pack,
            file,
            field,
            format!("string 取值含非法字符 {bad:?}（空白与 , = ' \" \\ 不允许）"),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Single-file parse output (input to sc_block compilation; no hashes or dense ids)
// ---------------------------------------------------------------------------

/// One conjunct of a permutation condition (restricted syntax, see [`parse_permutation_condition`]).
#[derive(Clone, Debug)]
pub struct ConditionTest {
    pub property: String,
    pub value: PropValue,
}

/// Permutation condition (conjunction; deterministic evaluation, no molang runtime).
#[derive(Clone, Debug)]
pub struct PermutationCondition {
    pub tests: Vec<ConditionTest>,
}

impl PermutationCondition {
    /// Evaluate against a property assignment (matches when every conjunct holds; unlisted properties are ignored).
    pub fn matches(&self, props: &[(String, PropValue)]) -> bool {
        self.tests
            .iter()
            .all(|t| props.iter().any(|(k, v)| k == &t.property && v == &t.value))
    }

    /// Write back the canonical condition text (the splitter reuses the same syntax, so round-trips agree).
    pub fn to_expression(&self, identifier: &str) -> String {
        self.tests
            .iter()
            .map(|t| {
                format!(
                    "q.block_property('{}','{}') == {}",
                    identifier,
                    t.property,
                    t.value.molang_literal()
                )
            })
            .collect::<Vec<_>>()
            .join(" && ")
    }
}

#[derive(Clone, Debug)]
pub struct ParsedPermutation {
    pub condition: PermutationCondition,
    /// Overriding components (validated raw JSON; whole-component value replacement).
    pub components: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct ParsedBlockFile {
    pub zip_path: String,
    pub identifier: String,
    /// Canonical order (property names ascending; value order = declaration order).
    pub properties: Vec<PropertyDef>,
    /// Default-state canonical index (explicitly declared by `sc:default_state`, never guessed).
    pub default_index: usize,
    /// Protocol runtime ids in one-to-one canonical state order (`sc:protocol_runtime_ids`,
    /// never recomputed from sorting).
    pub network_ids: Vec<u32>,
    /// Common components (validated raw JSON: name to value).
    pub base: BTreeMap<String, Value>,
    pub permutations: Vec<ParsedPermutation>,
}

impl ParsedBlockFile {
    /// State count (= `network_ids` length = canonical cartesian-product cardinality).
    pub fn state_count(&self) -> usize {
        self.network_ids.len()
    }

    /// Canonical state property assignments (indexed like `network_ids` / `sc:default_state`).
    pub fn state_props(&self, index: usize) -> Vec<(String, PropValue)> {
        canonical_state_props(&self.properties, index)
    }

    /// Human-readable canonical state keys.
    pub fn state_key(&self, index: usize) -> String {
        canonical_state_key(&self.state_props(index))
    }
}

// ---------------------------------------------------------------------------
// Permutation conditions: restricted-syntax parsing
// ---------------------------------------------------------------------------

fn read_quoted(src: &str) -> Result<(String, &str), String> {
    let s = src.trim_start();
    let inner = s
        .strip_prefix('\'')
        .ok_or_else(|| "应为单引号包裹的字符串".to_string())?;
    match inner.find('\'') {
        Some(i) => Ok((inner[..i].to_string(), &inner[i + 1..])),
        None => Err("单引号字符串未闭合".to_string()),
    }
}

/// Parse the restricted permutation-condition syntax (deterministic evaluation, no molang runtime):
///
/// ```text
/// condition := compare ("&&" compare)*
/// compare   := "q.block_property(" Q "," Q ")" "==" literal
/// literal   := "'" <string> "'" | <decimal integer> | true | false
/// ```
///
/// Only the forms above are supported: `||`, other query functions, arithmetic, and other comparators are loudly rejected
/// (never silently ignored or guessed). The queried identifier must equal this file
/// identifier, and properties/values must be declared in this file `description.states`.
pub fn parse_permutation_condition(
    pack: &str,
    file: &str,
    field: &str,
    identifier: &str,
    properties: &[PropertyDef],
    src: &str,
    budgets: &BlockBundleBudgets,
) -> Result<PermutationCondition, BlockJsonError> {
    let unsupported = "条件语法仅支持 q.block_property('<本文件 identifier>','<属性>') == <字面量> 的 && 合取（不支持 || / 算术 / 其他查询）";
    if src.len() > budgets.max_condition_len {
        return Err(err(
            pack,
            file,
            field,
            format!(
                "条件过长（{} > {} 字节）",
                src.len(),
                budgets.max_condition_len
            ),
        ));
    }
    if src.trim().is_empty() {
        return Err(err(pack, file, field, "条件不能为空"));
    }
    let mut tests = Vec::new();
    for (i, part) in src.split("&&").enumerate() {
        let part_field = format!("{field}[{i}]");
        let part = part.trim();
        let rest = part
            .strip_prefix("q.block_property(")
            .ok_or_else(|| err(pack, file, &part_field, unsupported))?;
        let (query_id, rest) = read_quoted(rest).map_err(|m| err(pack, file, &part_field, m))?;
        let rest = rest
            .trim_start()
            .strip_prefix(',')
            .ok_or_else(|| err(pack, file, &part_field, "查询参数之间缺 ','"))?;
        let (prop_name, rest) = read_quoted(rest).map_err(|m| err(pack, file, &part_field, m))?;
        let rest = rest
            .trim_start()
            .strip_prefix(')')
            .ok_or_else(|| err(pack, file, &part_field, "查询调用缺 ')'"))?;
        let rest = rest
            .trim_start()
            .strip_prefix("==")
            .ok_or_else(|| err(pack, file, &part_field, "仅支持 == 比较"))?;
        let literal = rest.trim();
        if literal.is_empty() || literal.contains(char::is_whitespace) {
            return Err(err(pack, file, &part_field, "比较右侧应为单个字面量"));
        }
        if query_id != identifier {
            return Err(err(
                pack,
                file,
                &part_field,
                format!(
                    "条件查询的 identifier {query_id:?} 与本文件 {identifier:?} 不一致（不支持跨方块查询）"
                ),
            ));
        }
        let value = PropValue::from_molang_literal(literal).ok_or_else(|| {
            err(
                pack,
                file,
                &part_field,
                format!("无法识别的字面量 {literal:?}（允许字符串/整数/true/false）"),
            )
        })?;
        let prop = properties
            .iter()
            .find(|p| p.name == *prop_name)
            .ok_or_else(|| {
                err(
                    pack,
                    file,
                    &part_field,
                    format!("条件引用了未声明的属性 {prop_name:?}"),
                )
            })?;
        if prop.value_index(&value).is_none() {
            return Err(err(
                pack,
                file,
                &part_field,
                format!(
                    "条件取值 {} 不是属性 {prop_name:?} 的声明值（类型 {:?}，声明 {:?}）",
                    literal,
                    value.prop_type(),
                    prop.values.iter().map(|v| v.repr()).collect::<Vec<_>>()
                ),
            ));
        }
        tests.push(ConditionTest {
            property: prop_name,
            value,
        });
    }
    Ok(PermutationCondition { tests })
}

// ---------------------------------------------------------------------------
// Single-file parsing
// ---------------------------------------------------------------------------

/// `description.states` to property declarations (both vanilla spellings accepted):
///
/// ```json
/// "states": { "pillar_axis": ["x", "y", "z"] }
/// "states": [ { "name": "pillar_axis", "values": ["x", "y", "z"] } ]
/// ```
///
/// Property types are fixed by value shape: all-bool to byte, integers to int, strings to string;
/// mixed shapes, duplicate values, and float/compound values are loudly rejected (never guessed or unified).
fn parse_states(
    pack: &str,
    file: &str,
    field: &str,
    v: &Value,
    budgets: &BlockBundleBudgets,
) -> Result<Vec<PropertyDef>, BlockJsonError> {
    let mut raw: Vec<(String, &Vec<Value>)> = Vec::new();
    match v {
        Value::Object(obj) => {
            for (name, values) in obj.iter() {
                let arr = values.as_array().ok_or_else(|| {
                    err(
                        pack,
                        file,
                        &format!("{field}.{name}"),
                        "属性值应为 JSON 数组",
                    )
                })?;
                raw.push((name.clone(), arr));
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let item_field = format!("{field}[{i}]");
                let obj = as_object(pack, file, &item_field, item)?;
                check_no_unknown_keys(pack, file, &item_field, obj, &["name", "values"])?;
                let name = obj
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| err(pack, file, &item_field, "缺少 name"))?;
                let values = obj
                    .get("values")
                    .and_then(Value::as_array)
                    .ok_or_else(|| err(pack, file, &item_field, "缺少 values 数组"))?;
                raw.push((name.to_string(), values));
            }
        }
        _ => {
            return Err(err(
                pack,
                file,
                field,
                "应为对象 {name: [values]} 或数组 [{name, values}]",
            ))
        }
    }
    // Canonical order: property names ascending (duplicate names rejected).
    raw.sort_by(|a, b| a.0.cmp(&b.0));
    let mut properties: Vec<PropertyDef> = Vec::with_capacity(raw.len());
    for (i, (name, values)) in raw.into_iter().enumerate() {
        let prop_field = format!("{field}[{i}]");
        check_property_name(pack, file, &prop_field, &name, budgets)?;
        if i > 0 && properties[i - 1].name == name {
            return Err(err(
                pack,
                file,
                &prop_field,
                format!("属性重复声明 {name:?}"),
            ));
        }
        if values.is_empty() {
            return Err(err(pack, file, &prop_field, "values 为空"));
        }
        if values.len() > budgets.max_prop_values_per_block {
            return Err(err(
                pack,
                file,
                &prop_field,
                format!(
                    "values 数量 {} 超出预算 {}",
                    values.len(),
                    budgets.max_prop_values_per_block
                ),
            ));
        }
        let mut parsed: Vec<PropValue> = Vec::with_capacity(values.len());
        for (vi, value) in values.iter().enumerate() {
            let value_field = format!("{prop_field}.values[{vi}]");
            let pv = PropValue::from_json(value).ok_or_else(|| {
                err(
                    pack,
                    file,
                    &value_field,
                    "属性值应为布尔（byte）、整数（int）或字符串（string）",
                )
            })?;
            if let PropValue::String(s) = &pv {
                check_prop_string_value(pack, file, &value_field, s, budgets)?;
            }
            parsed.push(pv);
        }
        let nbt_type = parsed[0].prop_type();
        if let Some(bad) = parsed.iter().find(|v| v.prop_type() != nbt_type) {
            return Err(err(
                pack,
                file,
                &prop_field,
                format!(
                    "同一属性的值类型必须一致：期望 {:?}，遇到 {:?}",
                    nbt_type,
                    bad.prop_type()
                ),
            ));
        }
        let mut seen: HashSet<String> = HashSet::with_capacity(parsed.len());
        for (vi, pv) in parsed.iter().enumerate() {
            if !seen.insert(pv.token()) {
                return Err(err(
                    pack,
                    file,
                    &format!("{prop_field}.values[{vi}]"),
                    format!("重复取值 {}", pv.repr()),
                ));
            }
        }
        properties.push(PropertyDef {
            name,
            nbt_type,
            values: parsed,
        });
    }
    Ok(properties)
}

/// Parse and validate one `.block.json` file (vanilla envelope; touches no global state).
///
/// ```json
/// {
///   "format_version": "1.10.0",
///   "minecraft:block": {
///     "description": { "identifier": "minecraft:oak_log",
///                      "states": { "pillar_axis": ["x", "y", "z"] } },
///     "components": { "minecraft:destructible_by_mining": { "value": 2.0 } },
///     "permutations": [
///       { "condition": "q.block_property('minecraft:oak_log','pillar_axis') == 'y'",
///         "components": { "minecraft:light_dampening": 15 } }
///     ],
///     "sc:default_state": { "pillar_axis": "y" },
///     "sc:protocol_runtime_ids": [100, 101, 102]
///   }
/// }
/// ```
///
/// Server-required data with no vanilla counterpart lives under `sc:` and is always explicit:
/// default state (never guessed) and protocol runtime ids (never recomputed from sorting).
/// Any `ur:*` top-level key is rejected with no compatibility handling
pub fn parse_block_file(
    pack: &str,
    path: &str,
    bytes: &[u8],
    budgets: &BlockBundleBudgets,
) -> Result<ParsedBlockFile, BlockJsonError> {
    let e = |field: &str, message: String| err(pack, path, field, message);
    reject_duplicate_keys(bytes).map_err(|m| e("$", format!("JSON 重复 key 或结构异常: {m}")))?;
    let root: Value =
        serde_json::from_slice(bytes).map_err(|ex| e("$", format!("JSON 解析失败: {ex}")))?;
    let obj = root
        .as_object()
        .ok_or_else(|| e("$", "根应为 JSON 对象".to_string()))?;
    check_no_unknown_keys(pack, path, "$", obj, &["format_version", "minecraft:block"])?;
    let format_version = obj
        .get("format_version")
        .and_then(Value::as_str)
        .ok_or_else(|| e("$.format_version", "必填（原版格式版本字符串）".to_string()))?;
    if format_version != BLOCK_JSON_FORMAT_VERSION {
        return Err(e(
            "$.format_version",
            format!("不支持的 format_version {format_version:?}（本实现仅支持 {BLOCK_JSON_FORMAT_VERSION:?}）"),
        ));
    }
    let block_value = obj
        .get("minecraft:block")
        .ok_or_else(|| e("$.minecraft:block", "必填（原版方块定义体）".to_string()))?;
    let block = as_object(pack, path, "$.minecraft:block", block_value)?;
    check_no_unknown_keys(
        pack,
        path,
        "$.minecraft:block",
        block,
        &[
            "description",
            "components",
            "permutations",
            "sc:default_state",
            "sc:protocol_runtime_ids",
        ],
    )?;
    // Any `ur:*` top-level key is rejected with no compatibility handling.
    for key in block.keys() {
        if key.starts_with("ur:") {
            return Err(e(
                "$.minecraft:block",
                format!("未知字段 {key:?}（`ur:*` 已整体迁移为 `sc:*`，不做兼容）"),
            ));
        }
    }

    let description_value = block
        .get("description")
        .ok_or_else(|| e("$.minecraft:block.description", "必填".to_string()))?;
    let description = as_object(
        pack,
        path,
        "$.minecraft:block.description",
        description_value,
    )?;
    check_no_unknown_keys(
        pack,
        path,
        "$.minecraft:block.description",
        description,
        &["identifier", "states"],
    )?;
    let identifier = description
        .get("identifier")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            e(
                "$.minecraft:block.description.identifier",
                "必填（方块 identifier）".to_string(),
            )
        })?;
    validate_identifier(identifier, budgets.max_identifier_len)
        .map_err(|m| e("$.minecraft:block.description.identifier", m))?;
    let expected = validate_block_path(path, budgets.max_identifier_len)
        .map_err(|m| e("$", format!("文件路径非法: {m}")))?;
    if identifier != expected {
        return Err(e(
            "$.minecraft:block.description.identifier",
            format!("identifier {identifier:?} 与文件路径 {path:?} 不一致（应为 {expected:?}）"),
        ));
    }
    let states_value = description.get("states").ok_or_else(|| {
        e(
            "$.minecraft:block.description.states",
            "必填（无属性方块请显式写 {}）".to_string(),
        )
    })?;
    let properties = parse_states(
        pack,
        path,
        "$.minecraft:block.description.states",
        states_value,
        budgets,
    )?;
    let state_count = canonical_state_count(&properties);
    if state_count > budgets.max_states_per_block {
        return Err(e(
            "$.minecraft:block.description.states",
            format!(
                "状态组合数 {state_count} 超出单方块预算 {}",
                budgets.max_states_per_block
            ),
        ));
    }

    // ---- sc:default_state (explicit default state; missing entries are rejected, never guessed) ----
    let default_field = "$.minecraft:block.sc:default_state";
    let default_value = block
        .get("sc:default_state")
        .ok_or_else(|| e(default_field, "必填（服务端必需数据，不猜测）".to_string()))?;
    let default_obj = as_object(pack, path, default_field, default_value)?;
    let mut default_assignment: BTreeMap<String, PropValue> = BTreeMap::new();
    for (prop_name, value) in default_obj.iter() {
        let field = format!("{default_field}.{prop_name}");
        let prop = properties
            .iter()
            .find(|p| p.name == *prop_name)
            .ok_or_else(|| err(pack, path, &field, format!("未声明的属性 {prop_name:?}")))?;
        let pv = PropValue::from_json(value)
            .ok_or_else(|| err(pack, path, &field, "取值应为布尔/整数/字符串"))?;
        if pv.prop_type() != prop.nbt_type {
            return Err(err(
                pack,
                path,
                &field,
                format!(
                    "取值类型 {:?} 与属性声明类型 {:?} 不符",
                    pv.prop_type(),
                    prop.nbt_type
                ),
            ));
        }
        if prop.value_index(&pv).is_none() {
            return Err(err(
                pack,
                path,
                &field,
                format!("取值 {} 不是该属性的声明值", pv.repr()),
            ));
        }
        default_assignment.insert(prop_name.clone(), pv);
    }
    let default_index =
        canonical_state_index(&properties, &default_assignment).ok_or_else(|| {
            err(
                pack,
                path,
                default_field,
                format!(
                    "必须恰好覆盖全部 {} 个属性（当前 {} 个）",
                    properties.len(),
                    default_assignment.len()
                ),
            )
        })?;

    // ---- sc:protocol_runtime_ids (order is canonical state order; never recomputed) ----
    let ids_field = "$.minecraft:block.sc:protocol_runtime_ids";
    let ids_value = block.get("sc:protocol_runtime_ids").ok_or_else(|| {
        e(
            ids_field,
            "必填（protocol runtime id 不由排序推导）".to_string(),
        )
    })?;
    let ids = ids_value
        .as_array()
        .ok_or_else(|| e(ids_field, "应为 JSON 数组".to_string()))?;
    if ids.len() != state_count {
        return Err(e(
            ids_field,
            format!(
                "长度 {} 与状态组合数 {state_count} 不符（一一对应，顺序 = 规范状态顺序）",
                ids.len()
            ),
        ));
    }
    let mut network_ids = Vec::with_capacity(ids.len());
    for (i, v) in ids.iter().enumerate() {
        let n = v
            .as_u64()
            .ok_or_else(|| err(pack, path, &format!("{ids_field}[{i}]"), "应为非负整数"))?;
        if n > u32::MAX as u64 {
            return Err(err(
                pack,
                path,
                &format!("{ids_field}[{i}]"),
                format!("{n} 超出 u32"),
            ));
        }
        network_ids.push(n as u32);
    }

    // ---- components (common components) ----
    let base = match block.get("components") {
        None => BTreeMap::new(),
        Some(v) => validate_components_map(pack, path, "$.minecraft:block.components", v, budgets)?,
    };

    // ---- permutations (restricted condition syntax) ----
    let mut permutations = Vec::new();
    if let Some(perms) = block.get("permutations") {
        let arr = perms
            .as_array()
            .ok_or_else(|| e("$.minecraft:block.permutations", "应为数组".to_string()))?;
        if arr.len() > budgets.max_permutations_per_block {
            return Err(e(
                "$.minecraft:block.permutations",
                format!(
                    "数量 {} 超出单方块预算 {}",
                    arr.len(),
                    budgets.max_permutations_per_block
                ),
            ));
        }
        for (i, item) in arr.iter().enumerate() {
            let field = format!("$.minecraft:block.permutations[{i}]");
            let obj = as_object(pack, path, &field, item)?;
            check_no_unknown_keys(pack, path, &field, obj, &["condition", "components"])?;
            let condition_src = obj
                .get("condition")
                .and_then(Value::as_str)
                .ok_or_else(|| err(pack, path, &format!("{field}.condition"), "必填（字符串）"))?;
            let condition = parse_permutation_condition(
                pack,
                path,
                &format!("{field}.condition"),
                identifier,
                &properties,
                condition_src,
                budgets,
            )?;
            let components = match obj.get("components") {
                None => BTreeMap::new(),
                Some(v) => {
                    validate_components_map(pack, path, &format!("{field}.components"), v, budgets)?
                }
            };
            permutations.push(ParsedPermutation {
                condition,
                components,
            });
        }
    }

    Ok(ParsedBlockFile {
        zip_path: path.to_string(),
        identifier: identifier.to_string(),
        properties,
        default_index,
        network_ids,
        base,
        permutations,
    })
}

fn fnv1a64(data: &[u8]) -> u64 {
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

fn read_capped<T: Read>(
    mut file: T,
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
pub fn load_block_json_bundle<T: Read + Seek>(
    zip: &mut zip::ZipArchive<T>,
    manifest: &BlockDataManifest,
    pack_id: &str,
    budgets: &BlockBundleBudgets,
) -> Result<Option<BlockJsonBundle>, BlockJsonError> {
    manifest.validate(pack_id)?;
    let prefix = format!("{BLOCKS_DIRECTORY}/");
    // Collect candidate entries (case-sensitive prefix; the directory itself is skipped).
    let mut candidates: Vec<(String, u64)> = Vec::new();
    for i in 0..zip.len() {
        let file = zip.by_index(i).map_err(|e| {
            BlockJsonError::new(pack_id, "<bundle>", "$", format!("zip 索引失败: {e}"))
        })?;
        let name = file.name().to_string();
        if !file.is_file() {
            continue;
        }
        if !name.starts_with(&prefix) || !name.ends_with(".block.json") {
            continue;
        }
        candidates.push((name, file.size()));
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
        let file = zip
            .by_name(name)
            .map_err(|e| BlockJsonError::new(pack_id, name, "$", format!("zip 读取失败: {e}")))?;
        let bytes = read_capped(file, budgets.max_file_bytes, pack_id, name)?;
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
const RESERVED_META_KEYS: &[&str] = &[
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
#[derive(Clone, Debug)]
pub enum SplitError {
    MissingDefaults(Vec<String>),
    Invalid(String),
}

impl fmt::Display for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SplitError::MissingDefaults(ids) => {
                write!(
                    f,
                    "缺少权威默认态（{} 个，多状态方块不猜测默认）：{}",
                    ids.len(),
                    ids.join(", ")
                )
            }
            SplitError::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SplitError {}

fn nbt_states_to_prop(
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
            SplitError::Invalid(format!("旧条目 identifier 非法 {:?}：{m}", entry.name))
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
                let (t, pv) = nbt_states_to_prop(identifier, k, v).map_err(SplitError::Invalid)?;
                if let PropValue::Byte(b) = pv {
                    if b != 0 && b != 1 {
                        return Err(SplitError::Invalid(format!(
                            "{identifier} 的属性 {k:?} 取 byte 值 {b}（JSON 侧只能表达 0/1 布尔，不做有损转换）"
                        )));
                    }
                }
                if let Some(prev) = prop_types.get(k.as_str()) {
                    if *prev != t {
                        return Err(SplitError::Invalid(format!(
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
                return Err(SplitError::Invalid(format!(
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
            return Err(SplitError::Invalid(format!(
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
                return Err(SplitError::Invalid(format!(
                    "{identifier} 的源 states 出现重复属性组合"
                )));
            }
        }
        if by_assignment.len() != state_count {
            return Err(SplitError::Invalid(format!(
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
                SplitError::Invalid(format!(
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
                SplitError::Invalid(format!(
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
            .map_err(SplitError::Invalid)?;
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
                        .ok_or_else(|| SplitError::Invalid(format!("{identifier} 硬度非有限值")))?;
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
            .map_err(|e| SplitError::Invalid(format!("JSON 序列化失败：{e}")))?;
        out.push(GeneratedBlockFile { zip_path, bytes });
    }
    if !missing_defaults.is_empty() {
        missing_defaults.sort();
        return Err(SplitError::MissingDefaults(missing_defaults));
    }
    // Self-check: generated files must load (catches splitter bugs).
    for file in out.iter() {
        parse_block_file("<split>", &file.zip_path, &file.bytes, budgets)
            .map_err(|e| SplitError::Invalid(format!("拆分自检失败 {}：{e}", file.zip_path)))?;
    }
    Ok(out)
}

/// Source property assignment to canonical index (fails when a value is outside the declared set).
fn default_state_index_of(
    properties: &[PropertyDef],
    assignment: &BTreeMap<String, PropValue>,
    identifier: &str,
) -> Result<usize, SplitError> {
    canonical_state_index(properties, assignment).ok_or_else(|| {
        SplitError::Invalid(format!(
            "{identifier} 的默认态属性组合不在声明值域内（不猜测）"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn make_zip(entries: &[(&str, &[u8])]) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        let cursor = writer.finish().unwrap();
        zip::ZipArchive::new(cursor).unwrap()
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
        let mut zip = make_zip(&[
            (log_path(), &log),
            (air_path(), &air),
            ("other/notes.txt", b"ignore me"),
        ]);
        let bundle = load_block_json_bundle(
            &mut zip,
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
        let mut zip = make_zip(&[("manifest.json", b"{}")]);
        load_block_json_bundle(
            &mut zip,
            &test_manifest(),
            PACK,
            &BlockBundleBudgets::default(),
        )
        .expect_err("零文件必须失败");
        // Illegal path (../).
        let air = air_json();
        let mut zip = make_zip(&[("definitions/blocks/minecraft/../evil.block.json", &air)]);
        let e = load_block_json_bundle(
            &mut zip,
            &test_manifest(),
            PACK,
            &BlockBundleBudgets::default(),
        )
        .expect_err("穿越路径必须失败");
        assert!(e.to_string().contains("非法"), "实际：{e}");
        // Broken JSON: never skipped, must fail with the file path.
        let mut zip = make_zip(&[(air_path(), b"{oops")]);
        let e = load_block_json_bundle(
            &mut zip,
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
        let mut zip = make_zip(&[(air_path(), bad.as_bytes())]);
        load_block_json_bundle(
            &mut zip,
            &test_manifest(),
            PACK,
            &BlockBundleBudgets::default(),
        )
        .expect_err("坏 schema 必须失败");
        // File-count budget.
        let air = air_json();
        let mut zip = make_zip(&[(air_path(), &air)]);
        let mut tight = BlockBundleBudgets::default();
        tight.max_files = 0;
        load_block_json_bundle(&mut zip, &test_manifest(), PACK, &tight).expect_err("文件数预算");
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
            SplitError::MissingDefaults(ids) => assert_eq!(ids, vec!["test:mixed".to_string()]),
            other => panic!("应进入待补齐清单，实际：{other}"),
        }
    }

    /// Real version-pack stats: measured basis for budget defaults (single-state subset split + reparse).
    #[test]
    fn real_pack_stats_and_single_state_subset_round_trip() {
        use std::fs;
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = fs::read(pack_path).expect("读取版本包失败");
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip 打开失败");
        let palette_bytes = {
            let mut f = zip
                .by_name("definitions/block_palette.nbt")
                .expect("palette 缺失");
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
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
