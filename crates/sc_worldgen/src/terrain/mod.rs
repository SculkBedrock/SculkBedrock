//! 地形生成阶段：提供从编译后的密度计划产出 [`BaseSnapshot`] 的主基地形
//! 生成器。
//!
//! Terrain generation stages: provides the primary base terrain generator
//! producing [`BaseSnapshot`]s from compiled density plans.

pub mod base;

pub use base::{BaseBlockPalette, BaseTerrainGenerator, CELL_SIZE_XZ, CELL_SIZE_Y, DEFAULT_SEA_LEVEL};
