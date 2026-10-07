//! Normal terrain stage: cell sampling, flood fill, aquifer fill, bedrock, heightmap.
//!
//! Notes:
//! - Per-apply local RNG (single-threaded generation, reseeded per chunk).
//! - The chunk is edited directly through `&mut WorldgenChunk`.
//! - Fluid-update scheduling belongs to the runtime layer and is skipped here.
//! - The per-chunk evaluation cache is a local value.
//! - The work queue uses `std::collections::VecDeque<usize>`.
//! - Stone identity is compared via injected `BlockRuntimeId`s.

use crate::worldgen::context::ChunkGenerateContext;
use crate::worldgen::densityfunction::common::CellFunctionContext;
use crate::worldgen::densityfunction::function::ChunkCache;
use crate::worldgen::holder::normal::NormalObjectHolder;
use crate::worldgen::material::filler::{MaterialFiller, MultiMaterial};
use crate::worldgen::random::{MtRandom, RandomSourceProvider};
use crate::worldgen::stages::{chunk_hash, GenerateStage};
use std::collections::VecDeque;
use std::rc::Rc;
use sc_world::chunk::BlockRuntimeId;

// ---------------------------------------------------------------------------
// Terrain constants
// ---------------------------------------------------------------------------

/// Sea level (surface waterline).
pub const SEA_LEVEL: i32 = 63;

/// Horizontal cell size in blocks.
const CELL_XZ_SIZE: i32 = 4;

/// Vertical cell size in blocks.
const CELL_HEIGHT: i32 = 8;

/// Cell count along x within one chunk (16 / cell size).
const CELL_X_COUNT: usize = 4;

/// Cell count along z within one chunk (16 / cell size).
const CELL_Z_COUNT: usize = 4;

/// Max cell y that can seed corner flood fill.
const CORNER_FLOOD_SEED_MAX_Y: i32 = 192;

// ---------------------------------------------------------------------------
// NormalTerrainStage: base terrain fill for the overworld
// ---------------------------------------------------------------------------

/// Base terrain generation stage.
///
/// Samples 4x4x8 cells, tracks the mandatory top surface, flood-fills from solid
/// mandatory cells, fills materials via multi-material/aquifer, then places
/// bedrock, applies deepslate replacement, and updates the heightmap.
///
/// Built with the injected stone/deepslate/bedrock/air `BlockRuntimeId`s.
pub struct NormalTerrainStage {
    stone: BlockRuntimeId,
    deepslate: BlockRuntimeId,
    bedrock: BlockRuntimeId,
    air: BlockRuntimeId,
}

impl NormalTerrainStage {
    /// Builds the stage from explicit block ids.
    pub fn new(
        stone: BlockRuntimeId,
        deepslate: BlockRuntimeId,
        bedrock: BlockRuntimeId,
        air: BlockRuntimeId,
    ) -> Self {
        Self {
            stone,
            deepslate,
            bedrock,
            air,
        }
    }
}

impl GenerateStage for NormalTerrainStage {
    /// Generates base terrain, bedrock, and heightmap for the chunk.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        // --- Snapshot immutable inputs (shared refs + Copy scalars, no ctx borrow held) ---
        let holder: &NormalObjectHolder = ctx.holder;
        let level_seed: i64 = ctx.level_seed;
        let min_y: i32 = ctx.min_y;
        let max_y: i32 = ctx.max_y;
        let chunk_x: i32 = ctx.chunk.x();
        let chunk_z: i32 = ctx.chunk.z();

        let terrain_holder = holder.terrain_holder();
        let multi_material = terrain_holder.multi_material();

        // Block-column height: `max_y` is inclusive here.
        let y_block_size = max_y + 1 - min_y;
        let chunk_base_x = chunk_x << 4;
        let chunk_base_z = chunk_z << 4;
        // Floor-divide `min_y` down to the 8-block cell grid.
        let cell_min_y = min_y.div_euclid(8) * 8;
        let cell_max_y = max_y.div_euclid(8) * 8;

        // Fresh per-chunk evaluation cache.
        let chunk_cache = Rc::new(ChunkCache::new());
        chunk_cache.clear();

        // Per-chunk RNG seeded from the level seed and chunk position.
        let mut random = MtRandom::new(level_seed ^ chunk_hash(chunk_x, chunk_z));

        // Function context over the chunk cache.
        let function_context = CellFunctionContext::new(Rc::clone(&chunk_cache));

        // Vertical cell count covering [cell_min_y, cell_max_y].
        let cell_y_count = (((cell_max_y - cell_min_y) / CELL_HEIGHT) + 1) as usize;

        // --- Mandatory top surface per column ---
        let mut mandatory_top_y = [0i32; 256];
        for x in 0..16usize {
            let world_x = chunk_base_x + x as i32;
            for z in 0..16usize {
                let world_z = chunk_base_z + z as i32;
                // Ceiled preliminary surface bound for this column.
                let upper = terrain_holder
                    .preliminary_surface_upper_bound()
                    .compute(function_context.set(world_x, 0, world_z))
                    .ceil() as i32;
                // Clamp into [sea level, max_y].
                mandatory_top_y[(x << 4) | z] = max_y.min(SEA_LEVEL.max(upper));
            }
        }

        // --- Begin aquifer sampling ---
        terrain_holder.begin_aquifer(
            chunk_x,
            chunk_z,
            level_seed,
            Rc::clone(&chunk_cache),
            min_y,
            y_block_size,
            SEA_LEVEL,
        );

        // --- Main generation passes ---
        let total_cells = CELL_X_COUNT * cell_y_count * CELL_Z_COUNT;
        let mut queued = vec![false; total_cells];
        let mut solid_mandatory_cells = vec![false; total_cells];
        let mut queue: VecDeque<usize> = VecDeque::new();

        // First pass: mandatory + corner flood-seed cells.
        for cell_y_index in 0..cell_y_count {
            let cell_y = cell_min_y + cell_y_index as i32 * CELL_HEIGHT;
            for cell_x_index in 0..CELL_X_COUNT {
                for cell_z_index in 0..CELL_Z_COUNT {
                    let cell_idx = cell_index(cell_x_index, cell_y_index, cell_z_index);
                    let cell_x = cell_x_index as i32 * CELL_XZ_SIZE;
                    let cell_z = cell_z_index as i32 * CELL_XZ_SIZE;
                    // Skip cells that are neither mandatory nor corner flood seeds.
                    if !should_generate_mandatory_cell(&mandatory_top_y, cell_x, cell_y, cell_z)
                        && !is_corner_flood_seed_cell(
                            cell_x_index,
                            cell_y_index,
                            cell_y,
                            cell_z_index,
                        )
                    {
                        continue;
                    }
                    queued[cell_idx] = true;
                    solid_mandatory_cells[cell_idx] = generate_cell(
                        ctx.chunk,
                        multi_material,
                        &function_context,
                        &mut random,
                        chunk_base_x,
                        chunk_base_z,
                        min_y,
                        max_y,
                        cell_x,
                        cell_y,
                        cell_z,
                        self.stone,
                        self.deepslate,
                        self.air,
                    );
                }
            }
        }

        // Second pass: enqueue neighbors of solid mandatory cells.
        for cell_idx in 0..total_cells {
            if !solid_mandatory_cells[cell_idx] {
                continue;
            }
            let cell_x_index = cell_idx % CELL_X_COUNT;
            let cell_z_index = (cell_idx / CELL_X_COUNT) % CELL_Z_COUNT;
            let cell_y_index = cell_idx / (CELL_X_COUNT * CELL_Z_COUNT);
            enqueue_neighbors(
                &mut queue,
                &mut queued,
                cell_x_index,
                cell_y_index,
                cell_z_index,
                cell_y_count,
            );
        }

        // BFS flood fill over queued cells.
        while let Some(cell_idx) = queue.pop_front() {
            let cell_x_index = cell_idx % CELL_X_COUNT;
            let cell_z_index = (cell_idx / CELL_X_COUNT) % CELL_Z_COUNT;
            let cell_y_index = cell_idx / (CELL_X_COUNT * CELL_Z_COUNT);
            let cell_x = cell_x_index as i32 * CELL_XZ_SIZE;
            let cell_z = cell_z_index as i32 * CELL_XZ_SIZE;
            let cell_y = cell_min_y + cell_y_index as i32 * CELL_HEIGHT;

            if generate_cell(
                ctx.chunk,
                multi_material,
                &function_context,
                &mut random,
                chunk_base_x,
                chunk_base_z,
                min_y,
                max_y,
                cell_x,
                cell_y,
                cell_z,
                self.stone,
                self.deepslate,
                self.air,
            ) {
                enqueue_neighbors(
                    &mut queue,
                    &mut queued,
                    cell_x_index,
                    cell_y_index,
                    cell_z_index,
                    cell_y_count,
                );
            }
        }

        // --- Bedrock floor ---
        for x in 0u8..16 {
            for z in 0u8..16 {
                // Solid bedrock at the bottom row.
                ctx.chunk.set_block_state(x, min_y, z, 0, self.bedrock);
                // Random extra bedrock depth.
                let bedrock_depth = random.next_bounded_int(6);
                for i in 0..bedrock_depth {
                    let y = min_y + i;
                    // Only replace non-air cells.
                    let state = ctx.chunk.block_state(x, y, z, 0);
                    if state != self.air {
                        ctx.chunk.set_block_state(x, y, z, 0, self.bedrock);
                    }
                }
            }
        }

        // --- End aquifer sampling ---
        terrain_holder.end_aquifer();
    }

    /// Stage name used for chain lookup.
    fn name(&self) -> &'static str {
        "normal_terrain"
    }
}

// ---------------------------------------------------------------------------
// Cell fill: samples one 4x8x4 cell into the chunk
// ---------------------------------------------------------------------------

/// Samples one 4x8x4 cell with the multi-material function, writes blocks to
/// the chunk, and updates the heightmap. Returns whether the cell holds any
/// non-air block.
#[allow(clippy::too_many_arguments)]
fn generate_cell(
    chunk: &mut crate::worldgen::chunk::WorldgenChunk,
    multi_material: &MultiMaterial,
    function_context: &CellFunctionContext,
    random: &mut MtRandom,
    chunk_base_x: i32,
    chunk_base_z: i32,
    min_y: i32,
    max_y: i32,
    cell_x: i32,
    cell_y: i32,
    cell_z: i32,
    stone: BlockRuntimeId,
    deepslate: BlockRuntimeId,
    air: BlockRuntimeId,
) -> bool {
    let mut has_non_air = false;

    for local_x in 0..CELL_XZ_SIZE {
        let x = cell_x + local_x;
        let world_x = chunk_base_x + x;
        for local_z in 0..CELL_XZ_SIZE {
            let z = cell_z + local_z;
            let world_z = chunk_base_z + z;
            // Top-down within the cell.
            for local_y in (0..CELL_HEIGHT).rev() {
                let y = cell_y + local_y;
                // Skip rows outside the world bounds.
                if y < min_y || y > max_y {
                    continue;
                }
                // Sample the multi-material function at this block.
                let generated_state =
                    multi_material.calculate(function_context.set(world_x, y, world_z));

                if let Some(generated) = generated_state {
                    // Stone may become deepslate by depth rule.
                    let mut block = generated;
                    if block == stone && should_place_deepslate(random, y) {
                        block = deepslate;
                    }
                    // Write the sampled block.
                    chunk.set_block_state(x as u8, y, z as u8, 0, block);

                    // Fluid-update scheduling belongs to the runtime layer and is skipped.

                    // Track the highest non-air block per column.
                    if block != air {
                        has_non_air = true;
                        let current_hm = chunk.height_map(x as u8, z as u8);
                        if y > current_hm {
                            chunk.set_height_map(x as u8, z as u8, y);
                        }
                    }
                }
            }
        }
    }

    has_non_air
}

// ---------------------------------------------------------------------------
// Mandatory-cell check
// ---------------------------------------------------------------------------

/// Returns whether a cell must always be generated (below sea level, or the
/// mandatory top surface reaches into it).
fn should_generate_mandatory_cell(
    mandatory_top_y: &[i32; 256],
    cell_x: i32,
    cell_y: i32,
    cell_z: i32,
) -> bool {
    // Cells fully below sea level are always mandatory.
    if cell_y + CELL_HEIGHT - 1 <= SEA_LEVEL {
        return true;
    }
    // Otherwise mandatory when any 4x4 column's top surface reaches the cell.
    for local_x in 0..CELL_XZ_SIZE {
        let x = cell_x + local_x;
        for local_z in 0..CELL_XZ_SIZE {
            let z = cell_z + local_z;
            if cell_y <= mandatory_top_y[((x as usize) << 4) | z as usize] {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Corner flood-seed cells + corner helpers
// ---------------------------------------------------------------------------

/// Returns whether a cell seeds corner flood fill (low y, every third row,
/// and a chunk-corner position).
fn is_corner_flood_seed_cell(
    cell_x_index: usize,
    cell_y_index: usize,
    cell_y: i32,
    cell_z_index: usize,
) -> bool {
    // Flood seeds only exist below the max seed height.
    if cell_y > CORNER_FLOOD_SEED_MAX_Y {
        return false;
    }
    // Only every third cell row seeds.
    if cell_y_index % 3 != 0 {
        return false;
    }
    // Any chunk corner qualifies.
    is_north_west_corner_seed(cell_x_index, cell_z_index)
        || is_north_east_corner_seed(cell_x_index, cell_z_index)
        || is_south_west_corner_seed(cell_x_index, cell_z_index)
        || is_south_east_corner_seed(cell_x_index, cell_z_index)
}

/// North-west corner seed cells.
fn is_north_west_corner_seed(cell_x_index: usize, cell_z_index: usize) -> bool {
    (cell_x_index == 0 && cell_z_index == 0)
        || (cell_x_index == 1 && cell_z_index == 0)
        || (cell_x_index == 0 && cell_z_index == 1)
}

/// North-east corner seed cells.
fn is_north_east_corner_seed(cell_x_index: usize, cell_z_index: usize) -> bool {
    (cell_x_index == CELL_X_COUNT - 1 && cell_z_index == 0)
        || (cell_x_index == CELL_X_COUNT - 2 && cell_z_index == 0)
        || (cell_x_index == CELL_X_COUNT - 1 && cell_z_index == 1)
}

/// South-west corner seed cells.
fn is_south_west_corner_seed(cell_x_index: usize, cell_z_index: usize) -> bool {
    (cell_x_index == 0 && cell_z_index == CELL_Z_COUNT - 1)
        || (cell_x_index == 1 && cell_z_index == CELL_Z_COUNT - 1)
        || (cell_x_index == 0 && cell_z_index == CELL_Z_COUNT - 2)
}

/// South-east corner seed cells.
fn is_south_east_corner_seed(cell_x_index: usize, cell_z_index: usize) -> bool {
    (cell_x_index == CELL_X_COUNT - 1 && cell_z_index == CELL_Z_COUNT - 1)
        || (cell_x_index == CELL_X_COUNT - 2 && cell_z_index == CELL_Z_COUNT - 1)
        || (cell_x_index == CELL_X_COUNT - 1 && cell_z_index == CELL_Z_COUNT - 2)
}

// ---------------------------------------------------------------------------
// Cell queue helpers
// ---------------------------------------------------------------------------

/// Enqueues one cell unless out of bounds or already queued.
fn enqueue_cell(
    queue: &mut VecDeque<usize>,
    queued: &mut [bool],
    cell_x_index: usize,
    cell_y_index: usize,
    cell_z_index: usize,
    cell_y_count: usize,
) {
    // Bounds check (negative indices are unreachable via usize).
    if cell_x_index >= CELL_X_COUNT || cell_y_index >= cell_y_count || cell_z_index >= CELL_Z_COUNT
    {
        return;
    }
    let index = cell_index(cell_x_index, cell_y_index, cell_z_index);
    // Skip already-queued cells.
    if queued[index] {
        return;
    }
    queued[index] = true;
    queue.push_back(index);
}

/// Enqueues the 6-direction neighbors of one cell.
fn enqueue_neighbors(
    queue: &mut VecDeque<usize>,
    queued: &mut [bool],
    cell_x_index: usize,
    cell_y_index: usize,
    cell_z_index: usize,
    cell_y_count: usize,
) {
    // 6-direction neighbors (x/y/z +- 1).
    // x+1 (may overflow usize, hence checked_add).
    if let Some(xi) = cell_x_index.checked_add(1) {
        enqueue_cell(queue, queued, xi, cell_y_index, cell_z_index, cell_y_count);
    }
    // x-1
    if cell_x_index > 0 {
        enqueue_cell(
            queue,
            queued,
            cell_x_index - 1,
            cell_y_index,
            cell_z_index,
            cell_y_count,
        );
    }
    // y+1
    if let Some(yi) = cell_y_index.checked_add(1) {
        enqueue_cell(queue, queued, cell_x_index, yi, cell_z_index, cell_y_count);
    }
    // y-1
    if cell_y_index > 0 {
        enqueue_cell(
            queue,
            queued,
            cell_x_index,
            cell_y_index - 1,
            cell_z_index,
            cell_y_count,
        );
    }
    // z+1
    if let Some(zi) = cell_z_index.checked_add(1) {
        enqueue_cell(queue, queued, cell_x_index, cell_y_index, zi, cell_y_count);
    }
    // z-1
    if cell_z_index > 0 {
        enqueue_cell(
            queue,
            queued,
            cell_x_index,
            cell_y_index,
            cell_z_index - 1,
            cell_y_count,
        );
    }
}

// ---------------------------------------------------------------------------
// Cell index packing
// ---------------------------------------------------------------------------

/// Cell index packing.
///
/// `(cellYIndex * CELL_Z_COUNT + cellZIndex) * CELL_X_COUNT + cellXIndex`
fn cell_index(cell_x_index: usize, cell_y_index: usize, cell_z_index: usize) -> usize {
    (cell_y_index * CELL_Z_COUNT + cell_z_index) * CELL_X_COUNT + cell_x_index
}

// ---------------------------------------------------------------------------
// Deepslate replacement rule
// ---------------------------------------------------------------------------

/// Decides stone-vs-deepslate replacement for one y level.
fn should_place_deepslate(random: &mut MtRandom, y: i32) -> bool {
    // Below y=0 stone always becomes deepslate.
    if y < 0 {
        return true;
    }
    // Above y=8 stone never becomes deepslate.
    if y > 8 {
        return false;
    }
    // Random gate scaled by height.
    random.next_bounded_int(9) >= y
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

    /// Builds test material blocks sharing one id set with the test holder.
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
    fn cell_index_layout() {
        // Index layout: (y * Z + z) * X + x.
        assert_eq!(cell_index(0, 0, 0), 0);
        assert_eq!(cell_index(1, 0, 0), 1);
        assert_eq!(cell_index(0, 0, 1), CELL_X_COUNT);
        assert_eq!(cell_index(0, 1, 0), CELL_X_COUNT * CELL_Z_COUNT);
    }

    #[test]
    fn corner_seed_detection() {
        // NW corner: (0,0), (1,0), (0,1)
        assert!(is_north_west_corner_seed(0, 0));
        assert!(is_north_west_corner_seed(1, 0));
        assert!(is_north_west_corner_seed(0, 1));
        assert!(!is_north_west_corner_seed(2, 2));

        // NE corner: (3,0), (2,0), (3,1)
        assert!(is_north_east_corner_seed(3, 0));
        assert!(is_north_east_corner_seed(2, 0));
        assert!(is_north_east_corner_seed(3, 1));

        // SW corner: (0,3), (1,3), (0,2)
        assert!(is_south_west_corner_seed(0, 3));
        assert!(is_south_west_corner_seed(1, 3));
        assert!(is_south_west_corner_seed(0, 2));

        // SE corner: (3,3), (2,3), (3,2)
        assert!(is_south_east_corner_seed(3, 3));
        assert!(is_south_east_corner_seed(2, 3));
        assert!(is_south_east_corner_seed(3, 2));
    }

    #[test]
    fn corner_flood_seed_respects_y_and_mod3() {
        // Above the max seed height there are no seeds.
        assert!(!is_corner_flood_seed_cell(0, 0, 200, 0));
        // Only every third row seeds.
        assert!(!is_corner_flood_seed_cell(0, 1, 0, 0));
        // Every third row at/below the max height on a corner seeds.
        assert!(is_corner_flood_seed_cell(0, 0, 0, 0));
        assert!(is_corner_flood_seed_cell(0, 3, 24, 0));
    }

    #[test]
    fn mandatory_cell_below_sea_level() {
        let mt = [100i32; 256];
        // Fully below sea level is always mandatory.
        assert!(should_generate_mandatory_cell(&mt, 0, 56, 0));
        // At/below the top surface is mandatory.
        assert!(should_generate_mandatory_cell(&mt, 0, 100, 0));
        // Above the top surface is not mandatory.
        assert!(!should_generate_mandatory_cell(&mt, 0, 101, 0));
    }

    #[test]
    fn should_place_deepslate_boundary() {
        let mut r = MtRandom::new(42);
        // Below zero always replaces.
        assert!(should_place_deepslate(&mut r, -1));
        // Above 8 never replaces.
        assert!(!should_place_deepslate(&mut r, 9));
    }

    #[test]
    fn enqueue_neighbors_floods_6_directions() {
        let mut queue = VecDeque::new();
        let mut queued = vec![false; 4 * 48 * 4]; // 4x48x4 grid
        // Center cell (1,1,1).
        enqueue_neighbors(&mut queue, &mut queued, 1, 1, 1, 48);
        // 6 in-bounds neighbors expected.
        assert_eq!(queue.len(), 6);
        // Corner cell (0,0,0) has 3 valid neighbors.
        queue.clear();
        let mut queued2 = vec![false; 4 * 48 * 4];
        enqueue_neighbors(&mut queue, &mut queued2, 0, 0, 0, 48);
        assert_eq!(queue.len(), 3);
    }

    #[test]
    fn stage_name_is_normal_terrain() {
        let stage = NormalTerrainStage::new(
            BlockRuntimeId(3),
            BlockRuntimeId(10),
            BlockRuntimeId(20),
            BlockRuntimeId(0),
        );
        assert_eq!(stage.name(), "normal_terrain");
    }

    /// Smoke test for full terrain generation: produces blocks plus a bedrock floor.
    #[test]
    fn terrain_stage_generates_blocks() {
        let blocks = test_blocks();
        let holder = NormalObjectHolder::new(Xoroshiro128::new(12345), blocks.clone());

        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 12345, -64, 319);

        let stage = NormalTerrainStage::new(
            blocks.stone,
            BlockRuntimeId(10), // deepslate (dummy)
            BlockRuntimeId(20), // bedrock (dummy)
            blocks.air,
        );
        stage.apply(&mut ctx);

        // The bottom row should be bedrock.
        assert_eq!(wc.block_state(0, -64, 0, 0), BlockRuntimeId(20));
        assert_eq!(wc.block_state(8, -64, 8, 0), BlockRuntimeId(20));

        // The heightmap should advance (some columns above zero).
        let any_hm_nonzero = (0..16).any(|x| (0..16).any(|z| wc.height_map(x as u8, z as u8) > 0));
        assert!(
            any_hm_nonzero,
            "heightmap should have non-zero entries after terrain generation"
        );

        // The terrain body should hold non-air, non-bedrock blocks.
        let mut has_terrain_block = false;
        'outer: for x in 0u8..16 {
            for z in 0u8..16 {
                for y in -60..100 {
                    let b = wc.block_state(x, y, z, 0);
                    if b != blocks.air && b != BlockRuntimeId(20) {
                        has_terrain_block = true;
                        break 'outer;
                    }
                }
            }
        }
        assert!(
            has_terrain_block,
            "terrain should produce non-air, non-bedrock blocks"
        );
    }

    /// Determinism check: the same seed generates the same result twice.
    #[test]
    fn terrain_stage_is_deterministic() {
        let blocks = test_blocks();

        // First generation.
        let holder1 = NormalObjectHolder::new(Xoroshiro128::new(777), blocks.clone());
        let chunk1 = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(3, -2));
        let mut wc1 = WorldgenChunk::new(chunk1);
        let mut ctx1 = ChunkGenerateContext::new(&mut wc1, &holder1, 777, -64, 319);
        let stage = NormalTerrainStage::new(
            blocks.stone,
            BlockRuntimeId(10),
            BlockRuntimeId(20),
            blocks.air,
        );
        stage.apply(&mut ctx1);

        // Second generation.
        let holder2 = NormalObjectHolder::new(Xoroshiro128::new(777), blocks.clone());
        let chunk2 = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(3, -2));
        let mut wc2 = WorldgenChunk::new(chunk2);
        let mut ctx2 = ChunkGenerateContext::new(&mut wc2, &holder2, 777, -64, 319);
        stage.apply(&mut ctx2);

        // Compare the heightmaps.
        assert_eq!(wc1.height_map_array(), wc2.height_map_array());

        // Compare sample points.
        for (x, y, z) in [
            (0u8, -64i32, 0u8),
            (8, 0, 8),
            (3, 63, 7),
            (15, 100, 15),
            (7, -32, 9),
        ] {
            assert_eq!(
                wc1.block_state(x, y, z, 0),
                wc2.block_state(x, y, z, 0),
                "block mismatch at ({x},{y},{z})"
            );
        }
    }
}
