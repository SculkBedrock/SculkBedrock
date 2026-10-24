//! 整数朝向处理器（`facing_direction`: 0-5）。
//!
//! 包含墙挂方块（梯子/按钮/避雷针）、漏斗、活塞、发射器/投掷器/木桶等。
//!
//! Integer facing handler (`facing_direction`: 0-5).
//!
//! Covers wall-mounted blocks (ladder/button/lightning rod), hoppers, pistons, and
//! dispensers/droppers/barrels, etc.

use crate::block::facing::context::PlacementContext;
use crate::block::facing::face::BlockFace;
use crate::block::facing::modifier::BlockStateModifier;
use sc_block::state::BlockPropertyValue;

fn is_wall_mounted(name: &str) -> bool {
    name == "minecraft:ladder"
        || name == "minecraft:end_rod"
        || name == "minecraft:lightning_rod"
        || name.contains("wall_sign")
        || name.contains("hanging_sign")
        || name.ends_with("_button")
}

fn is_hopper(name: &str) -> bool {
    name == "minecraft:hopper"
}

fn is_piston(name: &str) -> bool {
    name == "minecraft:piston" || name == "minecraft:sticky_piston"
}

/// 尝试应用整数 6 向朝向规则。
///
/// Applies the integer 6-way facing rule when the block has `facing_direction`.
pub fn try_apply(m: &mut BlockStateModifier, ctx: &PlacementContext) -> bool {
    if !m.has_property("facing_direction") {
        return false;
    }
    let name = &m.block_name;
    let facing = if is_wall_mounted(name) {
        // Wall-mounted: attaches to the face the player clicked.
        ctx.clicked_face.index() as i32
    } else if is_hopper(name) {
        // Hopper: opposite of the clicked face; a top/bottom click gives DOWN (0).
        if ctx.clicked_face == BlockFace::Down || ctx.clicked_face == BlockFace::Up {
            0
        } else {
            ctx.clicked_face.opposite().index() as i32
        }
    } else if is_piston(name) {
        // Piston: head points away from the player (along the look direction), vertical
        // follows the eye rule.
        ctx.eye_facing(ctx.look_face()).index() as i32
    } else {
        // Others (dispenser/dropper/barrel, etc.): front faces the player (opposite of
        // the look direction with the vertical eye override).
        ctx.eye_facing(ctx.look_face().opposite()).index() as i32
    };

    m.set_property("facing_direction", BlockPropertyValue::Int(facing));
    m.apply_upside_down(ctx.is_upside_down());
    true
}
