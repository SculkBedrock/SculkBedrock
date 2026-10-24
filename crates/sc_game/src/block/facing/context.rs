//! 方块放置上下文（PlacementContext）。
//!
//! 统一收拢放置时玩家位置、偏航角、眼高、目标格位置、点击面及格内高度分量。
//!
//! Block placement context (PlacementContext).
//!
//! Collects the placing player's position, yaw, eye height, target cell, clicked face
//! and in-cell height fraction in one value.

use super::face::BlockFace;
use sc_block::position::BlockPosition;
use sc_entity::motion::Position;

/// 方块放置上下文，用于驱动数据驱动的放置朝向计算。
///
/// Placement context that drives the data-driven placement orientation calculation.
#[derive(Clone, Debug)]
pub struct PlacementContext {
    /// 玩家点击的目标方块表面。
    ///
    /// The target block face the player clicked.
    pub clicked_face: BlockFace,
    /// 玩家当前脚部位置。
    ///
    /// The player's current feet position.
    pub player_pos: Position,
    /// 方块将要放置的目标格坐标。
    ///
    /// The target cell where the block will be placed.
    pub target_pos: BlockPosition,
    /// 玩家眼高（默认 1.62）。
    ///
    /// The player's eye height (default 1.62).
    pub eye_height: f32,
    /// 玩家身体偏航角（度）。
    ///
    /// The player's body yaw in degrees.
    pub player_yaw: f32,
    /// 协议携带的格内点击垂直分量（0.0..=1.0），用于区分台阶/楼梯/活板门的上下半部点击。
    ///
    /// Protocol in-cell vertical click fraction (0.0..=1.0), distinguishing the upper
    /// and lower half of slabs/stairs/trapdoors.
    pub click_frac_y: Option<f32>,
}

impl PlacementContext {
    /// 创建一个新的放置上下文。
    ///
    /// Creates a new placement context.
    pub fn new(
        clicked_face: BlockFace,
        player_pos: Position,
        target_pos: BlockPosition,
        eye_height: f32,
        player_yaw: f32,
        click_frac_y: Option<f32>,
    ) -> Self {
        Self {
            clicked_face,
            player_pos,
            target_pos,
            eye_height,
            player_yaw,
            click_frac_y,
        }
    }

    /// 由玩家当前偏航角计算视线水平朝向。
    ///
    /// Computes the horizontal look face from the player's current yaw.
    #[inline]
    pub fn look_face(&self) -> BlockFace {
        BlockFace::from_yaw(self.player_yaw)
    }

    /// 是否应放置为倒置状态（楼梯/活板门的 `upside_down_bit`）。
    ///
    /// 规则：点击方块底面（DOWN），或点击侧面的上半部（fraction > 0.5）。
    ///
    /// Whether the block should be placed upside down (stairs/trapdoor
    /// `upside_down_bit`).
    ///
    /// Rule: the clicked face is DOWN, or a side face was clicked in its upper half
    /// (fraction > 0.5).
    pub fn is_upside_down(&self) -> bool {
        self.clicked_face == BlockFace::Down
            || (self.clicked_face != BlockFace::Up
                && self.click_frac_y.is_some_and(|frac| frac.is_finite() && frac > 0.5))
    }

    /// 6 向方块的视线规则（发射器/投掷器/活塞等）：
    /// 当玩家在目标格水平 2 格以内时做竖直覆盖（眼睛高于目标 2 格以上则向上，眼睛低于目标则向下），否则保持水平方向。
    ///
    /// Six-way look rule (dispenser/dropper/piston, etc.): when the player is within
    /// 2 blocks horizontally of the target, a vertical override applies (eyes more than
    /// 2 above the target faces UP, eyes below the target faces DOWN); otherwise the
    /// horizontal face is kept.
    pub fn eye_facing(&self, horizontal: BlockFace) -> BlockFace {
        let feet_y = self.player_pos.y;
        if !feet_y.is_finite() || !self.eye_height.is_finite() {
            return horizontal;
        }
        let near = (self.player_pos.x - (self.target_pos.x as f32 + 0.5)).abs() < 2.0
            && (self.player_pos.z - (self.target_pos.z as f32 + 0.5)).abs() < 2.0;
        if !near {
            return horizontal;
        }
        let eye_y = feet_y + self.eye_height;
        if eye_y - self.target_pos.y as f32 > 2.0 {
            BlockFace::Up
        } else if self.target_pos.y as f32 - eye_y > 0.0 {
            BlockFace::Down
        } else {
            horizontal
        }
    }

    /// 站立横幅方向（0-15），由 yaw 换算。
    ///
    /// Standing-banner direction (0-15) derived from yaw.
    pub fn sign_direction(&self) -> i32 {
        if !self.player_yaw.is_finite() {
            return 0;
        }
        let mut rotation = self.player_yaw % 360.0;
        if rotation < 0.0 {
            rotation += 360.0;
        }
        ((rotation + 180.0 + 11.25) / 22.5).floor() as i32 & 15
    }
}
