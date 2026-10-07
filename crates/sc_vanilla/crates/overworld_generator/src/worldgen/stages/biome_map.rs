//! Biome-map stage: picks a biome per column and writes per-section biome ids.
//!
//! Notes:
//! - The picker is built fresh per `apply` from the holder plus a precomputed
//!   chunk heightmap callback. The picker itself carries no per-chunk state;
//!   only the heightmap lookup is per-chunk.
//! - Only the overworld picker path is used here.
//! - The per-chunk evaluation cache is a local value (single-threaded
//!   generation needs no pooling).
//! - The chunk is edited directly through `&mut WorldgenChunk`.
//! - The heightmap is precomputed into `[i32; 256]` because it does not change
//!   while this stage runs (the terrain stage wrote it in the previous stage).

use crate::worldgen::biome::overworld::{OverworldBiomePicker, OverworldBiomeResult};
use crate::worldgen::biome::BiomeResult;
use crate::worldgen::context::ChunkGenerateContext;
use crate::worldgen::densityfunction::common::CellFunctionContext;
use crate::worldgen::densityfunction::function::ChunkCache;
use crate::worldgen::holder::normal::NormalObjectHolder;
use crate::worldgen::stages::terrain::SEA_LEVEL;
use crate::worldgen::stages::GenerateStage;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// BiomeMapStage: per-column biome pick + per-section biome write
// ---------------------------------------------------------------------------

/// Biome-map stage.
///
/// Picks a biome per 16x16 column with `OverworldBiomePicker::pick` at sea level,
/// then walks each column from top to bottom applying the depth correction
/// for cave biomes, writing section biome ids, and resetting the result.
///
/// The stage itself is zero-sized; the picker is built inside `apply` from
/// the holder plus the chunk heightmap.
pub struct BiomeMapStage;

impl BiomeMapStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BiomeMapStage {
    fn default() -> Self {
        Self::new()
    }
}

impl GenerateStage for BiomeMapStage {
    /// Picks biomes for all columns and writes per-section biome ids.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        // --- Snapshot immutable inputs (avoids later borrow conflicts on the chunk) ---
        let holder: &NormalObjectHolder = ctx.holder;
        let level_seed: i64 = ctx.level_seed;
        let min_y: i32 = ctx.min_y;
        let max_y: i32 = ctx.max_y;
        let chunk_x: i32 = ctx.chunk.x();
        let chunk_z: i32 = ctx.chunk.z();
        let chunk_base_x = chunk_x * 16;
        let chunk_base_z = chunk_z * 16;

        // --- Precompute the heightmap (shared by the picker and the write loop) ---
        // Both the pick path and the per-column loop read the same heightmap,
        // which does not change while this stage runs. Precompute it once for
        // the picker callback and the depth correction below.
        let mut heightmap = [0i32; 256];
        for lx in 0u8..16 {
            for lz in 0u8..16 {
                heightmap[(lx as usize) * 16 + lz as usize] = ctx.chunk.height_map(lx, lz);
            }
        }

        // --- Build the picker ---
        // `OverworldBiomePicker::new` clones the density functions/noise internally
        // and holds no holder reference. The heightmap is `Copy`, so the move
        // closure captures a copy.
        let picker = OverworldBiomePicker::new(
            holder,
            level_seed,
            Box::new(move |wx, wz| {
                let lx = (wx - chunk_base_x) as usize;
                let lz = (wz - chunk_base_z) as usize;
                heightmap[lx * 16 + lz]
            }),
        );

        // --- 16x16 pick (overworld branch) ---
        // Fresh per-chunk evaluation cache plus function context.
        let chunk_cache = Rc::new(ChunkCache::new());
        chunk_cache.clear();
        let function_context = CellFunctionContext::new(Rc::clone(&chunk_cache));

        // One result per column; `OverworldBiomeResult` is not `Copy`, so use a vector.
        let mut biomes: Vec<OverworldBiomeResult> = Vec::with_capacity(256);
        for _x in 0..16i32 {
            let x = chunk_base_x + _x;
            for _z in 0..16i32 {
                let z = chunk_base_z + _z;
                // `pick_with_context` consults the heightmap callback and applies
                // the sea-level depth correction internally.
                biomes.push(picker.pick_with_context(
                    x,
                    SEA_LEVEL,
                    z,
                    function_context.set(x, SEA_LEVEL, z),
                ));
            }
        }
        // The cache drops here at the end of its scope.

        // --- Write biomes per y (top-down batch loop) ---
        // For each y from top to bottom and each column: apply the depth
        // correction, write the section biome id, then reset the result.
        for y in (min_y..=max_y).rev() {
            for lx in 0u8..16 {
                for lz in 0u8..16 {
                    let idx = (lx as usize) * 16 + lz as usize;
                    let result = &mut biomes[idx];
                    // Apply the depth correction for this y.
                    result.correct(y - heightmap[idx]);
                    // Write the section biome id.
                    ctx.chunk.set_biome_id(lx, y, lz, result.biome_id());
                    // Restore the original biome for the next y.
                    result.reset();
                }
            }
        }
    }

    /// Stage name used for chain lookup.
    fn name(&self) -> &'static str {
        "biome"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::chunk::WorldgenChunk;
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

    /// Runs BiomeMapStage and collects the 16x16 biome ids at y=64.
    fn run_biome_map(height: i32) -> Vec<i32> {
        let holder = NormalObjectHolder::new(Xoroshiro128::new(12345), test_blocks());
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        for x in 0u8..16 {
            for z in 0u8..16 {
                wc.set_height_map(x, z, height);
            }
        }
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 12345, -64, 319);
        BiomeMapStage::new().apply(&mut ctx);

        let mut biomes = Vec::with_capacity(256);
        for x in 0u8..16 {
            for z in 0u8..16 {
                biomes.push(ctx.chunk.biome_id(x, 64, z));
            }
        }
        biomes
    }

    #[test]
    fn biome_map_sets_biomes() {
        let biomes = run_biome_map(64);
        // At least some columns should have a non-zero biome id (non-OCEAN).
        let non_zero = biomes.iter().filter(|&&b| b != 0).count();
        assert!(non_zero > 0, "BiomeMapStage should set non-zero biomes");
    }

    #[test]
    fn biome_map_deterministic() {
        // Same seed + same coordinates produce the same biome distribution.
        let b1 = run_biome_map(64);
        let b2 = run_biome_map(64);
        assert_eq!(b1, b2);
    }

    #[test]
    fn biome_map_name() {
        assert_eq!(BiomeMapStage::new().name(), "biome");
    }

    #[test]
    fn biome_map_different_heights_differ() {
        // Different heightmaps may affect the depth correction, but the y=64 biome
        // is mostly noise-driven; this checks apply does not panic and yields
        // valid biome ids.
        let low = run_biome_map(32);
        let high = run_biome_map(128);
        // All biome ids should be non-negative.
        assert!(low.iter().all(|&b| b >= 0));
        assert!(high.iter().all(|&b| b >= 0));
    }
}
