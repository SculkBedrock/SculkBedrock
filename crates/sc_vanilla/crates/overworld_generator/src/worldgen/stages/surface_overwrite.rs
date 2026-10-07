//! Surface-overwrite stage: biome-specific topsoil cover for special biomes.
//!
//! Notes:
//! - Solidity is approximated as non-air and non-water (at generation time the
//!   columns only hold terrain-class blocks; a solid-property lookup can refine this later).
//! - Water covers both still and flowing variants via the block table.
//! - The clay-band cache uses a lazily initialized cell (single-threaded
//!   generation needs no locking primitive; `RefCell` provides borrow checks).
//! - The `"clay_bands"` seed uses the 32-bit string hash helper.
//! - Rounding uses the half-up helper (differs from `f32::round`).

use crate::blocks_table::WorldgenBlockTable;
use crate::worldgen::biome::biome_id::*;
use crate::worldgen::context::ChunkGenerateContext;
use crate::worldgen::holder::normal::SurfaceOverwriteHolder;
use crate::worldgen::math::{java_math_round_f32, java_string_hashcode};
use crate::worldgen::random::{MtRandom, RandomSourceProvider};
use crate::worldgen::stages::terrain::SEA_LEVEL;
use crate::worldgen::stages::{chunk_hash, GenerateStage};
use sc_world::chunk::BlockRuntimeId;

// ---------------------------------------------------------------------------
// NormalSurfaceOverwriteStage: biome-specific topsoil cover
// ---------------------------------------------------------------------------

/// Surface-overwrite stage for special biomes.
///
/// Handles badlands clay bands, swamp water, ice-plains snow layers, frozen-ocean
/// icebergs, and eroded-badlands pillars.
///
/// Built with an injected [`WorldgenBlockTable`].
pub struct NormalSurfaceOverwriteStage {
    table: WorldgenBlockTable,
}

impl NormalSurfaceOverwriteStage {
    pub fn new(table: WorldgenBlockTable) -> Self {
        Self { table }
    }

    /// Solidity check: non-air and non-water counts as solid at generation time.
    #[inline]
    fn is_solid(&self, state: BlockRuntimeId) -> bool {
        state != self.table.air && !self.table.is_water(state)
    }

    /// Water-state check via the block table.
    #[inline]
    fn is_water_state(&self, state: BlockRuntimeId) -> bool {
        self.table.is_water(state)
    }

    /// Noise-range check for the mesa-plateau stone cover.
    fn is_in_range(noise: f32) -> bool {
        (noise >= -0.909 && noise <= -0.5454)
            || (noise >= -0.1818 && noise <= 0.1818)
            || (noise >= 0.5454 && noise <= 0.909)
    }
}

impl GenerateStage for NormalSurfaceOverwriteStage {
    /// Applies biome-specific surface cover to every column in the chunk.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        // --- Snapshot immutable inputs ---
        let holder = ctx.holder;
        let level_seed: i64 = ctx.level_seed;
        let chunk_x: i32 = ctx.chunk.x();
        let chunk_z: i32 = ctx.chunk.z();
        let min_y: i32 = ctx.min_y;
        let surface_overwrite_holder = holder.surface_overwrite_holder();

        for x in 0u8..16 {
            for z in 0u8..16 {
                // World coordinates of this column.
                let lx = x as i32 + (chunk_x << 4);
                let lz = z as i32 + (chunk_z << 4);
                let y = ctx.chunk.height_map(x, z);
                let biome_id = ctx.chunk.biome_id(x, y, z);

                // --- First dispatch: badlands clay-band depth ---
                match biome_id {
                    MESA | MESA_BRYCE | MESA_PLATEAU_STONE => {
                        self.apply_clay_bands_depth(
                            ctx.chunk,
                            surface_overwrite_holder,
                            level_seed,
                            x,
                            z,
                            lx,
                            lz,
                            y,
                        );
                    }
                    _ => {}
                }

                // --- Second dispatch: per-biome cover (no fall-through) ---
                match biome_id {
                    // Swampland water patch.
                    SWAMPLAND => {
                        if surface_overwrite_holder
                            .swamp_noise()
                            .get_value(lx as f64, y as f64, lz as f64)
                            > 0.0
                        {
                            if y == SEA_LEVEL - 1 {
                                ctx.chunk.set_block_state(x, y, z, 0, self.table.water);
                            }
                        }
                    }
                    // Mangrove-swamp water patch.
                    MANGROVE_SWAMP => {
                        if surface_overwrite_holder
                            .swamp_noise()
                            .get_value(lx as f64, y as f64, lz as f64)
                            > 0.0
                        {
                            if y >= SEA_LEVEL - 2 && y < SEA_LEVEL {
                                ctx.chunk.set_block_state(x, y, z, 0, self.table.water);
                            }
                        }
                    }
                    // Mesa-plateau stone top cover.
                    MESA_PLATEAU_STONE => {
                        if y > 97 {
                            let noise = surface_overwrite_holder.surface_noise().get_value(
                                lx as f64 * 0.25,
                                y as f64,
                                lz as f64 * 0.25,
                            );
                            if Self::is_in_range(noise) {
                                ctx.chunk
                                    .set_block_state(x, y, z, 0, self.table.coarse_dirt);
                            } else {
                                ctx.chunk
                                    .set_block_state(x, y, z, 0, self.table.grass_block);
                            }
                        }
                    }
                    // Ice-plains snow layer.
                    ICE_PLAINS => {
                        let support = ctx.chunk.block_state(x, y, z, 0);
                        if self.is_solid(support) {
                            let state_above = ctx.chunk.block_state(x, y + 1, z, 0);
                            if state_above == self.table.air {
                                ctx.chunk
                                    .set_block_state(x, y + 1, z, 0, self.table.snow_layer);
                            } else {
                                // Layer 1 is the waterlog layer (stacks the snow layer
                                // onto an existing block).
                                let _ = ctx.chunk.get_and_set_block_state(
                                    x,
                                    y + 1,
                                    z,
                                    1,
                                    self.table.snow_layer,
                                );
                            }
                        }
                    }
                    // Frozen-ocean iceberg extension.
                    FROZEN_OCEAN | DEEP_FROZEN_OCEAN | LEGACY_FROZEN_OCEAN => {
                        self.frozen_ocean_extension(
                            ctx.chunk,
                            surface_overwrite_holder,
                            level_seed,
                            chunk_x,
                            chunk_z,
                            x,
                            z,
                            lx,
                            lz,
                            y,
                        );
                    }
                    // Eroded-badlands (bryce) pillar extension.
                    MESA_BRYCE => {
                        self.eroded_badlands_extension(
                            ctx.chunk,
                            surface_overwrite_holder,
                            level_seed,
                            x,
                            z,
                            lx,
                            lz,
                            y,
                            min_y,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    /// Stage name used for chain lookup.
    fn name(&self) -> &'static str {
        "normal_surface_overwrite"
    }
}

// ---------------------------------------------------------------------------
// Frozen-ocean iceberg extension
// ---------------------------------------------------------------------------

impl NormalSurfaceOverwriteStage {
    /// Builds a packed-ice/snow iceberg column where the iceberg noise passes the threshold.
    #[allow(clippy::too_many_arguments)]
    fn frozen_ocean_extension(
        &self,
        chunk: &mut crate::worldgen::chunk::WorldgenChunk,
        holder: &SurfaceOverwriteHolder,
        level_seed: i64,
        chunk_x: i32,
        chunk_z: i32,
        local_x: u8,
        local_z: u8,
        world_x: i32,
        world_z: i32,
        height: i32,
    ) {
        // Iceberg size: min of the scaled surface noise and pillar noise.
        let iceberg = (holder
            .iceberg_surface_noise()
            .get_value(world_x as f64, 0.0, world_z as f64) as f64
            * 8.25)
            .abs()
            .min(
                holder.iceberg_pillar_noise().get_value(
                    world_x as f64 * 1.28,
                    0.0,
                    world_z as f64 * 1.28,
                ) as f64
                    * 15.0,
            );
        // Skip columns below the iceberg threshold.
        if iceberg <= 1.8 {
            return;
        }

        // Iceberg roof height from the pillar-roof noise.
        let iceberg_roof = (holder.iceberg_pillar_roof_noise().get_value(
            world_x as f64 * 1.17,
            0.0,
            world_z as f64 * 1.17,
        ) as f64
            * 1.5)
            .abs();
        // Capped iceberg top height.
        let top = (iceberg * iceberg * 1.2).min((iceberg_roof * 40.0).ceil() + 14.0);
        // Skip columns below the top-height threshold.
        if top <= 2.0 {
            return;
        }

        // Vertical iceberg bounds.
        let extension_bottom = SEA_LEVEL as f64 - top - 7.0;
        let extension_top = top + SEA_LEVEL as f64;

        // Per-column RNG seeded from the level seed, chunk, and world position.
        let mut random = MtRandom::new(
            level_seed ^ chunk_hash(chunk_x, chunk_z) ^ (world_x as i64) ^ (world_z as i64),
        );

        // Snow-cap budget and height.
        let max_snow_depth = 2 + random.next_bounded_int(4);
        let min_snow_height = SEA_LEVEL + 18 + random.next_bounded_int(10);
        let mut snow_depth = 0i32;

        // Scan down from the higher of the terrain top and the iceberg top.
        let start_y = height.max((extension_top as i32) + 1);
        for y in (1..=start_y).rev() {
            let state = chunk.block_state(local_x, y, local_z, 0);
            // Stop at stone.
            if state == self.table.stone {
                break;
            }
            // Placement chance for air vs. water cells.
            let place = (state == self.table.air
                && y < (extension_top as i32)
                && random.next_double() > 0.01)
                || (self.is_water_state(state)
                    && y > (extension_bottom as i32)
                    && y <= SEA_LEVEL
                    && random.next_double() > 0.15);
            if !place {
                continue;
            }

            // Snow cap first, then packed ice.
            if snow_depth <= max_snow_depth && y > min_snow_height {
                chunk.set_block_state(local_x, y, local_z, 0, self.table.snow_block);
                snow_depth += 1;
            } else {
                chunk.set_block_state(local_x, y, local_z, 0, self.table.packed_ice);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Eroded-badlands pillar extension
// ---------------------------------------------------------------------------

impl NormalSurfaceOverwriteStage {
    /// Builds an eroded-badlands clay pillar where the pillar noise passes the threshold.
    #[allow(clippy::too_many_arguments)]
    fn eroded_badlands_extension(
        &self,
        chunk: &mut crate::worldgen::chunk::WorldgenChunk,
        holder: &SurfaceOverwriteHolder,
        level_seed: i64,
        local_x: u8,
        local_z: u8,
        world_x: i32,
        world_z: i32,
        height: i32,
        min_y: i32,
    ) {
        // Pillar size: min of the scaled surface noise and pillar noise.
        let pillar_buffer =
            (holder
                .badlands_surface_noise()
                .get_value(world_x as f64, 0.0, world_z as f64) as f64
                * 8.25)
                .abs()
                .min(
                    holder.badlands_pillar_noise().get_value(
                        world_x as f64 * 0.2,
                        0.0,
                        world_z as f64 * 0.2,
                    ) as f64
                        * 15.0,
                );
        // Skip columns below the pillar threshold.
        if pillar_buffer <= 0.0 {
            return;
        }

        // Pillar floor height from the pillar-roof noise.
        let pillar_floor = (holder.badlands_pillar_roof_noise().get_value(
            world_x as f64 * 0.75,
            0.0,
            world_z as f64 * 0.75,
        ) as f64
            * 1.5)
            .abs();
        // Capped pillar top height.
        let extension_top =
            64.0 + (pillar_buffer * pillar_buffer * 2.5).min((pillar_floor * 50.0).ceil() + 24.0);
        // Floored pillar start height.
        let start_y = extension_top.floor() as i32;
        // Skip columns already above the pillar top.
        if height > start_y {
            return;
        }

        // Scan down for the first solid block (water aborts the column).
        for y in (min_y..=start_y).rev() {
            let state = chunk.block_state(local_x, y, local_z, 0);
            if self.is_water_state(state) {
                return;
            }
            if self.is_solid(state) {
                break;
            }
        }

        // Fill clay bands down from the pillar top (stop at non-air).
        for y in (min_y..=start_y).rev() {
            let state = chunk.block_state(local_x, y, local_z, 0);
            if state != self.table.air {
                break;
            }
            let band = self.get_clay_band(holder, level_seed, world_x, y, world_z);
            chunk.set_block_state(local_x, y, local_z, 0, band);
        }
    }
}

// ---------------------------------------------------------------------------
// Clay-band depth fill
// ---------------------------------------------------------------------------

impl NormalSurfaceOverwriteStage {
    /// Fills badlands clay bands at and below the surface for one column.
    #[allow(clippy::too_many_arguments)]
    fn apply_clay_bands_depth(
        &self,
        chunk: &mut crate::worldgen::chunk::WorldgenChunk,
        holder: &SurfaceOverwriteHolder,
        level_seed: i64,
        local_x: u8,
        local_z: u8,
        world_x: i32,
        world_z: i32,
        surface_y: i32,
    ) {
        // Top block of the column.
        if surface_y >= 256 {
            chunk.set_block_state(local_x, surface_y, local_z, 0, self.table.orange_terracotta);
        } else if surface_y >= 74 {
            let band = self.get_clay_band(holder, level_seed, world_x, surface_y, world_z);
            chunk.set_block_state(local_x, surface_y, local_z, 0, band);
        }

        // Fill the y=74..64 transition range.
        for y in 64..=74 {
            if chunk.block_state(local_x, y, local_z, 0) == self.table.air {
                continue;
            }
            if surface_y > 63 && surface_y < 74 {
                chunk.set_block_state(local_x, y, local_z, 0, self.table.orange_terracotta);
            } else {
                let band = self.get_clay_band(holder, level_seed, world_x, y, world_z);
                chunk.set_block_state(local_x, y, local_z, 0, band);
            }
        }

        // Continue the clay bands below the surface.
        // depth = 4 + floor(|surfaceNoise * 3.0|)
        let depth = 4
            + (holder
                .surface_noise()
                .get_value(world_x as f64 * 0.25, 0.0, world_z as f64 * 0.25)
                .abs()
                * 3.0) as i32;

        // Fill downward while the cells stay solid.
        let stop_y = surface_y - depth;
        for y in (stop_y..surface_y).rev() {
            if y < 0 {
                break;
            }
            let state = chunk.block_state(local_x, y, local_z, 0);
            if state == self.table.air || self.is_water_state(state) {
                break;
            }
            if !self.is_solid(state) {
                break;
            }
            let band = self.get_clay_band(holder, level_seed, world_x, y, world_z);
            chunk.set_block_state(local_x, y, local_z, 0, band);
        }
    }
}

// ---------------------------------------------------------------------------
// Clay-band lookup + band-pattern generation
// ---------------------------------------------------------------------------

impl NormalSurfaceOverwriteStage {
    /// Returns the clay-band block for one column position.
    ///
    /// `level_seed` seeds the band-pattern RNG. The band cache is initialized
    /// lazily on first use, then indexed by `y + offset`.
    fn get_clay_band(
        &self,
        holder: &SurfaceOverwriteHolder,
        level_seed: i64,
        world_x: i32,
        y: i32,
        world_z: i32,
    ) -> BlockRuntimeId {
        let cache_ref = holder.clay_bands_cache();
        // Lazily generate the bands on first use (single-threaded: `RefCell`
        // borrow/borrow_mut is enough).
        {
            let cache = cache_ref.borrow();
            if cache[0].is_none() {
                drop(cache);
                let mut cache = cache_ref.borrow_mut();
                if cache[0].is_none() {
                    Self::generate_bands(
                        &mut cache,
                        level_seed,
                        self.table.hardened_clay,
                        self.table.orange_terracotta,
                        self.table.yellow_terracotta,
                        self.table.brown_terracotta,
                        self.table.red_terracotta,
                        self.table.white_terracotta,
                        self.table.light_gray_terracotta,
                    );
                }
            }
        }

        // Noise-driven band offset (rounded half-up).
        let offset = java_math_round_f32(
            holder
                .clay_bands_offset_noise()
                .get_value(world_x as f64, 0.0, world_z as f64)
                * 4.0,
        );

        // Wrap the (y + offset) index into the band table.
        let cache = cache_ref.borrow();
        let len = cache.len() as i32;
        let idx = ((y + offset + len) % len) as usize;
        cache[idx].unwrap_or(self.table.hardened_clay)
    }

    /// Generates the full clay-band pattern for one seed.
    #[allow(clippy::too_many_arguments)]
    fn generate_bands(
        bands: &mut [Option<BlockRuntimeId>],
        level_seed: i64,
        hardened_clay: BlockRuntimeId,
        orange_terracotta: BlockRuntimeId,
        yellow_terracotta: BlockRuntimeId,
        brown_terracotta: BlockRuntimeId,
        red_terracotta: BlockRuntimeId,
        white_terracotta: BlockRuntimeId,
        light_gray_terracotta: BlockRuntimeId,
    ) {
        // Band RNG seeded from the level seed mixed with the band-table hash.
        let mut random =
            MtRandom::new(level_seed ^ (java_string_hashcode("clay_bands") as i64));
        // Start with a uniform hardened-clay fill.
        for b in bands.iter_mut() {
            *b = Some(hardened_clay);
        }

        // Scattered orange-terracotta slots.
        let mut i = 0i32;
        while i < bands.len() as i32 {
            i += random.next_bounded_int(5) + 1;
            if i < bands.len() as i32 {
                bands[i as usize] = Some(orange_terracotta);
            }
        }

        // Three colored band runs.
        Self::make_bands(&mut random, bands, 1, yellow_terracotta);
        Self::make_bands(&mut random, bands, 2, brown_terracotta);
        Self::make_bands(&mut random, bands, 1, red_terracotta);

        // White/light-gray band runs.
        let white_band_count = random.next_int_range(9, 15);
        let mut count = 0i32;
        let mut start = 0i32;
        while count < white_band_count && start < bands.len() as i32 {
            bands[start as usize] = Some(white_terracotta);
            if start - 1 > 0 && random.next_boolean() {
                bands[(start - 1) as usize] = Some(light_gray_terracotta);
            }
            if start + 1 < bands.len() as i32 && random.next_boolean() {
                bands[(start + 1) as usize] = Some(light_gray_terracotta);
            }
            count += 1;
            start += random.next_bounded_int(16) + 4;
        }
    }

    /// Paints one colored band run into the band table.
    fn make_bands(
        random: &mut MtRandom,
        bands: &mut [Option<BlockRuntimeId>],
        base_width: i32,
        state: BlockRuntimeId,
    ) {
        // Random band-run count.
        let band_count = random.next_int_range(6, 15);
        for _ in 0..band_count {
            // Random run width.
            let width = base_width + random.next_bounded_int(3);
            // Random run start.
            let start = random.next_bounded_int(bands.len() as i32 - 1);
            // Paint the run.
            let mut p = 0;
            while start + p < bands.len() as i32 && p < width {
                bands[(start + p) as usize] = Some(state);
                p += 1;
            }
        }
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

    fn test_table() -> WorldgenBlockTable {
        WorldgenBlockTable {
            air: BlockRuntimeId(0),
            stone: BlockRuntimeId(3),
            deepslate: BlockRuntimeId(10),
            bedrock: BlockRuntimeId(20),
            water: BlockRuntimeId(1),
            flowing_water: BlockRuntimeId(1),
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
        }
    }

    #[test]
    fn stage_name_is_normal_surface_overwrite() {
        let stage = NormalSurfaceOverwriteStage::new(test_table());
        assert_eq!(stage.name(), "normal_surface_overwrite");
    }

    #[test]
    fn is_in_range_thresholds() {
        // Three valid ranges.
        assert!(NormalSurfaceOverwriteStage::is_in_range(-0.909));
        assert!(NormalSurfaceOverwriteStage::is_in_range(-0.7));
        assert!(NormalSurfaceOverwriteStage::is_in_range(-0.5454));
        assert!(NormalSurfaceOverwriteStage::is_in_range(0.0));
        assert!(NormalSurfaceOverwriteStage::is_in_range(0.1818));
        assert!(NormalSurfaceOverwriteStage::is_in_range(0.7));
        assert!(NormalSurfaceOverwriteStage::is_in_range(0.909));
        // Outside the ranges.
        assert!(!NormalSurfaceOverwriteStage::is_in_range(-0.3));
        assert!(!NormalSurfaceOverwriteStage::is_in_range(0.3));
        assert!(!NormalSurfaceOverwriteStage::is_in_range(-1.0));
        assert!(!NormalSurfaceOverwriteStage::is_in_range(1.0));
    }

    #[test]
    fn is_solid_approximation() {
        let stage = NormalSurfaceOverwriteStage::new(test_table());
        // Air and water are not solid.
        assert!(!stage.is_solid(BlockRuntimeId(0))); // air
        assert!(!stage.is_solid(BlockRuntimeId(1))); // water
        // Other blocks are solid.
        assert!(stage.is_solid(BlockRuntimeId(3))); // stone
        assert!(stage.is_solid(BlockRuntimeId(32))); // grass
        assert!(stage.is_solid(BlockRuntimeId(44))); // snow_block
    }

    #[test]
    fn generate_bands_is_deterministic() {
        let blocks = test_blocks();
        let mut bands1 = vec![None; 192];
        let mut bands2 = vec![None; 192];

        NormalSurfaceOverwriteStage::generate_bands(
            &mut bands1,
            12345,
            blocks.stone, // Stone stands in for hardened clay (simplified test).
            BlockRuntimeId(37),
            BlockRuntimeId(39),
            BlockRuntimeId(40),
            BlockRuntimeId(41),
            BlockRuntimeId(38),
            BlockRuntimeId(42),
        );
        NormalSurfaceOverwriteStage::generate_bands(
            &mut bands2,
            12345,
            blocks.stone,
            BlockRuntimeId(37),
            BlockRuntimeId(39),
            BlockRuntimeId(40),
            BlockRuntimeId(41),
            BlockRuntimeId(38),
            BlockRuntimeId(42),
        );

        assert_eq!(bands1, bands2, "generateBands should be deterministic");
        // Every slot should be filled (no None).
        assert!(bands1.iter().all(|b| b.is_some()));
    }

    #[test]
    fn generate_bands_different_seeds_differ() {
        let mut bands1 = vec![None; 192];
        let mut bands2 = vec![None; 192];

        NormalSurfaceOverwriteStage::generate_bands(
            &mut bands1,
            111,
            BlockRuntimeId(36),
            BlockRuntimeId(37),
            BlockRuntimeId(39),
            BlockRuntimeId(40),
            BlockRuntimeId(41),
            BlockRuntimeId(38),
            BlockRuntimeId(42),
        );
        NormalSurfaceOverwriteStage::generate_bands(
            &mut bands2,
            222,
            BlockRuntimeId(36),
            BlockRuntimeId(37),
            BlockRuntimeId(39),
            BlockRuntimeId(40),
            BlockRuntimeId(41),
            BlockRuntimeId(38),
            BlockRuntimeId(42),
        );

        // Bands from different seeds should differ (overwhelmingly likely).
        assert_ne!(bands1, bands2);
    }

    #[test]
    fn clay_band_cache_lazy_init() {
        let table = test_table();
        let stage = NormalSurfaceOverwriteStage::new(table);

        let holder = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(42),
            test_blocks(),
        );
        let soh = holder.surface_overwrite_holder();

        // The first call triggers band generation.
        let band1 = stage.get_clay_band(soh, 42, 0, 50, 0);
        // The cache should be initialized now.
        {
            let cache = soh.clay_bands_cache().borrow();
            assert!(
                cache[0].is_some(),
                "cache should be initialized after first call"
            );
        }
        // The second call reuses the cache (no regeneration).
        let band2 = stage.get_clay_band(soh, 42, 0, 50, 0);
        assert_eq!(band1, band2, "same coords should return same band");
    }

    #[test]
    fn surface_overwrite_runs_without_panic() {
        let table = test_table();
        let stage = NormalSurfaceOverwriteStage::new(table);

        let holder = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(42),
            test_blocks(),
        );
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        // Set heightmap/biome coverage to avoid the all-zero branch.
        for x in 0u8..16 {
            for z in 0u8..16 {
                wc.set_height_map(x, z, 70);
                wc.set_biome_id(x, 70, z, PLAINS);
            }
        }
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);
        stage.apply(&mut ctx);
        // Passing means no panic.
    }

    #[test]
    fn surface_overwrite_deterministic() {
        let seed = 777;
        let table = test_table();

        // First run.
        let holder1 = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(seed),
            test_blocks(),
        );
        let chunk1 = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(2, -3));
        let mut wc1 = WorldgenChunk::new(chunk1);
        for x in 0u8..16 {
            for z in 0u8..16 {
                wc1.set_height_map(x, z, 80);
                wc1.set_biome_id(x, 80, z, MESA);
            }
        }
        let mut ctx1 = ChunkGenerateContext::new(&mut wc1, &holder1, seed, -64, 319);
        NormalSurfaceOverwriteStage::new(table.clone()).apply(&mut ctx1);

        // Second run.
        let holder2 = crate::worldgen::holder::normal::NormalObjectHolder::new(
            Xoroshiro128::new(seed),
            test_blocks(),
        );
        let chunk2 = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(2, -3));
        let mut wc2 = WorldgenChunk::new(chunk2);
        for x in 0u8..16 {
            for z in 0u8..16 {
                wc2.set_height_map(x, z, 80);
                wc2.set_biome_id(x, 80, z, MESA);
            }
        }
        let mut ctx2 = ChunkGenerateContext::new(&mut wc2, &holder2, seed, -64, 319);
        NormalSurfaceOverwriteStage::new(table).apply(&mut ctx2);

        // Compare sample points.
        for (x, y, z) in [(0u8, 75i32, 0u8), (8, 70, 8), (3, 64, 7)] {
            assert_eq!(
                wc1.block_state(x, y, z, 0),
                wc2.block_state(x, y, z, 0),
                "block mismatch at ({x},{y},{z})"
            );
        }
    }
}
