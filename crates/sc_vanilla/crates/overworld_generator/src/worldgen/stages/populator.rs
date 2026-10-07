//! Structure populator stage for the Normal stage chain.
//!
//! Runs the registered structure populators for the chunk, buffering block
//! writes through the generation context and flushing them once at the end.
//! Only code-generated structures are registered here; template-based
//! structures are covered by the populator modules (see `populator/normal.rs`).

use std::sync::Arc;

use crate::worldgen::chunk::ChunkState;
use crate::worldgen::context::ChunkGenerateContext;
use crate::worldgen::populator::normal::{DesertWellPopulator, SwampHutPopulator};
use crate::worldgen::populator::structures::StructureBlockTable;
use crate::worldgen::populator::Populator;
use crate::worldgen::stages::GenerateStage;

// ---------------------------------------------------------------------------
// NormalPopulatorStage: runs structure populators and flushes buffered writes
// ---------------------------------------------------------------------------

/// Structure populator stage.
///
/// Apply semantics:
/// 1. Marks the chunk as populated;
/// 2. Runs each populator in order (block writes go through the context queue into the root buffer);
/// 3. Flushes the root buffer to the chunk once at the end.
pub struct NormalPopulatorStage {
    populators: Vec<Box<dyn Populator>>,
}

impl NormalPopulatorStage {
    /// Builds the stage with the code-generated structure populators.
    pub fn new(table: Arc<StructureBlockTable>) -> Self {
        let populators: Vec<Box<dyn Populator>> = vec![
            Box::new(DesertWellPopulator::new(table.clone())),
            Box::new(SwampHutPopulator::new(table)),
        ];
        Self { populators }
    }
}

impl GenerateStage for NormalPopulatorStage {
    /// Runs all populators for the chunk and flushes buffered writes.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        ctx.chunk.set_state(ChunkState::Populated);
        for populator in &self.populators {
            populator.apply(ctx);
            if ctx.spillover_overflowed() {
                return;
            }
        }
        // Flush buffered writes to the chunk.
        ctx.apply_root_to_chunk();
    }

    /// Stage name used for chain lookup.
    fn name(&self) -> &'static str {
        "normal_populator"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::chunk::WorldgenChunk;
    use crate::worldgen::holder::normal::NormalObjectHolder;
    use crate::worldgen::material::MaterialBlocks;
    use crate::worldgen::random::Xoroshiro128;
    use sc_world::chunk::{BlockRuntimeId, ChunkPosition};

    fn test_material_blocks() -> MaterialBlocks {
        let id = |n: u32| BlockRuntimeId(n);
        MaterialBlocks {
            air: id(0),
            water: id(4),
            lava: id(28),
            stone: id(1),
            granite: id(29),
            tuff: id(30),
            copper_ore: id(31),
            deepslate_iron_ore: id(32),
            raw_copper_block: id(33),
            raw_iron_block: id(34),
        }
    }

    /// Dummy block table for stage-flow tests (structure shapes are covered
    /// with explicit id tables elsewhere; this only checks the stage flow).
    fn test_stage() -> NormalPopulatorStage {
        NormalPopulatorStage::new(Arc::new(StructureBlockTable::from_core_palette()))
    }

    #[test]
    fn populator_stage_sets_populated() {
        let stage = test_stage();
        let holder = NormalObjectHolder::new(Xoroshiro128::new(42), test_material_blocks());
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);
        stage.apply(&mut ctx);
        assert_eq!(ctx.chunk.state(), ChunkState::Populated);
    }

    #[test]
    fn populator_stage_name() {
        assert_eq!(test_stage().name(), "normal_populator");
    }

    #[test]
    fn structures_disabled_skips_populators() {
        // With structures disabled, apply returns early without writing blocks.
        let stage = test_stage();
        let holder = NormalObjectHolder::new(Xoroshiro128::new(42), test_material_blocks());
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);
        ctx.structures = false;
        stage.apply(&mut ctx);
        assert_eq!(ctx.chunk.state(), ChunkState::Populated);
    }
}
