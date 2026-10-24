//! 放置朝向（facing）：数据驱动的状态选择。
//!
//! 按序应用朝向规则，每一步都以方块确实拥有对应属性为前提；
//! 未知方块保持默认状态，不猜测朝向。子模块：`face` 几何面与空间轴、
//! `context` 放置环境、`modifier` 状态变更构建器、`handlers` 各属性朝向规则处理器
//! （柱轴、楼梯、方位、整数朝向、火把等）。
//!
//! Placement orientation (facing): data-driven state selection.
//!
//! Orientation rules apply in order; each step assumes the block actually has the
//! matching property. Unknown blocks keep their default state and never guess an
//! orientation. Submodules: `face` (geometric faces and axes), `context` (placement
//! environment), `modifier` (state-change builder), `handlers` (per-property facing
//! rules: pillar axis, stairs, cardinal, integer facing, torch, etc.).

pub mod context;
pub mod face;
pub mod handlers;
pub mod modifier;

#[cfg(test)]
mod tests;

pub use context::PlacementContext;
pub use face::{Axis, BlockFace};
pub use modifier::BlockStateModifier;

use sc_block::registry::BlockStateRegistry;

/// 依据放置上下文解析应用朝向后的方块状态哈希。
///
/// Resolves the placed state hash with orientation applied from `ctx`.
pub fn orient_state(
    registry: &BlockStateRegistry,
    base_hash: u32,
    ctx: &PlacementContext,
) -> u32 {
    let Some(mut modifier) = BlockStateModifier::new(registry, base_hash) else {
        return base_hash;
    };

    let handlers: [&dyn Fn(&mut BlockStateModifier, &PlacementContext) -> bool; 8] = [
        &handlers::pillar::try_apply,
        &handlers::stairs::try_apply,
        &handlers::directional::try_apply,
        &handlers::trapdoor::try_apply,
        &handlers::cardinal::try_apply,
        &handlers::observer::try_apply,
        &handlers::torch::try_apply,
        &handlers::banner::try_apply,
    ];

    for handler in handlers {
        if handler(&mut modifier, ctx) {
            break;
        }
    }

    modifier.commit()
}
