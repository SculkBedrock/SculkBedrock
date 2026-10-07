use crate::types::block_states::MinecraftBlockStates;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MinecraftBlockIdent {
    BlockName(String),
    Block(MinecraftBlock),
}

impl Default for MinecraftBlockIdent {
    fn default() -> Self {
        MinecraftBlockIdent::BlockName("minecraft:air".to_string())
    }
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftBlock {
    pub name: String,
    pub states: MinecraftBlockStates,
}
