//! 结构填充器接口与沙漠水井、沼泽小屋等结构实现。
//!
//! Structure populator interface and implementations such as desert wells and swamp huts.

pub mod normal;
pub mod placement;
pub mod structures;

use crate::worldgen::context::ChunkGenerateContext;

// ---------------------------------------------------------------------------
// Populator trait
// ---------------------------------------------------------------------------

/// 结构填充器接口：生成结构方块并通过队列对象写入区块。
///
/// Structure populator interface generating structure blocks and queuing them for the chunk.
pub trait Populator: Send + Sync {
    /// Returns the populator name.
    fn name(&self) -> &'static str;

    /// Runs the populator against the chunk context.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>);
}

// ---------------------------------------------------------------------------
// PopulatorStructure
// ---------------------------------------------------------------------------

/// Returns whether structure generation is enabled (generator setting
/// "structures", default true).
pub fn should_generate_structures(ctx: &ChunkGenerateContext<'_>) -> bool {
    ctx.structures
}
