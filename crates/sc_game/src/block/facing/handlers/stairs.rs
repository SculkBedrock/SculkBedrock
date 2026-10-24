//! 楼梯朝向处理器（`weirdo_direction` + `upside_down_bit`）。
//!
//! Stairs facing handler (`weirdo_direction` + `upside_down_bit`).

use crate::block::facing::context::PlacementContext;
use crate::block::facing::modifier::BlockStateModifier;
use sc_block::state::BlockPropertyValue;

/// 尝试应用楼梯朝向：方向取自玩家视线，倒置取自点击底面或侧面上半部。
///
/// Applies the stairs facing: direction from the player's look, upside-down from a
/// bottom-face click or the upper half of a side click.
pub fn try_apply(m: &mut BlockStateModifier, ctx: &PlacementContext) -> bool {
    if !m.has_property("weirdo_direction") {
        return false;
    }
    let weirdo = ctx.look_face().as_weirdo_int();
    m.set_property("weirdo_direction", BlockPropertyValue::Int(weirdo));
    m.apply_upside_down(ctx.is_upside_down());
    true
}
