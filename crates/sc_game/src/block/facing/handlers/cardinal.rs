//! 方位朝向处理器（`minecraft:cardinal_direction`）。
//!
//! 适用于熔炉、箱子、雕刻台、切石机、门等方块。
//!
//! Cardinal facing handler (`minecraft:cardinal_direction`).
//!
//! Applies to furnaces, chests, cartography tables, stonecutters, doors, etc.

use crate::block::facing::context::PlacementContext;
use crate::block::facing::modifier::BlockStateModifier;
use sc_block::state::BlockPropertyValue;

/// 尝试应用方位朝向：正面朝向玩家（视线反面）。
///
/// Applies the cardinal facing: the front faces the player (opposite of the look
/// direction).
pub fn try_apply(m: &mut BlockStateModifier, ctx: &PlacementContext) -> bool {
    if !m.has_property("minecraft:cardinal_direction") {
        return false;
    }
    let facing = ctx.look_face().opposite();
    m.set_property(
        "minecraft:cardinal_direction",
        BlockPropertyValue::Enum(facing.as_cardinal_str().into()),
    );
    true
}
