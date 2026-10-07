use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MinecraftItemIdent {
    ItemName(String),
}

impl Default for MinecraftItemIdent {
    fn default() -> Self {
        Self::ItemName("".to_string())
    }
}
