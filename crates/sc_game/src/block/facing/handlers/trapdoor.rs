//! 活板门朝向处理器（`direction` + `upside_down_bit`）。
//!
//! Trapdoor facing handler (`direction` + `upside_down_bit`).

use crate::block::facing::context::PlacementContext;
use crate::block::facing::modifier::BlockStateModifier;

fn is_trapdoor(name: &str) -> bool {
    name.contains("trapdoor")
}

/// 尝试应用活板门朝向：EWSN 罗盘序（视线反面）加倒置规则。
///
/// Applies the trapdoor facing: EWSN compass order (opposite of the look) plus the
/// upside-down rule.
pub fn try_apply(m: &mut BlockStateModifier, ctx: &PlacementContext) -> bool {
    if !m.has_property("direction") || !is_trapdoor(&m.block_name) {
        return false;
    }
    let trapdoor_dir = ctx.look_face().opposite().as_trapdoor_int();
    m.set_typed_numeric("direction", trapdoor_dir);
    m.apply_upside_down(ctx.is_upside_down());
    true
}
