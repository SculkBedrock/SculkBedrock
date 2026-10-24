//! 通用的小型包数据类型（每种类型只有一种反序列化形态）。
//!
//! Small shared pack types with a single deserialization shape each.
//!
//! 将原 `types/*` 微模块统一收敛到此处；旧模块路径（`types::molang`、
//! `types::trigger` 等）保留为薄再导出，既有 `use` 路径不受影响。
//!
//! Consolidates the former `types/*` micro-modules in one place. The old
//! module paths (`types::molang`, `types::trigger`, ...) stay valid as thin
//! re-export shims, so existing `use` paths keep working.

use crate::types::filter::MinecraftFilterType;
use serde::Deserialize;
use serde_json::Value;

/// Molang 表达式（无类型 JSON 值或空值）。
///
/// Molang expression (untyped JSON value or null).
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(untagged)]
pub enum MolangExpression {
    /// 空表达式（`null`）。
    ///
    /// Empty expression (`null`).
    #[default]
    Null,
    /// 无类型的 JSON 值表达式。
    ///
    /// Untyped JSON value expression.
    ValueExpression(Value),
}

/// 方块状态映射占位（状态表存于 `sc_block` 快照中）。
///
/// Block states map placeholder (state tables live in `sc_block` snapshots).
#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftBlockStates {}

/// 事件触发器（事件名 + 目标选择器）。
///
/// Event trigger (event name plus target selector).
#[derive(Debug, Deserialize, Default, Clone)]
pub struct MinecraftTrigger {
    /// 事件名称。
    ///
    /// Event name.
    #[serde(default)]
    pub event: String,
    /// 目标选择器。
    ///
    /// Target selector.
    #[serde(default)]
    pub target: String,
}

/// 带过滤条件的事件（触发器 + 实体过滤器）。
///
/// Filtered event (trigger plus entity filter).
#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftEvent {
    /// 触发前需满足的过滤条件。
    ///
    /// Filter conditions that must pass before firing.
    #[serde(default)]
    pub filters: MinecraftFilterType,
    /// 事件名称。
    ///
    /// Event name.
    #[serde(default)]
    pub event: String,
    /// 目标选择器。
    ///
    /// Target selector.
    #[serde(default)]
    pub target: String,
}

/// 方块指定器（仅名称，或名称 + 显式状态）。
///
/// Block specifier (name or name plus explicit states).
#[derive(Clone, Deserialize, Debug)]
#[serde(untagged)]
pub enum MinecraftBlockSpecifiers {
    /// 仅给出方块名称。
    ///
    /// Only the block name is given.
    Name(String),
    /// 名称加显式状态的完整指定器。
    ///
    /// A full specifier with name and explicit states.
    BlockSpecifier(MinecraftBlockSpecifier),
}

/// 结构化方块指定器（名称 + 可选状态）。
///
/// Structured block specifier (name plus optional states).
#[derive(Clone, Deserialize, Debug)]
pub struct MinecraftBlockSpecifier {
    /// 方块标识名。
    ///
    /// Block identifier name.
    pub name: String,
    /// 可选的显式方块状态。
    ///
    /// Optional explicit block states.
    #[serde(default)]
    pub states: Option<MinecraftBlockStates>,
}
