//! Surface-data stage: replaces the top block per column with biome surface material.
//!
//! Notes:
//! - Noise-based material adjustments are skipped; the hardcoded per-biome
//!   const table supplies the base material directly.
//! - Still water is identified via the block table's water id.
//! - The chunk is edited directly through `&mut WorldgenChunk`.
//! - The fill-depth noise uses local x/z coordinates.

use crate::blocks_table::{biome_surface_material, WorldgenBlockTable};
use crate::worldgen::context::ChunkGenerateContext;
use crate::worldgen::math::remap_from_normalized;
use crate::worldgen::stages::GenerateStage;

// ---------------------------------------------------------------------------
// NormalSurfaceDataStage: per-column surface material replacement
// ---------------------------------------------------------------------------

/// Surface-material replacement stage.
///
/// Reads the heightmap top block and biome per column and replaces surface
/// blocks from the biome's surface material (top/mid/seaFloor/seaFloorDepth).
///
/// - Non-water top: replaces the top block and fills mid blocks below (noise-driven depth).
/// - Water top: scans down past the water and places the sea-floor or mid
///   blocks depending on water depth.
///
/// Built with an injected [`WorldgenBlockTable`].
pub struct NormalSurfaceDataStage {
    table: WorldgenBlockTable,
}

impl NormalSurfaceDataStage {
    pub fn new(table: WorldgenBlockTable) -> Self {
        Self { table }
    }
}

impl GenerateStage for NormalSurfaceDataStage {
    /// Replaces surface blocks for every column in the chunk.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        // --- Snapshot immutable inputs ---
        let holder = ctx.holder;

        // Surface noise from the holder.
        let noise = holder.surface_holder().noise();
        let water = self.table.water;
        let air = self.table.air;

        for x in 0u8..16 {
            for z in 0u8..16 {
                // Top block, biome, and surface material for this column.
                let y = ctx.chunk.height_map(x, z);
                let top_block_state = ctx.chunk.block_state(x, y, z, 0);
                let biome_id = ctx.chunk.biome_id(x, y, z);

                let material = biome_surface_material(biome_id, &self.table);

                // Branch on whether the top block is water.
                if top_block_state != water {
                    // --- Dry branch ---
                    ctx.chunk.set_block_state(x, y, z, 0, material.top);
                    // Fill depth from surface noise (local x/z).
                    let mid_depth =
                        remap_from_normalized(noise.get_value(x as f64, 0.0, z as f64), 1.0, 4.0);
                    // Integer counter compared against the float depth.
                    let mut i = 1i32;
                    while (i as f32) < mid_depth {
                        ctx.chunk
                            .set_block_state(x, y.wrapping_sub(i), z, 0, material.mid);
                        i = i.wrapping_add(1);
                    }
                } else {
                    // --- Water branch ---
                    // Scan down past the water column; wrapping arithmetic
                    // preserves 32-bit overflow wraparound.
                    let mut depth = 0i32;
                    let mut current = top_block_state;
                    while current == water {
                        depth = depth.wrapping_add(1);
                        current = ctx.chunk.block_state(x, y.wrapping_sub(depth), z, 0);
                    }
                    if depth > material.sea_floor_depth {
                        ctx.chunk.set_block_state(
                            x,
                            y.wrapping_sub(depth),
                            z,
                            0,
                            material.sea_floor,
                        );
                    } else {
                        for i in 0..3i32 {
                            ctx.chunk.set_block_state(
                                x,
                                y.wrapping_sub(depth).wrapping_sub(i),
                                z,
                                0,
                                material.mid,
                            );
                        }
                    }
                }

                // Keep the air binding used (reserved for water/air checks).
                let _ = air;
            }
        }
    }

    /// Stage name used for chain lookup.
    fn name(&self) -> &'static str {
        "normal_surface"
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
    use crate::worldgen::stages::terrain::NormalTerrainStage;
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

    /// Builds a chunk with terrain already generated, for surface-stage tests.
    fn make_terrain_chunk(seed: i64, cx: i32, cz: i32) -> (WorldgenChunk, WorldgenBlockTable) {
        let blocks = test_blocks();
        let holder = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(seed),
            blocks.clone(),
        );
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(cx, cz));
        let mut wc = WorldgenChunk::new(chunk);
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, seed, -64, 319);

        // Terrain stage generates base blocks + heightmap.
        let terrain = NormalTerrainStage::new(
            blocks.stone,
            BlockRuntimeId(10), // deepslate
            BlockRuntimeId(20), // bedrock
            blocks.air,
        );
        terrain.apply(&mut ctx);

        // Build the block table (using the test-block ids).
        let table = WorldgenBlockTable {
            air: blocks.air,
            stone: blocks.stone,
            deepslate: BlockRuntimeId(10),
            bedrock: BlockRuntimeId(20),
            water: blocks.water,
            flowing_water: blocks.water, // Simplified: flowing matches still.
            dirt: BlockRuntimeId(30),
            coarse_dirt: BlockRuntimeId(31),
            grass_block: BlockRuntimeId(32),
            sand: BlockRuntimeId(33),
            sandstone: BlockRuntimeId(34),
            gravel: BlockRuntimeId(35),
            hardened_clay: BlockRuntimeId(36),
            orange_terracotta: BlockRuntimeId(37),
            white_terracotta: BlockRuntimeId(38),
            yellow_terracotta: BlockRuntimeId(39),
            brown_terracotta: BlockRuntimeId(40),
            red_terracotta: BlockRuntimeId(41),
            light_gray_terracotta: BlockRuntimeId(42),
            snow_layer: BlockRuntimeId(43),
            snow_block: BlockRuntimeId(44),
            ice: BlockRuntimeId(45),
            packed_ice: BlockRuntimeId(46),
            mycelium: BlockRuntimeId(47),
            podzol: BlockRuntimeId(48),
            red_sand: BlockRuntimeId(49),
            red_sandstone: BlockRuntimeId(50),
            terracotta: BlockRuntimeId(51),
        };

        (wc, table)
    }

    #[test]
    fn stage_name_is_normal_surface() {
        let table = WorldgenBlockTable::from_core_palette();
        let stage = NormalSurfaceDataStage::new(table);
        assert_eq!(stage.name(), "normal_surface");
    }

    #[test]
    fn surface_data_stage_runs_without_panic() {
        let (mut wc, table) = make_terrain_chunk(42, 0, 0);
        let holder = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(42),
            test_blocks(),
        );
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);
        let stage = NormalSurfaceDataStage::new(table);
        stage.apply(&mut ctx);
        // Passing means no panic.
    }

    #[test]
    fn surface_data_stage_is_deterministic() {
        let seed = 999;
        // First run.
        let (mut wc1, table1) = make_terrain_chunk(seed, 3, -2);
        let holder1 = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(seed),
            test_blocks(),
        );
        let mut ctx1 = ChunkGenerateContext::new(&mut wc1, &holder1, seed, -64, 319);
        NormalSurfaceDataStage::new(table1).apply(&mut ctx1);

        // Second run.
        let (mut wc2, table2) = make_terrain_chunk(seed, 3, -2);
        let holder2 = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(seed),
            test_blocks(),
        );
        let mut ctx2 = ChunkGenerateContext::new(&mut wc2, &holder2, seed, -64, 319);
        NormalSurfaceDataStage::new(table2).apply(&mut ctx2);

        // Compare sample points.
        for (x, y, z) in [(0u8, 63i32, 0u8), (8, 70, 8), (3, 80, 7), (15, 100, 15)] {
            assert_eq!(
                wc1.block_state(x, y, z, 0),
                wc2.block_state(x, y, z, 0),
                "block mismatch at ({x},{y},{z})"
            );
        }
    }

    #[test]
    fn surface_data_replaces_top_block_at_heightmap() {
        // Simple chunk: stone at y=70, heightmap=70, biome=PLAINS (top becomes grass).
        let (mut wc, table) = make_terrain_chunk(42, 0, 0);

        // Manually set one column: stone at y=70, heightmap=70.
        let stone = test_blocks().stone;
        wc.set_block_state(5, 70, 5, 0, stone);
        wc.set_height_map(5, 5, 70);
        // biome = PLAINS(1), so the surface material is grass/dirt/dirt.
        wc.set_biome_id(5, 70, 5, crate::worldgen::biome::biome_id::PLAINS);

        let holder = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(42),
            test_blocks(),
        );
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);
        let stage = NormalSurfaceDataStage::new(table);
        stage.apply(&mut ctx);

        // y=70 should become grass_block.
        assert_eq!(wc.block_state(5, 70, 5, 0), BlockRuntimeId(32)); // grass_block
    }

    #[test]
    fn surface_data_water_branch_places_sea_floor() {
        // One water column: water at y=63..58, stone at y=57, heightmap=63.
        let (mut wc, table) = make_terrain_chunk(42, 0, 0);
        let water = test_blocks().water;
        let stone = test_blocks().stone;

        // Clear the y=57..64 range.
        for y in 57..=64 {
            wc.set_block_state(3, y, 3, 0, water);
        }
        wc.set_block_state(3, 57, 3, 0, stone);
        wc.set_height_map(3, 3, 63);
        // biome = OCEAN(0) → sea_floor=gravel, sea_floor_depth=0
        wc.set_biome_id(3, 63, 3, crate::worldgen::biome::biome_id::OCEAN);

        let holder = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(42),
            test_blocks(),
        );
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);
        NormalSurfaceDataStage::new(table).apply(&mut ctx);

        // Water depth 6 exceeds sea_floor_depth(0), so sea_floor (gravel) is placed.
        assert_eq!(wc.block_state(3, 57, 3, 0), BlockRuntimeId(35)); // gravel
    }
}
