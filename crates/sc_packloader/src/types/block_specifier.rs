use crate::types::block_states::MinecraftBlockStates;
use serde::Deserialize;

#[derive(Clone, Deserialize, Debug)]
#[serde(untagged)]
pub enum MinecraftBlockSpecifiers {
    Name(String),
    BlockSpecifier(MinecraftBlockSpecifier),
}

#[derive(Clone, Deserialize, Debug)]
pub struct MinecraftBlockSpecifier {
    pub name: String,
    #[serde(default)]
    pub states: Option<MinecraftBlockStates>,
}
