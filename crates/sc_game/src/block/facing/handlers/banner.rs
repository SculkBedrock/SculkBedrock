//! 站立横幅朝向处理器（`ground_sign_direction`: 0-15）。
//!
//! Standing-banner facing handler (`ground_sign_direction`: 0-15).

use crate::block::facing::context::PlacementContext;
use crate::block::facing::modifier::BlockStateModifier;

/// 尝试应用横幅 16 向朝向：由偏航角换算。
///
/// Applies the banner's 16-way facing: derived from the yaw angle.
pub fn try_apply(m: &mut BlockStateModifier, ctx: &PlacementContext) -> bool {
    if !m.has_property("ground_sign_direction") {
        return false;
    }
    m.set_typed_numeric("ground_sign_direction", ctx.sign_direction());
    true
}
