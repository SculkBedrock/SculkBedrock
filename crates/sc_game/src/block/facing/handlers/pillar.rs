//! 柱轴朝向处理器（`pillar_axis`）。
//!
//! 适用于原木、木桩、玄武岩、锁链等方块。
//!
//! Pillar-axis facing handler (`pillar_axis`).
//!
//! Applies to logs, wood, basalt, chains, etc.

use crate::block::facing::context::PlacementContext;
use crate::block::facing::modifier::BlockStateModifier;
use sc_block::state::BlockPropertyValue;

/// 尝试应用柱轴朝向：根据玩家点击的面设置 X/Y/Z 轴。
///
/// Applies the pillar axis: sets the X/Y/Z axis from the face the player clicked.
pub fn try_apply(m: &mut BlockStateModifier, ctx: &PlacementContext) -> bool {
    if !m.has_property("pillar_axis") {
        return false;
    }
    let axis_str = ctx.clicked_face.axis().as_str();
    m.set_property("pillar_axis", BlockPropertyValue::Enum(axis_str.into()));
    true
}
