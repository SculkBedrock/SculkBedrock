use crate::types::filter::MinecraftFilterType;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(untagged)]
pub enum MinecraftEntityTypes {
    #[default]
    Null,
    Vec(Vec<MinecraftEntityType>),
    Single(MinecraftEntityType),
}

#[serde_inline_default]
#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftEntityType {
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(16.0)]
    pub max_dist: f64,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde_inline_default(3.0)]
    pub must_see_forget_duration: f64,
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    #[serde_inline_default(1.0)]
    pub sprint_speed_multiplier: f64,
    #[serde_inline_default(1.0)]
    pub walk_speed_multiplier: f64,
    #[serde(default)]
    pub priority: i32,
}
