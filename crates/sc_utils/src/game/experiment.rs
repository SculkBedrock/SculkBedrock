use crate::nbt::deserialize_nbt_bool;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

/// Represents the "experiments" compound in world.dat NBT.
/// Only stores metadata flags; actual experiment toggles come from ServerProperties.
#[serde_inline_default]
#[derive(Deserialize, Debug, Clone, Default)]
pub struct ExperimentFlags {
    #[serde_inline_default(false)]
    #[serde(alias = "saved_with_toggled_experiments")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub saved_with_toggled_experiments: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "experiments_ever_used")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub experiments_ever_used: bool,
}

#[derive(Clone, Debug)]
pub struct ExperimentData {
    pub name: &'static str,
    pub is_enabled: bool,
}

impl ExperimentData {
    pub const DATA_DRIVEN_ITEMS: ExperimentData = ExperimentData::new("data_driven_items", false);
    pub const DATA_DRIVEN_BIOMES: ExperimentData = ExperimentData::new("data_driven_biomes", false);
    pub const UPCOMING_CREATOR_FEATURES: ExperimentData =
        ExperimentData::new("upcoming_creator_features", false);
    pub const GAMETEST: ExperimentData = ExperimentData::new("gametest", false);
    pub const EXPERIMENTAL_MOLOANG_FEATURE: ExperimentData =
        ExperimentData::new("experimental_molang_feature", false);
    pub const CAMERAS: ExperimentData = ExperimentData::new("cameras", false);
    pub const fn new(name: &'static str, is_enabled: bool) -> Self {
        Self { name, is_enabled }
    }

    pub fn set_enabled(mut self, is_enabled: bool) -> Self {
        self.is_enabled = is_enabled;
        self
    }
}
