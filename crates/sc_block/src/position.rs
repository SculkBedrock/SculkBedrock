//! 整数方块坐标。
//!
//! Integer block coordinates.

use sc_world::chunk::ChunkPosition;
use std::fmt;

/// 世界中的整数方块坐标（方块粒度，非实体粒度）。
///
/// Integer block coordinates in the world (block granularity, not entity granularity).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct BlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPosition {
    /// 由三个整数分量构造方块坐标。
    ///
    /// Constructs a block position from three integer components.
    ///
    /// - `x` / `y` / `z`: 各轴整数分量
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// 把浮点实体坐标向下取整到其所在的方块。
    ///
    /// Floor floating-point entity coordinates to their containing block.
    ///
    /// - `x` / `y` / `z`: 浮点坐标（朝负无穷方向取整）
    pub fn from_float(x: f32, y: f32, z: f32) -> Self {
        Self::new(x.floor() as i32, y.floor() as i32, z.floor() as i32)
    }

    /// 返回该坐标所属的区块坐标。
    ///
    /// Returns the chunk position this coordinate belongs to.
    pub fn chunk_position(&self) -> ChunkPosition {
        ChunkPosition::from_world(self.x, self.z)
    }

    /// 返回区块内局部 X（0..16，负数坐标环绕）。
    ///
    /// Returns the in-chunk local X (0..16, negative coordinates wrap).
    pub fn local_x(&self) -> u8 {
        self.x.rem_euclid(16) as u8
    }

    /// 返回区块内局部 Z（0..16，负数坐标环绕）。
    ///
    /// Returns the in-chunk local Z (0..16, negative coordinates wrap).
    pub fn local_z(&self) -> u8 {
        self.z.rem_euclid(16) as u8
    }
}

impl fmt::Display for BlockPosition {
    /// 格式化为 `(x, y, z)`。
    ///
    /// Formats as `(x, y, z)`.
    ///
    /// - `formatter`: 输出格式化器
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "({}, {}, {})", self.x, self.y, self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates_map_to_floor_chunk_and_wrapped_local() {
        let position = BlockPosition::new(-1, 64, -17);
        assert_eq!(position.chunk_position(), ChunkPosition::new(-1, -2));
        assert_eq!(position.local_x(), 15);
        assert_eq!(position.local_z(), 15);
    }

    #[test]
    fn from_float_floors_toward_negative_infinity() {
        assert_eq!(
            BlockPosition::from_float(-0.5, 63.9, 16.0),
            BlockPosition::new(-1, 63, 16)
        );
    }
}
