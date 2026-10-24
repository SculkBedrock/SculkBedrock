//! `noise_settings` 解析与 router 根提取。
//!
//! `noise_settings` parsing and router-root extraction.
//!
//! 覆盖密度语义：单元格 `noise` 块与每个 `noise_router` 根都按密度值解析。
//! 材质/地表规则、含水层、生成目标与调试函数按**原始 JSON 冻结**，供地形计划
//! 构建使用——它们参与指纹但在此不做解释。未知顶层键是显式错误。
//!
//! Covers the density semantics: the cell `noise` block and every
//! `noise_router` root are parsed as density values. The material/surface
//! rule, aquifers, spawn targets and debug functions are **frozen as raw
//! JSON** for the terrain plan builder — they are fingerprinted but not
//! interpreted here. Unknown top-level keys are loud errors.

use crate::data::json::{parse_strict, Json};
use crate::data::node::{parse_node, Node, NodeBudget, ParseError};
use crate::sources::{WorldgenCategory, WorldgenSources};
use std::collections::BTreeMap;

/// 解析后的 `noise_settings`（仅密度范围）。
///
/// Parsed `noise_settings` (density-scope only).
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseSettings {
    /// `noise.min_y`（世界底部）。
    ///
    /// `noise.min_y` (world bottom).
    pub min_y: i64,
    /// `noise.height`（世界高度，单位方块）。
    ///
    /// `noise.height` (world height in blocks).
    pub height: i64,
    /// 每个 `noise_router` 根，已解析为（链接前）密度值。
    ///
    /// Every `noise_router` root as a parsed (pre-link) density value.
    pub router: BTreeMap<String, Node>,
    /// 非密度块（material_rule、aquifers、spawn_target、debug_functions、
    /// default_block/default_fluid、flags）的原始 JSON，冻结供后续阶段使用。
    ///
    /// Raw JSON of the non-density blocks (material_rule, aquifers,
    /// spawn_target, debug_functions, default_block/default_fluid, flags),
    /// frozen for later stages.
    pub frozen: BTreeMap<String, Json>,
}

/// noise_settings 解析失败。
///
/// noise_settings parse failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    /// JSON 解析失败。
    ///
    /// JSON parse failure.
    Parse(ParseError),
    /// 未知顶层键（26.3 schema 是封闭的）。
    ///
    /// Unknown top-level key (26.3 schema is closed).
    UnknownField { path: String },
    /// 必填结构字段缺失或格式错误。
    ///
    /// A required structural field is missing or malformed.
    Structure { path: String, detail: String },
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "{error}"),
            Self::UnknownField { path } => write!(f, "{path}: unknown noise_settings field"),
            Self::Structure { path, detail } => write!(f, "{path}: {detail}"),
        }
    }
}

impl std::error::Error for SettingsError {}

impl From<ParseError> for SettingsError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

/// 26.3 `noise_settings` schema 的顶层键（取自随包 bundle）。密度范围的键
/// 被解析，其余按原样冻结。
///
/// Top-level keys of the 26.3 `noise_settings` schema (surveyed from the
/// shipped bundle). Density-scope keys are parsed; the rest are frozen raw.
pub const KNOWN_SETTINGS_KEYS: &[&str] = &[
    "noise",
    "noise_router",
    "sea_level",
    "disable_mob_generation",
    "legacy_random_source",
    "aquifers",
    "debug_functions",
    "default_block",
    "default_fluid",
    "material_rule",
    "spawn_target",
];

/// 解析一个 noise_settings JSON 值。
///
/// Parses a noise_settings JSON value.
///
/// - `json`: 待解析的 JSON 值
pub fn parse_noise_settings(json: &Json) -> Result<NoiseSettings, SettingsError> {
    let root_path = "$";
    if let Json::Obj(fields) = json {
        for (key, _) in fields {
            if !KNOWN_SETTINGS_KEYS.contains(&key.as_str()) {
                return Err(SettingsError::UnknownField { path: format!("{root_path}.{key}") });
            }
        }
    } else {
        return Err(SettingsError::Structure {
            path: root_path.to_string(),
            detail: "expected an object".to_string(),
        });
    }

    let noise_json = json
        .get("noise")
        .ok_or_else(|| SettingsError::Structure {
            path: format!("{root_path}.noise"),
            detail: "missing".to_string(),
        })?;
    let min_y = noise_json
        .get("min_y")
        .and_then(Json::as_i64)
        .ok_or_else(|| SettingsError::Structure {
            path: format!("{root_path}.noise.min_y"),
            detail: "missing or non-integer".to_string(),
        })?;
    let height = noise_json
        .get("height")
        .and_then(Json::as_i64)
        .ok_or_else(|| SettingsError::Structure {
            path: format!("{root_path}.noise.height"),
            detail: "missing or non-integer".to_string(),
        })?;
    if height <= 0 {
        return Err(SettingsError::Structure {
            path: format!("{root_path}.noise.height"),
            detail: format!("height must be positive, got {height}"),
        });
    }

    let router_json = json
        .get("noise_router")
        .ok_or_else(|| SettingsError::Structure {
            path: format!("{root_path}.noise_router"),
            detail: "missing".to_string(),
        })?;
    let Json::Obj(router_fields) = router_json else {
        return Err(SettingsError::Structure {
            path: format!("{root_path}.noise_router"),
            detail: "expected an object".to_string(),
        });
    };
    let mut budget = NodeBudget::new(8192);
    let mut router = BTreeMap::new();
    for (key, value) in router_fields {
        let node = parse_node(value, &format!("{root_path}.noise_router.{key}"), &mut budget)?;
        router.insert(key.clone(), node);
    }

    let mut frozen = BTreeMap::new();
    for (key, value) in json.get_obj_fields() {
        if key != "noise" && key != "noise_router" {
            frozen.insert(key.clone(), value.clone());
        }
    }

    Ok(NoiseSettings { min_y, height, router, frozen })
}

impl Json {
    fn get_obj_fields(&self) -> &[(String, Json)] {
        match self {
            Json::Obj(fields) => fields,
            _ => &[],
        }
    }
}

/// 从 bundle 读取并解析一个 noise_settings 资源。
///
/// Reads and parses a noise_settings resource from the bundle.
///
/// - `sources`: 世界生成 bundle 源
/// - `resource`: noise_settings 资源 id
pub fn load_noise_settings(
    sources: &WorldgenSources,
    resource: &str,
) -> Result<NoiseSettings, SettingsError> {
    let bytes = sources
        .resolve(WorldgenCategory::NoiseSettings, resource)
        .ok_or_else(|| SettingsError::Structure {
            path: format!("$[{resource}]"),
            detail: "noise_settings resource missing from the bundle".to_string(),
        })?
        .clone();
    let json = parse_strict(&bytes)
        .map_err(|error| SettingsError::Parse(error.into()))?;
    parse_noise_settings(&json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_router_roots_and_freezes_material_rule() {
        let json = parse_strict(
            br#"{
                "sea_level": 63,
                "disable_mob_generation": false,
                "legacy_random_source": false,
                "aquifers": {"enabled": true},
                "debug_functions": [],
                "default_block": {"Name": "minecraft:stone"},
                "default_fluid": {"Name": "minecraft:water"},
                "material_rule": {"sequence": []},
                "spawn_target": [],
                "noise": {"min_y": -64, "height": 384},
                "noise_router": {
                    "continents": "minecraft:overworld/continents",
                    "final_density": {"type": "minecraft:add", "left": 1, "right": 2}
                }
            }"#,
        )
        .unwrap();
        let settings = parse_noise_settings(&json).unwrap();
        assert_eq!(settings.min_y, -64);
        assert_eq!(settings.height, 384);
        assert_eq!(settings.router.len(), 2);
        assert!(matches!(
            settings.router.get("final_density"),
            Some(Node::Add(_, _))
        ));
        assert_eq!(settings.frozen.len(), 9);
        assert!(settings.frozen.contains_key("material_rule"));
    }

    #[test]
    fn unknown_fields_and_missing_structure_are_loud() {
        let json = parse_strict(br#"{"noise":{"min_y":0,"height":1},"noise_router":{},"mystery":1}"#)
            .unwrap();
        assert!(matches!(
            parse_noise_settings(&json),
            Err(SettingsError::UnknownField { .. })
        ));
        let json = parse_strict(br#"{"noise":{"min_y":0,"height":-5},"noise_router":{}}"#).unwrap();
        assert!(matches!(
            parse_noise_settings(&json),
            Err(SettingsError::Structure { .. })
        ));
    }
}
