use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MinecraftEntityIdent {
    EntityName(String),
}

impl Default for MinecraftEntityIdent {
    fn default() -> Self {
        Self::EntityName("".to_string())
    }
}
