//! 地形完成标记阶段：将区块状态置为已生成。
//!
//! Post-terrain marker stage: sets the chunk state to generated.

use crate::worldgen::chunk::ChunkState;
use crate::worldgen::context::ChunkGenerateContext;
use crate::worldgen::stages::GenerateStage;

// ---------------------------------------------------------------------------
// GeneratedStage: marks the chunk as terrain-generated
// ---------------------------------------------------------------------------

/// 地形生成完成标记阶段。
///
/// 将区块标记为地形已生成（状态变为 `Generated`），后续 populate/feature 阶段依赖该状态。
///
/// Marker stage for completed terrain generation.
///
/// Marks the chunk as terrain-generated (state becomes `Generated`).
/// Later populate/feature stages depend on this state.
pub struct GeneratedStage;

impl GeneratedStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GeneratedStage {
    fn default() -> Self {
        Self::new()
    }
}

impl GenerateStage for GeneratedStage {
    /// Marks the chunk as terrain-generated.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        ctx.chunk.set_state(ChunkState::Generated);

        // No dirty marker (`WorldgenChunk` carries none).
        // No pending sub-chunk updates (runtime networking layer).
    }

    /// Stage name used for chain lookup.
    fn name(&self) -> &'static str {
        "generated"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::chunk::{ChunkState, WorldgenChunk};
    use crate::worldgen::holder::normal::NormalObjectHolder;
    use crate::worldgen::material::MaterialBlocks;
    use crate::worldgen::random::Xoroshiro128;
    use sc_world::chunk::{BlockRuntimeId, ChunkPosition};

    fn test_blocks() -> MaterialBlocks {
        MaterialBlocks {
            air: BlockRuntimeId(0),
            water: BlockRuntimeId(1),
            lava: BlockRuntimeId(2),
            stone: BlockRuntimeId(3),
            granite: BlockRuntimeId(4),
            tuff: BlockRuntimeId(5),
            copper_ore: BlockRuntimeId(6),
            deepslate_iron_ore: BlockRuntimeId(7),
            raw_copper_block: BlockRuntimeId(8),
            raw_iron_block: BlockRuntimeId(9),
        }
    }

    #[test]
    fn generated_stage_sets_state() {
        let holder = NormalObjectHolder::new(Xoroshiro128::new(42), test_blocks());
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        assert_eq!(wc.state(), ChunkState::New);

        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);
        GeneratedStage.apply(&mut ctx);

        assert_eq!(ctx.chunk.state(), ChunkState::Generated);
        assert!(ctx.chunk.is_generated());
    }

    #[test]
    fn generated_stage_name() {
        assert_eq!(GeneratedStage.name(), "generated");
    }
}
