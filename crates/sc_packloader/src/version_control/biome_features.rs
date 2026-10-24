//! `definitions/biome_features.json` 的反序列化结构（世界生成特性调度数据）。
//!
//! Deserialization shape for definitions/biome_features.json (worldgen feature-scheduling data).
//!
//! 来源：`biome_definitions.nbt`（1.26.40）中各生物群系的
//! `chunkGenData.consolidatedFeatures.features`，由
//! `.tools/gamedata/extract_biome_features.py` 抽取为独立 JSON
//! （枚举序数映射为名称）。
//!
//! Source: per-biome `chunkGenData.consolidatedFeatures.features` in `biome_definitions.nbt` (1.26.40),
//! extracted to standalone JSON by `.tools/gamedata/extract_biome_features.py` (enum ordinals mapped to names).
//!
//! 包加载器只做 IO 与反序列化；特性调度语义（eval_order 排序、
//! 标识符到 GenerateFeature 注册表匹配）位于世界生成
//! （对应 NormalChunkFeatureStage 调度阶段）。
//!
//! Packloader only does IO and deserialization; feature-scheduling semantics (eval_order sorting,
//! identifier-to-GenerateFeature registry matching) live in world generation
//! (matching the NormalChunkFeatureStage scheduling stage).
//!
//! 字段名与 `BiomeConsolidatedFeatureData`/`BiomeScatterParamData`/
//! `BiomeCoordinateData` 一一对应。
//!
//! Field names match `BiomeConsolidatedFeatureData`/`BiomeScatterParamData`/
//! `BiomeCoordinateData` one to one.

use serde::Deserialize;
use std::collections::HashMap;

/// 根结构：`{"biomes": {"minecraft:xxx": {"features": [...]}}}`。
///
/// Root shape: `{"biomes": {"minecraft:xxx": {"features": [...]}}}`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BiomeFeaturesData {
    /// 生物群系标识（带 `minecraft:` 前缀，与 biome_definitions.nbt 的
    /// biomeStringList 一致）到该生物群系特性调度列表的映射。
    ///
    /// Biome identifier (with `minecraft:` prefix, matching biome_definitions.nbt
    /// biomeStringList) to the feature-scheduling list for that biome.
    pub biomes: HashMap<String, BiomeFeatureList>,
}

/// 单个生物群系的特性列表。
///
/// Feature list for one biome.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BiomeFeatureList {
    /// 特性条目列表。
    ///
    /// Feature entries.
    pub features: Vec<ConsolidatedFeatureData>,
}

/// 镜像 `BiomeConsolidatedFeatureData`（NBT: chunkGenData.consolidatedFeatures.features[]）。
///
/// Mirrors `BiomeConsolidatedFeatureData` (NBT: chunkGenData.consolidatedFeatures.features[]).
#[derive(Debug, Clone, Deserialize)]
pub struct ConsolidatedFeatureData {
    /// 调度标识符（如 "minecraft:forest_surface_trees_feature"）：
    /// 匹配特性注册表的首选键。
    ///
    /// Scheduling identifier (e.g. "minecraft:forest_surface_trees_feature"):
    /// preferred match key into the feature registry.
    pub identifier: String,
    /// 全局特性名（如 "minecraft:legacy:forest_tree_feature"）：
    /// 标识符未命中时的回退匹配键。
    ///
    /// Global feature name (e.g. "minecraft:legacy:forest_tree_feature"):
    /// fallback match key when the identifier misses.
    pub feature: String,
    /// 调度阶段（"surface_pass"/"after_surface_pass"/"underground_pass"/...）。
    ///
    /// Scheduling pass ("surface_pass"/"after_surface_pass"/"underground_pass"/...).
    pub pass: String,
    /// 是否允许使用内部特性。
    ///
    /// Whether internal features may be used.
    pub can_use_internal_feature: bool,
    /// 散布参数。
    ///
    /// Scatter parameters.
    pub scatter: ScatterParamData,
}

/// 镜像 `BiomeScatterParamData`（NBT: scatter）。
///
/// Mirrors `BiomeScatterParamData` (NBT: scatter).
#[derive(Debug, Clone, Deserialize)]
pub struct ScatterParamData {
    /// 坐标求值顺序（"XYZ"/"XZY"/...）；特性按其序数排序。
    ///
    /// Coordinate evaluation order ("XYZ"/"XZY"/...); features sort by its ordinal.
    pub eval_order: String,
    /// ExpressionOp 名称（如 "FLOAT"）。
    ///
    /// ExpressionOp name (e.g. "FLOAT").
    pub chance_percent_type: String,
    /// 百分比概率。
    ///
    /// Percentage chance.
    pub chance_percent: i16,
    /// 概率分子。
    ///
    /// Chance numerator.
    pub chance_numerator: i32,
    /// 概率分母。
    ///
    /// Chance denominator.
    pub chance_denominator: i32,
    /// ExpressionOp 名称。
    ///
    /// ExpressionOp name.
    pub iterations_type: String,
    /// 迭代次数。
    ///
    /// Iteration count.
    pub iterations: i16,
    /// 每个 x/y/z 轴一条，顺序与 NBT coordinates 列表一致。
    ///
    /// One entry per x/y/z axis, in NBT coordinates list order.
    pub coordinates: Vec<CoordinateData>,
}

/// 镜像 `BiomeCoordinateData`（NBT: scatter.coordinates[]）。
///
/// Mirrors `BiomeCoordinateData` (NBT: scatter.coordinates[]).
#[derive(Debug, Clone, Deserialize)]
pub struct CoordinateData {
    /// "x" | "y" | "z"（由抽取脚本按列表顺序标注）。
    ///
    /// "x" | "y" | "z" (labeled in list order by the extraction script).
    pub axis: String,
    /// 最小值表达式的 ExpressionOp 名称（可空）。
    ///
    /// ExpressionOp name for the minimum expression (optional).
    pub min_value_type: Option<String>,
    /// 最小值。
    ///
    /// Minimum value.
    pub min_value: i16,
    /// 最大值表达式的 ExpressionOp 名称（可空）。
    ///
    /// ExpressionOp name for the maximum expression (optional).
    pub max_value_type: Option<String>,
    /// 最大值。
    ///
    /// Maximum value.
    pub max_value: i16,
    /// 网格偏移。
    ///
    /// Grid offset.
    pub grid_offset: i64,
    /// 网格步长。
    ///
    /// Grid step size.
    pub grid_step_size: i64,
    /// RandomDistributionType 名称（"UNIFORM"/"GAUSSIAN"/...）。
    ///
    /// RandomDistributionType name ("UNIFORM"/"GAUSSIAN"/...).
    pub distribution: Option<String>,
}
