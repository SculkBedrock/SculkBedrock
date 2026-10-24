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
    /// 方块标识名（如 `minecraft:stone`）。
    ///
    /// Block identifier name (e.g. `minecraft:stone`).
    pub name: String,
    /// 该标识对应的方块状态集合。
    ///
    /// Block states attached to this identifier.
    pub states: MinecraftBlockStates,
}
