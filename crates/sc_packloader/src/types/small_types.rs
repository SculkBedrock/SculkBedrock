//! Small shared pack types with a single deserialization shape each.
//!
//! Consolidates the former `types/*` micro-modules (each under 30 lines) in
//! one place. The old module paths (`types::molang`, `types::trigger`, ...)
//! stay valid as thin re-export shims, so existing `use` paths keep working.

use crate::types::filter::MinecraftFilterType;
use serde::Deserialize;
use serde_json::Value;

/// Molang expression (untyped JSON value or null).
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(untagged)]
pub enum MolangExpression {
    #[default]
    Null,
    ValueExpression(Value),
}

/// Block states map placeholder (state tables live in `sc_block` snapshots).
#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftBlockStates {}

/// Event trigger (event name plus target selector).
#[derive(Debug, Deserialize, Default, Clone)]
pub struct MinecraftTrigger {
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub target: String,
}

/// Filtered event (trigger plus entity filter).
#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftEvent {
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub target: String,
}

/// Block specifier (name or name plus explicit states).
#[derive(Clone, Deserialize, Debug)]
#[serde(untagged)]
pub enum MinecraftBlockSpecifiers {
    Name(String),
    BlockSpecifier(MinecraftBlockSpecifier),
}

/// Structured block specifier (name plus optional states).
#[derive(Clone, Deserialize, Debug)]
pub struct MinecraftBlockSpecifier {
    pub name: String,
    #[serde(default)]
    pub states: Option<MinecraftBlockStates>,
}
