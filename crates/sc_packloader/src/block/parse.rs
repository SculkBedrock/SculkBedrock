//! Single-file `.block.json` parsing: direct load of the vanilla envelope
//! (`format_version` + `minecraft:block`) into [`ParsedBlockFile`].
//!
//! Loading is direct: JSON in, typed parse output out. There is no format
//! whitelist, no duplicate-key scan, no unknown-field rejection and no
//! budgets — only the data model's required pieces are enforced (identifier,
//! property typing, `sc:protocol_runtime_ids`, and the permutation-condition
//! grammar that defines how conditions evaluate).

use std::collections::BTreeMap;

use sc_nbt::NbtValue;
use serde_json::Value;

use crate::version_control::json_budget::BudgetReader;
use crate::block::BlockJsonError;
use crate::block::component::is_ignored_component;

// ---------------------------------------------------------------------------
// Property types and values (byte/int/string stay distinct, never unified)
// ---------------------------------------------------------------------------

/// 方块属性的 NBT 类型。
///
/// byte 与 int 属于不同的状态标识，不合并。
///
/// NBT type of a block property.
///
/// byte and int are distinct state identities and are never merged.
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

/// 解析阶段的带类型属性值。
///
/// Typed property value used during parsing.
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
    /// floats/compounds map to `None` (callers fail loudly, no lenient conversion).
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

/// 单个属性声明的定义，对应 description.states 的一项。
///
/// Declaration of one property, corresponding to an entry of description.states.
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
/// origin as the [`crate::block::split_state_key`] `{prop}_{repr}` rule.
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
// Single-file parse output (input to sc_block compilation; no hashes or dense ids)
// ---------------------------------------------------------------------------

/// 排列条件中的一个合取项。
///
/// One conjunct of a permutation condition.
#[derive(Clone, Debug)]
pub struct ConditionTest {
    pub property: String,
    pub value: PropValue,
}

/// 排列条件的合取组合，按确定性规则求值。
///
/// Conjunction of permutation tests evaluated deterministically.
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
    /// Overriding components (raw JSON; whole-component value replacement).
    pub components: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct ParsedBlockFile {
    pub zip_path: String,
    pub identifier: String,
    /// Canonical order (property names ascending; value order = declaration order).
    pub properties: Vec<PropertyDef>,
    /// Default-state canonical index: the first declared value of every property
    /// (canonical index 0; the format carries no explicit default-state field).
    pub default_index: usize,
    /// Protocol runtime ids in one-to-one canonical state order (`sc:protocol_runtime_ids`,
    /// never recomputed from sorting).
    pub network_ids: Vec<u32>,
    /// Common components (raw JSON: name to value; client-side ignore-list entries dropped).
    pub base: BTreeMap<String, Value>,
    pub permutations: Vec<ParsedPermutation>,
}

impl ParsedBlockFile {
    /// State count (= `network_ids` length = canonical cartesian-product cardinality).
    pub fn state_count(&self) -> usize {
        self.network_ids.len()
    }

    /// Canonical state property assignments (indexed like `network_ids`).
    pub fn state_props(&self, index: usize) -> Vec<(String, PropValue)> {
        canonical_state_props(&self.properties, index)
    }

    /// Human-readable canonical state keys.
    pub fn state_key(&self, index: usize) -> String {
        canonical_state_key(&self.state_props(index))
    }

    /// All `minecraft:loot` reference paths in this file (base layer plus
    /// permutation layers; both the string and the `{"table": path}` object
    /// spelling; deduped, first-seen order).
    pub fn referenced_loot_paths(&self) -> Vec<String> {
        const LOOT: &str = crate::loot_tables::LOOT_REFERENCE_COMPONENT;
        let mut out = Vec::new();
        let mut push = |map: &BTreeMap<String, Value>| {
            if let Some(path) = crate::loot_tables::loot_reference_path(map.get(LOOT)) {
                if !out.contains(&path) {
                    out.push(path);
                }
            }
        };
        push(&self.base);
        for permutation in self.permutations.iter() {
            push(&permutation.components);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Identifier/path mapping (used by the palette splitter for generated file paths)
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
) -> Result<PermutationCondition, BlockJsonError> {
    let e = |field: &str, message: String| BlockJsonError::new(pack, file, field, message);
    let unsupported = "条件语法仅支持 q.block_property('<本文件 identifier>','<属性>') == <字面量> 的 && 合取（不支持 || / 算术 / 其他查询）";
    if src.trim().is_empty() {
        return Err(e(field, "条件不能为空".to_string()));
    }
    let mut tests = Vec::new();
    for (i, part) in src.split("&&").enumerate() {
        let part_field = format!("{field}[{i}]");
        let part = part.trim();
        let rest = part
            .strip_prefix("q.block_property(")
            .ok_or_else(|| e(&part_field, unsupported.to_string()))?;
        let (query_id, rest) = read_quoted(rest).map_err(|m| e(&part_field, m))?;
        let rest = rest
            .trim_start()
            .strip_prefix(',')
            .ok_or_else(|| e(&part_field, "查询参数之间缺 ','".to_string()))?;
        let (prop_name, rest) = read_quoted(rest).map_err(|m| e(&part_field, m))?;
        let rest = rest
            .trim_start()
            .strip_prefix(')')
            .ok_or_else(|| e(&part_field, "查询调用缺 ')'".to_string()))?;
        let rest = rest
            .trim_start()
            .strip_prefix("==")
            .ok_or_else(|| e(&part_field, "仅支持 == 比较".to_string()))?;
        let literal = rest.trim();
        if literal.is_empty() || literal.contains(char::is_whitespace) {
            return Err(e(&part_field, "比较右侧应为单个字面量".to_string()));
        }
        if query_id != identifier {
            return Err(e(
                &part_field,
                format!(
                    "条件查询的 identifier {query_id:?} 与本文件 {identifier:?} 不一致（不支持跨方块查询）"
                ),
            ));
        }
        let value = PropValue::from_molang_literal(literal).ok_or_else(|| {
            e(
                &part_field,
                format!("无法识别的字面量 {literal:?}（允许字符串/整数/true/false）"),
            )
        })?;
        let prop = properties
            .iter()
            .find(|p| p.name == *prop_name)
            .ok_or_else(|| {
                e(
                    &part_field,
                    format!("条件引用了未声明的属性 {prop_name:?}"),
                )
            })?;
        if prop.value_index(&value).is_none() {
            return Err(e(
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

fn as_object<'a>(
    pack: &str,
    file: &str,
    field: &str,
    v: &'a Value,
) -> Result<&'a serde_json::Map<String, Value>, BlockJsonError> {
    v.as_object()
        .ok_or_else(|| BlockJsonError::new(pack, file, field, "应为 JSON 对象"))
}

/// Take a component map layer: raw JSON name→value, dropping the client-side
/// ignore-list entries (they never enter snapshots).
fn component_layer(
    pack: &str,
    file: &str,
    field: &str,
    v: &Value,
) -> Result<BTreeMap<String, Value>, BlockJsonError> {
    let obj = as_object(pack, file, field, v)?;
    let mut out = BTreeMap::new();
    for (name, value) in obj.iter() {
        if !is_ignored_component(name) {
            out.insert(name.clone(), value.clone());
        }
    }
    Ok(out)
}

/// `description.states` to property declarations (both vanilla spellings accepted):
///
/// ```json
/// "states": { "pillar_axis": ["x", "y", "z"] }
/// "states": [ { "name": "pillar_axis", "values": ["x", "y", "z"] } ]
/// ```
///
/// Property types are fixed by value shape: all-bool to byte, integers to int, strings to string.
pub(super) fn parse_states(
    pack: &str,
    file: &str,
    field: &str,
    v: &Value,
) -> Result<Vec<PropertyDef>, BlockJsonError> {
    let mut raw: Vec<(String, &Vec<Value>)> = Vec::new();
    match v {
        Value::Object(obj) => {
            for (name, values) in obj.iter() {
                let arr = values.as_array().ok_or_else(|| {
                    BlockJsonError::new(
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
                let name = obj
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| BlockJsonError::new(pack, file, &item_field, "缺少 name"))?;
                let values = obj.get("values").and_then(Value::as_array).ok_or_else(|| {
                    BlockJsonError::new(pack, file, &item_field, "缺少 values 数组")
                })?;
                raw.push((name.to_string(), values));
            }
        }
        _ => {
            return Err(BlockJsonError::new(
                pack,
                file,
                field,
                "应为对象 {name: [values]} 或数组 [{name, values}]",
            ))
        }
    }
    // Canonical order: property names ascending.
    raw.sort_by(|a, b| a.0.cmp(&b.0));
    let mut properties: Vec<PropertyDef> = Vec::with_capacity(raw.len());
    for (name, values) in raw {
        if values.is_empty() {
            return Err(BlockJsonError::new(
                pack,
                file,
                &format!("{field}.{name}"),
                "values 为空",
            ));
        }
        let mut parsed: Vec<PropValue> = Vec::with_capacity(values.len());
        for value in values.iter() {
            let pv = PropValue::from_json(value).ok_or_else(|| {
                BlockJsonError::new(
                    pack,
                    file,
                    &format!("{field}.{name}"),
                    "属性值应为布尔（byte）、整数（int）或字符串（string）",
                )
            })?;
            parsed.push(pv);
        }
        let nbt_type = parsed[0].prop_type();
        if let Some(bad) = parsed.iter().find(|v| v.prop_type() != nbt_type) {
            return Err(BlockJsonError::new(
                pack,
                file,
                &format!("{field}.{name}"),
                format!(
                    "同一属性的值类型必须一致：期望 {:?}，遇到 {:?}",
                    nbt_type,
                    bad.prop_type()
                ),
            ));
        }
        properties.push(PropertyDef {
            name,
            nbt_type,
            values: parsed,
        });
    }
    Ok(properties)
}

/// Parse one `.block.json` file (vanilla envelope; direct load, touches no global state).
///
/// ```json
/// {
///   "format_version": "1.10.0",
///   "minecraft:block": {
///     "description": { "identifier": "minecraft:oak_log",
///                      "states": { "pillar_axis": ["y", "x", "z"] } },
///     "components": { "minecraft:destructible_by_mining": { "value": 2.0 } },
///     "permutations": [
///       { "condition": "q.block_property('minecraft:oak_log','pillar_axis') == 'y'",
///         "components": { "minecraft:light_dampening": 15 } }
///     ],
///     "sc:protocol_runtime_ids": [100, 101, 102]
///   }
/// }
/// ```
///
/// The default state is the first declared value of every property (vanilla convention;
/// canonical index 0) and carries no explicit declaration — the former `sc:default_state`
/// field is retired and ignored if present. Server-required data with no vanilla
/// counterpart lives under `sc:` and is always explicit: protocol runtime ids (never
/// recomputed from sorting). `description.states` names the state table;
/// `description.default_states` is accepted as an alias with identical semantics.
/// Everything else loads as-is: unknown fields and components are kept raw, client-side
/// ignore-list entries are dropped, loot-table references resolve at bundle load (see
/// [`crate::block::load_block_json_bundle`]).
pub fn parse_block_file(
    pack: &str,
    path: &str,
    bytes: &[u8],
) -> Result<ParsedBlockFile, BlockJsonError> {
    let e = |field: &str, message: String| BlockJsonError::new(pack, path, field, message);
    let root: Value = serde_json::from_reader(BudgetReader::new(bytes))
        .map_err(|ex| e("$", format!("JSON 解析失败: {ex}")))?;
    let block_value = root
        .get("minecraft:block")
        .ok_or_else(|| e("$.minecraft:block", "必填（原版方块定义体）".to_string()))?;
    let block = as_object(pack, path, "$.minecraft:block", block_value)?;

    let description = block
        .get("description")
        .ok_or_else(|| e("$.minecraft:block.description", "必填".to_string()))?;
    let description = as_object(pack, path, "$.minecraft:block.description", description)?;
    let identifier = description
        .get("identifier")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            e(
                "$.minecraft:block.description.identifier",
                "必填（方块 identifier）".to_string(),
            )
        })?;
    // Missing `states` loads as a property-less block (direct load; explicit `{}` also works).
    // `default_states` is an alias for the same state table (pack-compat; identical semantics).
    let properties = match description.get("states") {
        Some(states_value) => {
            parse_states(pack, path, "$.minecraft:block.description.states", states_value)?
        }
        None => match description.get("default_states") {
            Some(states_value) => parse_states(
                pack,
                path,
                "$.minecraft:block.description.default_states",
                states_value,
            )?,
            None => Vec::new(),
        },
    };
    let state_count = canonical_state_count(&properties);

    // ---- Default state: first declared value of every property (canonical index 0) ----
    // No explicit declaration exists in the format; the former `sc:default_state` field
    // is retired and, like any unknown field, simply ignored.
    let default_index = 0usize;

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
            .ok_or_else(|| BlockJsonError::new(pack, path, &format!("{ids_field}[{i}]"), "应为非负整数"))?;
        if n > u32::MAX as u64 {
            return Err(BlockJsonError::new(
                pack,
                path,
                &format!("{ids_field}[{i}]"),
                format!("{n} 超出 u32"),
            ));
        }
        network_ids.push(n as u32);
    }

    // ---- components (raw JSON; ignore-list entries dropped) ----
    let base = match block.get("components") {
        None => BTreeMap::new(),
        Some(v) => component_layer(pack, path, "$.minecraft:block.components", v)?,
    };

    // ---- permutations (restricted condition syntax) ----
    let mut permutations = Vec::new();
    if let Some(perms) = block.get("permutations") {
        let arr = perms
            .as_array()
            .ok_or_else(|| e("$.minecraft:block.permutations", "应为数组".to_string()))?;
        for (i, item) in arr.iter().enumerate() {
            let field = format!("$.minecraft:block.permutations[{i}]");
            let obj = as_object(pack, path, &field, item)?;
            let condition_src = obj
                .get("condition")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    BlockJsonError::new(pack, path, &format!("{field}.condition"), "必填（字符串）")
                })?;
            let condition = parse_permutation_condition(
                pack,
                path,
                &format!("{field}.condition"),
                identifier,
                &properties,
                condition_src,
            )?;
            let components = match obj.get("components") {
                None => BTreeMap::new(),
                Some(v) => component_layer(pack, path, &format!("{field}.components"), v)?,
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

/// Client-side ignore-list entries never reach the component layers (re-checked here for tests).
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PACK: &str = "test-pack";

    fn air_json() -> Vec<u8> {
        r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {"sc:replaceable": true, "minecraft:display_name": "Air"},
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
                    "states": {"pillar_axis": ["y", "x", "z"]}
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
                "sc:protocol_runtime_ids": [100, 101, 102]
            }
        }"#
        .as_bytes()
        .to_vec()
    }

    #[test]
    fn direct_load_accepts_air_and_log() {
        let air = parse_block_file(PACK, "any/air.block.json", &air_json()).expect("air 应加载");
        assert_eq!(air.identifier, "minecraft:air");
        assert!(air.properties.is_empty());
        assert_eq!(air.state_count(), 1);
        assert_eq!(air.network_ids, vec![0]);
        // Client-side entries are dropped and never enter base.
        assert!(!air.base.contains_key("minecraft:display_name"));
        let replaceable = air.base.get("sc:replaceable").cloned();
        assert_eq!(replaceable, Some(json!(true)));

        let log = parse_block_file(PACK, "any/oak_log.block.json", &log_json()).expect("log 应加载");
        assert_eq!(log.properties.len(), 1);
        assert_eq!(log.properties[0].name, "pillar_axis");
        assert_eq!(log.properties[0].nbt_type, NbtPropType::String);
        assert_eq!(log.state_count(), 3);
        // Default state is the first item in declaration value order ('y' is declared first).
        assert_eq!(log.default_index, 0);
        assert_eq!(log.state_key(0), "pillar_axis=y");
        assert_eq!(log.network_ids, vec![100, 101, 102]);
        // Permutation condition evaluation.
        assert_eq!(log.permutations.len(), 1);
        let perm = &log.permutations[0];
        assert!(perm.condition.matches(&log.state_props(0)));
        assert!(!perm.condition.matches(&log.state_props(1)));
        assert!(!perm.condition.matches(&log.state_props(2)));
        // Loot references stay raw path strings at parse time.
        assert_eq!(
            perm.components.get("minecraft:loot"),
            Some(&json!("loot_tables/blocks/oak_log.json"))
        );
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
    }

    /// Both vanilla `description.states` spellings are accepted; mixed types are rejected.
    #[test]
    fn state_property_forms_and_types_are_parsed() {
        let json_body = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "test:forms",
                    "states": [
                        {"name": "lit", "values": [false, true]},
                        {"name": "age", "values": [0, 1, 2]}
                    ]
                },
                "sc:protocol_runtime_ids": [1, 2, 3, 4, 5, 6]
            }
        }"#;
        let parsed =
            parse_block_file(PACK, "definitions/blocks/test/forms.block.json", json_body.as_bytes())
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
        // Default state = first declared value of every property (age=0, lit=false).
        assert_eq!(parsed.default_index, 0);

        // Mixed types in one property are rejected (data-model coherence).
        let mixed = json_body.replace("\"values\": [false, true]", "\"values\": [false, 1]");
        let e = parse_block_file(
            PACK,
            "definitions/blocks/test/forms.block.json",
            mixed.as_bytes(),
        )
        .expect_err("混合类型必须拒绝");
        assert!(e.to_string().contains("值类型必须一致"), "实际：{e}");
    }

    /// Restricted permutation-condition syntax: `&&` conjunctions accepted, the rest loudly rejected.
    #[test]
    fn condition_grammar_is_restricted_and_loud() {
        let properties = vec![PropertyDef {
            name: "pillar_axis".to_string(),
            nbt_type: NbtPropType::String,
            values: vec![
                PropValue::String("x".to_string()),
                PropValue::String("y".to_string()),
            ],
        }];
        let ok = |src: &str| {
            parse_permutation_condition(PACK, "f", "$.c", "minecraft:oak_log", &properties, src)
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
            parse_permutation_condition(PACK, "f", "$.c", "minecraft:oak_log", &properties, bad)
                .expect_err(&format!("应拒绝：{bad}"));
        }
    }

    /// Required server data (`sc:protocol_runtime_ids`) stays loud; the retired
    /// `sc:default_state` field is ignored (default state = first declared values).
    #[test]
    fn protocol_runtime_ids_stay_required_and_default_state_is_implicit() {
        let air = String::from_utf8(air_json()).unwrap();
        // Missing sc:protocol_runtime_ids.
        let bad = air.replace(",\n                \"sc:protocol_runtime_ids\": [0]", "");
        let e = parse_block_file(PACK, "any/air.block.json", bad.as_bytes())
            .expect_err("缺 protocol id 必须拒绝");
        assert!(e.to_string().contains("sc:protocol_runtime_ids"), "实际：{e}");
        // Protocol id count mismatches the state-combination count.
        let log = String::from_utf8(log_json()).unwrap();
        let bad = log.replace(
            "\"sc:protocol_runtime_ids\": [100, 101, 102]",
            "\"sc:protocol_runtime_ids\": [100, 101]",
        );
        let e = parse_block_file(PACK, "any/oak_log.block.json", bad.as_bytes())
            .expect_err("protocol id 数量不符必须拒绝");
        assert!(e.to_string().contains("一一对应"), "实际：{e}");
        // Retired `sc:default_state` is tolerated and ignored: default stays index 0
        // (first declared value, here 'y'), never the retired field's assignment.
        let legacy = log.replace(
            "\"sc:protocol_runtime_ids\": [100, 101, 102]",
            "\"sc:default_state\": {\"pillar_axis\": \"z\"},\n                \"sc:protocol_runtime_ids\": [100, 101, 102]",
        );
        let parsed = parse_block_file(PACK, "any/oak_log.block.json", legacy.as_bytes())
            .expect("过期的 sc:default_state 字段应被忽略");
        assert_eq!(parsed.default_index, 0);
        assert_eq!(parsed.state_key(0), "pillar_axis=y");
    }

    /// `description.default_states` is accepted as the state table (pack-compat alias)
    /// with no default-state field at all: the default state is the first declared value.
    #[test]
    fn default_states_alias_loads() {
        let json_body = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "test:alias",
                    "default_states": {
                        "button_pressed_bit": [false, true],
                        "facing_direction": [0, 1, 2, 3, 4, 5]
                    }
                },
                "sc:protocol_runtime_ids": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
            }
        }"#;
        let parsed = parse_block_file(
            PACK,
            "definitions/blocks/test/alias.block.json",
            json_body.as_bytes(),
        )
        .expect("default_states 写法应合法");
        assert_eq!(parsed.properties.len(), 2);
        // Property names ascending: button_pressed_bit < facing_direction;
        // the rightmost property varies fastest.
        assert_eq!(parsed.properties[0].name, "button_pressed_bit");
        assert_eq!(parsed.properties[1].name, "facing_direction");
        assert_eq!(parsed.state_count(), 12);
        assert_eq!(
            parsed.state_key(0),
            "button_pressed_bit=0,facing_direction=0"
        );
        assert_eq!(parsed.state_key(11), "button_pressed_bit=1,facing_direction=5");
        // Default state = first declared value of every property (canonical index 0).
        assert_eq!(parsed.default_index, 0);
        assert_eq!(parsed.network_ids.len(), 12);
    }

    /// Direct load: unknown components and `ur:*` stay raw in the map (no rejection).
    #[test]
    fn unknown_components_load_raw() {
        let air = String::from_utf8(air_json()).unwrap();
        let extended = air.replace(
            "\"sc:replaceable\": true",
            "\"sc:replaceable\": true, \"ur:fly\": {}, \"plugin:thing\": {\"x\": 1}",
        );
        let parsed = parse_block_file(PACK, "any/air.block.json", extended.as_bytes())
            .expect("未知组件直接加载");
        assert_eq!(parsed.base.get("ur:fly"), Some(&json!({})));
        assert_eq!(parsed.base.get("plugin:thing"), Some(&json!({"x": 1})));
        // Ignore-list entries are dropped, not kept.
        assert!(parsed.base.get("minecraft:display_name").is_none());
    }
}
