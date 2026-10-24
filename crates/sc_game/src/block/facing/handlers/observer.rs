//! 侦测器命名朝向处理器（`minecraft:facing_direction`）。
//!
//! Observer named-facing handler (`minecraft:facing_direction`).

use crate::block::facing::context::PlacementContext;
use crate::block::facing::modifier::BlockStateModifier;
use sc_block::state::BlockPropertyValue;

/// 尝试应用侦测器朝向：沿视线方向（检测面背离玩家），竖直方向遵循视线规则。
///
/// Applies the observer facing: along the look direction (the detecting face points
/// away from the player), vertical follows the eye rule.
pub fn try_apply(m: &mut BlockStateModifier, ctx: &PlacementContext) -> bool {
    if !m.has_property("minecraft:facing_direction") {
        return false;
    }
    let facing = ctx.eye_facing(ctx.look_face());
    m.set_property(
        "minecraft:facing_direction",
        BlockPropertyValue::Enum(facing.as_observer_str().into()),
    );
    true
}
