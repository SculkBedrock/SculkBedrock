//! 生物群系选择器与生物群系 ID 表。
//!
//! Biome pickers and the biome ID table.

pub mod biome_id;
pub mod overworld;

/// 可变的生物群系查询结果：保存当前与原始生物群系 ID。
///
/// Mutable biome query result holding the current and original biome IDs.
pub trait BiomeResult {
    /// Returns the current biome id.
    fn biome_id(&self) -> i32;

    /// Overwrites the biome id (used by correction/reset).
    fn set_biome_id(&mut self, id: i32);
}

/// 在方块位置选择生物群系的接口。
///
/// Interface selecting the biome at a block position.
pub trait BiomePicker {
    type Result: BiomeResult;

    /// Picks the biome result for the given block coordinates.
    fn pick(&self, x: i32, y: i32, z: i32) -> Self::Result;
}
