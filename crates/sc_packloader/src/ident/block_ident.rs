use crate::types::block_states::MinecraftBlockStates;
use serde::Deserialize;

crate::define_ident!(
    MinecraftBlockIdent,
    BlockName,
    Block(MinecraftBlock),
    default = "minecraft:air"
);

#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftBlock {
    pub name: String,
    pub states: MinecraftBlockStates,
}
