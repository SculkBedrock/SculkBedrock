//! Deserialization shape for definitions/biome_features.json (worldgen feature-scheduling data).
//!
//! Source: per-biome `chunkGenData.consolidatedFeatures.features` in `biome_definitions.nbt` (1.26.40),
//! extracted to standalone JSON by `.tools/gamedata/extract_biome_features.py` (enum ordinals mapped to names).
//!
//! Packloader only does IO and deserialization; feature-scheduling semantics (eval_order sorting,
//! identifier-to-GenerateFeature registry matching) live in world generation
//! (matching the NormalChunkFeatureStage scheduling stage).
//!
//! Field names match `BiomeConsolidatedFeatureData`/`BiomeScatterParamData`/
//! `BiomeCoordinateData` one to one.

use serde::Deserialize;
use std::collections::HashMap;

/// Root shape: `{"biomes": {"minecraft:xxx": {"features": [...]}}}`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BiomeFeaturesData {
    /// Biome identifier (with `minecraft:` prefix, matching biome_definitions.nbt
    /// biomeStringList) to the feature-scheduling list for that biome.
    pub biomes: HashMap<String, BiomeFeatureList>,
}

/// Feature list for one biome.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BiomeFeatureList {
    pub features: Vec<ConsolidatedFeatureData>,
}

/// Mirrors `BiomeConsolidatedFeatureData` (NBT: chunkGenData.consolidatedFeatures.features[]).
#[derive(Debug, Clone, Deserialize)]
pub struct ConsolidatedFeatureData {
    /// Scheduling identifier (e.g. "minecraft:forest_surface_trees_feature"):
    /// preferred match key into the feature registry.
    pub identifier: String,
    /// Global feature name (e.g. "minecraft:legacy:forest_tree_feature"):
    /// fallback match key when the identifier misses.
    pub feature: String,
    /// Scheduling pass ("surface_pass"/"after_surface_pass"/"underground_pass"/...).
    pub pass: String,
    pub can_use_internal_feature: bool,
    pub scatter: ScatterParamData,
}

/// Mirrors `BiomeScatterParamData` (NBT: scatter).
#[derive(Debug, Clone, Deserialize)]
pub struct ScatterParamData {
    /// Coordinate evaluation order ("XYZ"/"XZY"/...); features sort by its ordinal.
    pub eval_order: String,
    /// ExpressionOp name (e.g. "FLOAT").
    pub chance_percent_type: String,
    pub chance_percent: i16,
    pub chance_numerator: i32,
    pub chance_denominator: i32,
    /// ExpressionOp name.
    pub iterations_type: String,
    pub iterations: i16,
    /// One entry per x/y/z axis, in NBT coordinates list order.
    pub coordinates: Vec<CoordinateData>,
}

/// Mirrors `BiomeCoordinateData` (NBT: scatter.coordinates[]).
#[derive(Debug, Clone, Deserialize)]
pub struct CoordinateData {
    /// "x" | "y" | "z" (labeled in list order by the extraction script).
    pub axis: String,
    pub min_value_type: Option<String>,
    pub min_value: i16,
    pub max_value_type: Option<String>,
    pub max_value: i16,
    pub grid_offset: i64,
    pub grid_step_size: i64,
    /// RandomDistributionType name ("UNIFORM"/"GAUSSIAN"/...).
    pub distribution: Option<String>,
}
