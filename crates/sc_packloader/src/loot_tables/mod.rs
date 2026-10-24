//! 战利品表路径引用与直接表解析。
//!
//! Loot-table path references and direct table parsing.
//!
//! 方块与实体通过路径引用战利品表（`minecraft:loot`，支持裸字符串与
//! `{"table": path}` 对象两种写法）；本模块拥有引用模型与表 JSON 解析。
//! 路径到字节的解析由调用方注入（持有包来源的加载器），因此同一批辅助
//! 函数可同时服务版本包与行为包。掉落语义（池/条目编译与掷骰）位于
//! `sc_block::mining_drops`。
//!
//! Blocks and entities reference tables by path (`minecraft:loot`, both the
//! bare-string and the `{"table": path}` object spellings); this module owns
//! the reference model and the table JSON parsing. Resolution of a path to
//! bytes is injected by the caller (the loader that owns the pack source), so
//! the same helpers serve version packs and behavior packs. Drop semantics
//! (pool/entry compilation and rolling) live in `sc_block::mining_drops`.
//!
//! 解耦：本模块不导入方块域。
//!
//! Decoupling: this module never imports the block domain.

use sc_ecs::component::Component;
use serde_json::Value;

use crate::version_control::json_budget::BudgetReader;
use json_comments::StripComments;

/// 战利品表路径引用（`minecraft:loot`）；被引用的文件由加载方解析，
/// 并编译进各状态的掉落配置（见 `sc_block::mining_drops`）。
///
/// Loot-table path reference (`minecraft:loot`); the referenced file is parsed
/// by the loading pack and compiled into the per-state drops profile (see
/// `sc_block::mining_drops`).
///
/// 两种写法均可反序列化（与其他包加载组件同一约定，如
/// `minecraft:collision_box`）：裸路径字符串
/// （`"loot_tables/blocks/oak_log.json"`）或对象形式
/// （`{"table": "loot_tables/blocks/oak_log.json"}`）。
///
/// Both spellings deserialize (same convention as the other packloader components,
/// e.g. `minecraft:collision_box`): a bare path string
/// (`"loot_tables/blocks/oak_log.json"`) or the object form
/// (`{"table": "loot_tables/blocks/oak_log.json"}`).
///
/// 注册的组件键（`NAME`）来自 `BlockComponents` 的 `components_export!` 列表。
///
/// The registered component key (`NAME`) comes from the
/// `BlockComponents` `components_export!` list.
#[derive(Clone, Component, Debug)]
pub struct Loot(pub Box<str>);

impl<'de> serde::Deserialize<'de> for Loot {
    /// 从裸字符串或 `{"table": path}` 对象反序列化；两者都不是时报错。
    ///
    /// Deserializes from a bare string or a `{"table": path}` object; errors
    /// when the value is neither.
    ///
    /// - `deserializer`: serde 反序列化器
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        loot_reference_path(Some(&value))
            .map(|path| Loot(path.into_boxed_str()))
            .ok_or_else(|| {
                serde::de::Error::custom("应为路径字符串或 {\"table\": 路径} 对象".to_string())
            })
    }
}

/// 值为战利品表文件引用的原版组件名。
///
/// The vanilla component whose value references a loot-table file.
pub const LOOT_REFERENCE_COMPONENT: &str = "minecraft:loot";

/// 从两种写法提取 `minecraft:loot` 引用路径：裸字符串或
/// `{"table": path}` 对象形式；两者都不是时为 `None`。
///
/// `minecraft:loot` reference path from either spelling: a bare string, or the
/// `{"table": path}` object form. `None` when the value is neither.
pub(crate) fn loot_reference_path(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(path) => Some(path.clone()),
        Value::Object(map) => match map.get("table") {
            Some(Value::String(path)) => Some(path.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// 将一个战利品表文件（JSON 字节）解析为原始 JSON 值。
///
/// Parse one loot-table file (JSON bytes) into its raw JSON value.
///
/// 直接加载：会剥离注释（包自带注释）并套用标准包 JSON 读取预算；
/// 其余内容原样处理。仅当文件完全不是可解析 JSON 时返回 `None`。
///
/// Direct load: comments are stripped (packs ship them) and the standard
/// pack JSON reader budget applies; anything else is taken as-is.
/// Returns `None` only when the file is not parseable JSON at all.
///
/// - `bytes`: 战利品表文件字节
pub fn parse_loot_tables(bytes: &[u8]) -> Option<Value> {
    serde_json::from_reader(BudgetReader::new(StripComments::new(bytes))).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TABLE: &[u8] = br#"{
        // packs ship comments
        "pools": [
            {
                "rolls": 1,
                "entries": [ { "type": "item", "name": "minecraft:stick" } ]
            }
        ]
    }"#;

    #[test]
    fn parse_loot_tables_loads_json_directly() {
        let value = parse_loot_tables(TABLE).expect("应解析");
        assert_eq!(value["pools"][0]["entries"][0]["name"], "minecraft:stick");
        // Not JSON at all -> None, never a panic.
        assert!(parse_loot_tables(b"{oops").is_none());
    }

/// `minecraft:loot` 接受两种写法（与其他包加载组件同一约定）：
/// 裸路径字符串与 `{"table": path}` 对象形式反序列化得到同一路径。
///
/// `minecraft:loot` accepts both spellings (same convention as the other
/// packloader components): the bare path string and the `{"table": path}`
/// object form deserialize to the same path.
    #[test]
    fn loot_reference_accepts_string_and_object_forms() {
        let string_form: Loot =
            serde_json::from_value(json!("loot_tables/blocks/oak_log.json")).expect("字符串形式");
        let object_form: Loot = serde_json::from_value(json!({
            "table": "loot_tables/blocks/oak_log.json"
        }))
        .expect("对象形式");
        assert_eq!(&*string_form.0, "loot_tables/blocks/oak_log.json");
        assert_eq!(object_form.0, string_form.0);
        // Neither spelling -> loud error (never a silent empty path).
        assert!(serde_json::from_value::<Loot>(json!(7)).is_err());
        assert!(serde_json::from_value::<Loot>(json!({"nope": "x"})).is_err());
    }
}
