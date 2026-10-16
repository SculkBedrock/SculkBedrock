//! Identifier/path validation, JSON hygiene, and component validation.
//!
//! Split from `schema.rs`; re-exported at `crate::block::schema` and `crate::block`.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;
use super::{BlockBundleBudgets, BlockJsonError, BLOCKS_DIRECTORY};

// ---------------------------------------------------------------------------
// Identifier/path contract
// ---------------------------------------------------------------------------

pub(super) fn is_ns_char(b: u8) -> bool {
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
// Component maps and parse-time helpers
// ---------------------------------------------------------------------------

pub(super) fn err(
    pack: &str,
    file: &str,
    field: impl Into<String>,
    message: impl Into<String>,
) -> BlockJsonError {
    BlockJsonError::new(pack, file, field.into(), message.into())
}

pub(super) fn as_object<'a>(
    pack: &str,
    file: &str,
    field: &str,
    v: &'a Value,
) -> Result<&'a serde_json::Map<String, Value>, BlockJsonError> {
    v.as_object()
        .ok_or_else(|| err(pack, file, field, "应为 JSON 对象"))
}

pub(super) fn check_no_unknown_keys(
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
pub(super) fn validate_components_map(
    pack: &str,
    file: &str,
    field: &str,
    v: &Value,
    budgets: &BlockBundleBudgets,
) -> Result<BTreeMap<String, Value>, BlockJsonError> {
    use crate::block::component::{
        is_ignored_component, is_known_component, validate_block_component, Drops,
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
pub(super) fn check_component_budgets(
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
pub(super) fn forbidden_key_char(c: char) -> bool {
    c.is_whitespace() || ",='\"\\".contains(c)
}

/// Property-name contract: non-empty, length-bounded, no forbidden chars.
pub(super) fn check_property_name(
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
pub(super) fn check_prop_string_value(
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
