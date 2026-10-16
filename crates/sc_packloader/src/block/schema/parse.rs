//! Single-file `.block.json` parsing (properties, conditions, files).
//!
//! Split from `schema.rs`; re-exported at `crate::block::schema` and `crate::block`.

use std::collections::{BTreeMap, HashSet};

use sc_nbt::NbtValue;
use serde_json::Value;
use super::{
    validate::{
        as_object, check_no_unknown_keys, check_prop_string_value, check_property_name, err,
        reject_duplicate_keys, validate_block_path, validate_components_map, validate_identifier,
    },
    BlockBundleBudgets, BlockJsonError, BLOCK_JSON_FORMAT_VERSION,
};

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
pub(super) fn canonical_stride(properties: &[PropertyDef], slot: usize) -> usize {
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

pub(super) fn read_quoted(src: &str) -> Result<(String, &str), String> {
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
pub(super) fn parse_states(
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
