//! 火把朝向处理器（`torch_facing_direction`）。
//!
//! Torch facing handler (`torch_facing_direction`).

use crate::block::facing::context::PlacementContext;
use crate::block::facing::modifier::BlockStateModifier;
use sc_block::state::BlockPropertyValue;

/// 尝试应用火把朝向：顶面为 top，侧面为面名，天花板保持默认。
///
/// Applies the torch facing: top face gives `top`, side faces give the face name, the
/// ceiling keeps the default.
pub fn try_apply(m: &mut BlockStateModifier, ctx: &PlacementContext) -> bool {
    if !m.has_property("torch_facing_direction") {
        return false;
    }
    if let Some(name) = ctx.clicked_face.as_torch_str() {
        m.set_property(
            "torch_facing_direction",
            BlockPropertyValue::Enum(name.into()),
        );
    }
    true
}
