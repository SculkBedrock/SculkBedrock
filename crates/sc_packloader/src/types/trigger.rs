use serde::Deserialize;

#[derive(Debug, Deserialize, Default, Clone)]
pub struct MinecraftTrigger {
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub target: String,
}
