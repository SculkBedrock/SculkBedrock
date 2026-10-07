//! Unified ingredient / output model.
//!
//! Stage-1 field support: item identifier, `data`/damage, count and item
//! tags. Anything that needs full item components / NBT (user-data blob,
//! `components` conditions, potion userdata, ...) is **not** silently
//! ignored: the compiler marks the recipe `unsupported` with an explicit
//! reason.

use serde::{Deserialize, Serialize};

/// Sentinel for "any data value" (`data` absent or 32767 on the wire).
pub const DATA_WILDCARD: i32 = 32767;

/// A single ingredient alternative.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum IngredientChoice {
    /// Exact item identifier (e.g. `minecraft:iron_ingot`).
    Item {
        identifier: String,
        /// `None` = wildcard (field absent). `Some(v)` = exact data value.
        data: Option<i32>,
    },
    /// Item tag (e.g. `minecraft:planks`). Expansion happens at match time
    /// against the caller-provided tag resolver.
    Tag { tag: String },
}

/// Split BedrockBeta `"<namespace>:<name>:<data>"` sugar
/// (e.g. furnace `"input": "minecraft:log:0"`, `"output": "minecraft:coal:1"`).
///
/// Only when `data` is absent and the tail parses as `i32`; non-numeric
/// tails (`minecraft:potion_type:water`) and plain identifiers are left
/// untouched. Without this, `id:data` strings never match anything: exact
/// comparison fails and no tag family is registered under the suffixed name.
fn split_legacy_id_data(identifier: &str, data: Option<i32>) -> (String, Option<i32>) {
    if data.is_some() {
        return (identifier.to_string(), data);
    }
    let mut parts = identifier.rsplitn(2, ':');
    let tail = parts.next().unwrap_or("");
    let head = parts.next().unwrap_or("");
    if head.contains(':') {
        if let Ok(value) = tail.parse::<i32>() {
            return (head.to_string(), Some(value));
        }
    }
    (identifier.to_string(), data)
}

impl IngredientChoice {
    pub fn identifier(&self) -> String {
        match self {
            Self::Item { identifier, .. } => identifier.clone(),
            Self::Tag { tag } => format!("tag:{tag}"),
        }
    }
}

/// One ingredient slot: exactly one of the choices must match, consuming
/// `count` items.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IngredientSpec {
    pub choices: Vec<IngredientChoice>,
    pub count: u16,
}

impl IngredientSpec {
    pub fn single_item(identifier: impl Into<String>, data: Option<i32>, count: u16) -> Self {
        let identifier = identifier.into();
        let identifier = if identifier.contains(':') {
            identifier
        } else {
            format!("minecraft:{identifier}")
        };
        let (identifier, data) = split_legacy_id_data(&identifier, data);
        Self {
            choices: vec![IngredientChoice::Item { identifier, data }],
            count: count.max(1),
        }
    }

    pub fn single_tag(tag: impl Into<String>, count: u16) -> Self {
        Self {
            choices: vec![IngredientChoice::Tag { tag: tag.into() }],
            count: count.max(1),
        }
    }
}

/// One output stack.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OutputSpec {
    pub identifier: String,
    pub count: u16,
    /// `None` = default (0 on the wire). Preserved for matching/wire.
    pub data: Option<i32>,
}

impl OutputSpec {
    pub fn new(identifier: impl Into<String>, count: u16, data: Option<i32>) -> Self {
        let identifier = identifier.into();
        let identifier = if identifier.contains(':') {
            identifier
        } else {
            format!("minecraft:{identifier}")
        };
        let (identifier, data) = split_legacy_id_data(&identifier, data);
        Self {
            identifier,
            count: count.max(1),
            data,
        }
    }

    pub fn wire_data(&self) -> u32 {
        self.data.unwrap_or(0).max(0) as u32
    }
}

/// Parse one ingredient value which may be:
/// - `"minecraft:stone"` (string shorthand)
/// - `{"item": "...", "data"?, "count"?}`
/// - `{"tag": "...", "count"?}`
pub fn parse_ingredient(
    value: &serde_json::Value,
    ctx: &str,
) -> Result<(IngredientSpec, Vec<String>), String> {
    let mut unknown_fields = Vec::new();
    match value {
        serde_json::Value::String(name) => Ok((
            IngredientSpec::single_item(name.clone(), None, 1),
            unknown_fields,
        )),
        serde_json::Value::Object(map) => {
            for key in map.keys() {
                match key.as_str() {
                    "item" | "tag" | "data" | "count" | "damage" | "aux" => {}
                    _ => unknown_fields.push(format!("{ctx}.{key}")),
                }
            }
            let count = map
                .get("count")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .min(u16::MAX as u64) as u16;
            let data = map
                .get("data")
                .or_else(|| map.get("damage"))
                .or_else(|| map.get("aux"))
                .and_then(|v| v.as_i64())
                .map(|v| v as i32);
            if let Some(item) = map.get("item").and_then(|v| v.as_str()) {
                if map.contains_key("tag") {
                    return Err(format!("{ctx}: 'item' and 'tag' are mutually exclusive"));
                }
                // Any other object content (components/NBT/userdata) makes the
                // ingredient conditional on data we cannot evaluate yet.
                return Ok((
                    IngredientSpec::single_item(item.to_string(), data, count.max(1)),
                    unknown_fields,
                ));
            }
            if let Some(tag) = map.get("tag").and_then(|v| v.as_str()) {
                return Ok((
                    IngredientSpec::single_tag(tag.to_string(), count.max(1)),
                    unknown_fields,
                ));
            }
            Err(format!("{ctx}: ingredient needs 'item' or 'tag'"))
        }
        _ => Err(format!("{ctx}: ingredient must be a string or object")),
    }
}

/// Parse one output value which may be a string or an object.
/// Arrays are handled by the caller (multi-output).
pub fn parse_output(
    value: &serde_json::Value,
    ctx: &str,
) -> Result<(OutputSpec, Vec<String>), String> {
    let mut unknown_fields = Vec::new();
    match value {
        serde_json::Value::String(name) => {
            Ok((OutputSpec::new(name.clone(), 1, None), unknown_fields))
        }
        serde_json::Value::Object(map) => {
            for key in map.keys() {
                match key.as_str() {
                    "item" | "data" | "count" | "damage" | "aux" => {}
                    _ => unknown_fields.push(format!("{ctx}.{key}")),
                }
            }
            let Some(item) = map.get("item").and_then(|v| v.as_str()) else {
                return Err(format!("{ctx}: output object needs 'item'"));
            };
            let count = map
                .get("count")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .min(u16::MAX as u64) as u16;
            let data = map
                .get("data")
                .or_else(|| map.get("damage"))
                .or_else(|| map.get("aux"))
                .and_then(|v| v.as_i64())
                .map(|v| v as i32);
            Ok((
                OutputSpec::new(item.to_string(), count.max(1), data),
                unknown_fields,
            ))
        }
        _ => Err(format!("{ctx}: output must be a string or object")),
    }
}

/// Returns true when the raw JSON object carries fields that require full
/// item components / NBT to evaluate (`components`, `nbt`, `user_data`,
/// `can_place_on`, ...). Such recipes must be quarantined, never matched
/// by ignoring the condition.
pub fn requires_components(value: &serde_json::Value) -> Option<String> {
    let map = value.as_object()?;
    for key in [
        "components",
        "nbt",
        "user_data",
        "userdata",
        "can_place_on",
        "can_destroy",
        "lock_in_inventory",
    ] {
        if map.contains_key(key) {
            return Some(key.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_and_object_forms() {
        let (spec, _) = parse_ingredient(&serde_json::json!("minecraft:stone"), "in").unwrap();
        assert_eq!(spec.count, 1);
        let (spec, _) = parse_ingredient(
            &serde_json::json!({"item": "minecraft:stone", "data": 5}),
            "in",
        )
        .unwrap();
        assert!(matches!(
            &spec.choices[0],
            IngredientChoice::Item { data: Some(5), .. }
        ));
        let (spec, _) = parse_ingredient(
            &serde_json::json!({"tag": "minecraft:planks", "count": 2}),
            "in",
        )
        .unwrap();
        assert_eq!(spec.count, 2);
        assert!(parse_ingredient(&serde_json::json!({"item": "a", "tag": "b"}), "in").is_err());
    }

    #[test]
    fn legacy_id_data_sugar_splits_only_with_numeric_tail() {
        // Legacy furnace form: "input": "minecraft:log:0" becomes (log, 0).
        let (spec, _) = parse_ingredient(&serde_json::json!("minecraft:log:0"), "in").unwrap();
        assert!(matches!(
            &spec.choices[0],
            IngredientChoice::Item { identifier, data: Some(0) }
                if identifier == "minecraft:log"
        ));
        // Legacy output form: "output": "minecraft:coal:1" (charcoal) becomes (coal, 1).
        let (out, _) = parse_output(&serde_json::json!("minecraft:coal:1"), "out").unwrap();
        assert_eq!(out.identifier, "minecraft:coal");
        assert_eq!(out.data, Some(1));
        // Non-numeric suffixes stay untouched (brewing potion types).
        let (spec, _) =
            parse_ingredient(&serde_json::json!("minecraft:potion_type:water"), "in").unwrap();
        assert!(matches!(
            &spec.choices[0],
            IngredientChoice::Item { identifier, data: None }
                if identifier == "minecraft:potion_type:water"
        ));
        // Leaves entries with an existing data field unsplit.
        let (spec, _) = parse_ingredient(
            &serde_json::json!({"item": "minecraft:log", "data": 2}),
            "in",
        )
        .unwrap();
        assert!(matches!(
            &spec.choices[0],
            IngredientChoice::Item { identifier, data: Some(2) }
                if identifier == "minecraft:log"
        ));
    }
}
