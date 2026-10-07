//! Aquifer sampling: groundwater/lava levels, barriers, and fluid updates.
//!
//! Notes:
//! - Block states are `BlockRuntimeId`s injected via [`MaterialBlocks`].
//! - The constructor takes plain scalars (chunk position/level seed).
//! - Per-use RNG state is a local instance reseeded before each use.
//! - The preliminary-surface cache is a bounded 64-entry LRU keyed by packed
//!   coordinates, missing as `i32::MIN`.

use crate::worldgen::densityfunction::function::{
    ChunkCache, ChunkCacheContext, DensityFunction, FunctionContext,
};
use crate::worldgen::material::MaterialBlocks;
use crate::worldgen::math::clamp_f64;
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use sc_world::chunk::BlockRuntimeId;

/// Similarity threshold that marks neighboring aquifer cells as flowing (schedules a fluid update).
pub const FLOWING_UPDATE_SIMILARITY: f64 = similarity(100, 144);

/// Aquifer grid sampling extents and surface-level tuning constants.
const X_RANGE: i32 = 10;
const Y_RANGE: i32 = 9;
const Z_RANGE: i32 = 10;
const Y_SPACING: i32 = 12;
const SURFACE_LEVEL_Y_OFFSET: i32 = 8;
const WAY_BELOW_MIN_Y: i32 = -1_000_000;
/// Preliminary-surface LRU capacity.
const PRELIMINARY_SURFACE_CACHE_CAP: usize = 64;

/// Chunk-space sampling offsets for the preliminary surface scan.
const SURFACE_SAMPLING_OFFSETS_IN_CHUNKS: [[i32; 2]; 13] = [
    [0, 0],
    [-2, -1],
    [-1, -1],
    [0, -1],
    [1, -1],
    [-3, 0],
    [-2, 0],
    [-1, 0],
    [1, 0],
    [-2, 1],
    [-1, 1],
    [0, 1],
    [1, 1],
];

// ---------------------------------------------------------------------------
// FluidPicker / FluidStatus
// ---------------------------------------------------------------------------

/// Per-position fluid lookup.
pub trait FluidPicker {
    /// Returns the fluid status at a block position.
    fn compute_fluid(&self, block_x: i32, block_y: i32, block_z: i32) -> FluidStatus;
}

/// Fluid level plus fluid block type at one aquifer cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluidStatus {
    pub fluid_level: i32,
    pub fluid_type: BlockRuntimeId,
}

impl FluidStatus {
    /// Returns the fluid block below the fluid level, else air.
    pub fn at(&self, block_y: i32, air: BlockRuntimeId) -> BlockRuntimeId {
        if block_y < self.fluid_level {
            self.fluid_type
        } else {
            air
        }
    }
}

/// Overworld fluid picker: lava below the lava threshold, water otherwise.
pub struct OverworldFluidPicker {
    lava_level: i32,
    lava_threshold: i32,
    sea_level: i32,
    water: BlockRuntimeId,
    lava: BlockRuntimeId,
}

/// Builds the overworld fluid picker for a sea level.
pub fn overworld_fluid_picker(sea_level: i32, blocks: &MaterialBlocks) -> OverworldFluidPicker {
    let lava_level = -54;
    let lava_threshold = lava_level.min(sea_level);
    OverworldFluidPicker {
        lava_level,
        lava_threshold,
        sea_level,
        water: blocks.water,
        lava: blocks.lava,
    }
}

impl FluidPicker for OverworldFluidPicker {
    fn compute_fluid(&self, _block_x: i32, block_y: i32, _block_z: i32) -> FluidStatus {
        if block_y < self.lava_threshold {
            FluidStatus {
                fluid_level: self.lava_level,
                fluid_type: self.lava,
            }
        } else {
            FluidStatus {
                fluid_level: self.sea_level,
                fluid_type: self.water,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Bounded LRU map for the preliminary-surface cache
// ---------------------------------------------------------------------------

/// Bounded LRU map from packed coordinates to surface levels (miss = `i32::MIN`).
struct LruLongIntMap {
    /// Front is oldest, back is newest.
    entries: VecDeque<(i64, i32)>,
    /// Construction capacity (eviction uses `PRELIMINARY_SURFACE_CACHE_CAP`).
    #[allow(dead_code)]
    cap: usize,
}

impl LruLongIntMap {
    fn new(cap: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Looks up a key and moves the hit to the back (miss returns `i32::MIN`).
    fn get_and_move_to_last(&mut self, key: i64) -> i32 {
        if let Some(pos) = self.entries.iter().position(|&(k, _)| k == key) {
            let (_, value) = self.entries.remove(pos).expect("position just found");
            self.entries.push_back((key, value));
            value
        } else {
            i32::MIN
        }
    }

    /// Inserts or overwrites a key, moving it to the back.
    fn put_and_move_to_last(&mut self, key: i64, value: i32) {
        if let Some(pos) = self.entries.iter().position(|&(k, _)| k == key) {
            self.entries.remove(pos).expect("position just found");
        }
        self.entries.push_back((key, value));
    }

    /// Evict the oldest entry.
    fn remove_first(&mut self) {
        self.entries.pop_front();
    }

    /// Java: `size()`.
    fn len(&self) -> usize {
        self.entries.len()
    }
}

// ---------------------------------------------------------------------------
// CachedPointContext
// ---------------------------------------------------------------------------

/// Java: `private static final class CachedPointContext implements ChunkCacheContext`(L637-673).
struct CachedPointContext {
    chunk_cache: Rc<ChunkCache>,
    block_x: Cell<i32>,
    block_y: Cell<i32>,
    block_z: Cell<i32>,
}

impl CachedPointContext {
    /// Constructor.
    fn new(chunk_cache: Rc<ChunkCache>) -> Self {
        Self {
            chunk_cache,
            block_x: Cell::new(0),
            block_y: Cell::new(0),
            block_z: Cell::new(0),
        }
    }

    /// Set the cursor position (builder style, returns this).
    fn set(&self, block_x: i32, block_y: i32, block_z: i32) -> &Self {
        self.block_x.set(block_x);
        self.block_y.set(block_y);
        self.block_z.set(block_z);
        self
    }
}

impl FunctionContext for CachedPointContext {
    fn block_x(&self) -> i32 {
        self.block_x.get()
    }

    fn block_y(&self) -> i32 {
        self.block_y.get()
    }

    fn block_z(&self) -> i32 {
        self.block_z.get()
    }

    fn as_chunk_cache_context(&self) -> Option<&dyn ChunkCacheContext> {
        Some(self)
    }
}

impl ChunkCacheContext for CachedPointContext {
    fn density_chunk_cache(&self) -> Rc<ChunkCache> {
        Rc::clone(&self.chunk_cache)
    }
}

// ---------------------------------------------------------------------------
// Aquifer
// ---------------------------------------------------------------------------

/// Java: `public final class Aquifer`(L20-674).
pub struct Aquifer {
    barrier_noise: Arc<dyn DensityFunction>,
    fluid_level_floodedness_noise: Arc<dyn DensityFunction>,
    fluid_level_spread_noise: Arc<dyn DensityFunction>,
    lava_noise: Arc<dyn DensityFunction>,
    erosion: Arc<dyn DensityFunction>,
    depth: Arc<dyn DensityFunction>,
    preliminary_surface_density: Arc<dyn DensityFunction>,
    preliminary_surface_upper_bound: Arc<dyn DensityFunction>,
    /// Lazily-filled aquifer cache.
    aquifer_cache: RefCell<Vec<Option<FluidStatus>>>,
    /// Java: `long[] aquiferLocationCache`(L42).
    aquifer_location_cache: Vec<i64>,
    /// Java: `short[] aquiferOffsetCache`(L43).
    aquifer_offset_cache: Vec<i16>,
    global_fluid_picker: Arc<dyn FluidPicker>,
    skip_sampling_above_y: Cell<i32>,
    min_y: i32,
    max_y: i32,
    min_grid_x: i32,
    min_grid_y: i32,
    min_grid_z: i32,
    grid_size_x: i32,
    grid_size_z: i32,
    /// Seed used only by the construction-time preload;
    /// the field mirrors the upstream class shape.
    #[allow(dead_code)]
    random_seed: i64,
    preliminary_surface_lower_bound: i32,
    preliminary_surface_cell_height: i32,
    cached_point_context: CachedPointContext,
    preliminary_surface_level_cache: RefCell<LruLongIntMap>,
    cached_barrier_noise: Cell<f64>,
    should_schedule_fluid_update: Cell<bool>,
    blocks: MaterialBlocks,
}

impl Aquifer {
    /// Constructor.
    ///
    /// `chunk_x`/`chunk_z` are chunk coordinates, `level_seed` the world seed;
    /// `chunk_cache` is the shared chunk cache.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chunk_x: i32,
        chunk_z: i32,
        level_seed: i64,
        chunk_cache: Rc<ChunkCache>,
        barrier_noise: Arc<dyn DensityFunction>,
        fluid_level_floodedness_noise: Arc<dyn DensityFunction>,
        fluid_level_spread_noise: Arc<dyn DensityFunction>,
        lava_noise: Arc<dyn DensityFunction>,
        erosion: Arc<dyn DensityFunction>,
        depth: Arc<dyn DensityFunction>,
        preliminary_surface_density: Arc<dyn DensityFunction>,
        preliminary_surface_upper_bound: Arc<dyn DensityFunction>,
        preliminary_surface_lower_bound: i32,
        preliminary_surface_cell_height: i32,
        min_block_y: i32,
        y_block_size: i32,
        global_fluid_picker: Arc<dyn FluidPicker>,
        blocks: MaterialBlocks,
    ) -> Self {
        let random_seed = level_seed ^ 0x4f99_39f5_08;
        let min_y = min_block_y;
        let max_y = min_block_y + y_block_size - 1;

        let min_block_x = chunk_x << 4;
        let max_block_x = min_block_x + 15;
        let min_block_z = chunk_z << 4;
        let max_block_z = min_block_z + 15;

        let min_grid_x = grid_x(min_block_x - 5);
        let max_grid_x = grid_x(max_block_x - 5) + 1;
        let grid_size_x = max_grid_x - min_grid_x + 1;
        let min_grid_y = grid_y(min_block_y + 1) - 1;
        let max_grid_y = grid_y(min_block_y + y_block_size + 1) + 1;
        let grid_size_y = max_grid_y - min_grid_y + 1;
        let min_grid_z = grid_z(min_block_z - 5);
        let max_grid_z = grid_z(max_block_z - 5) + 1;
        let grid_size_z = max_grid_z - min_grid_z + 1;

        let total_grid_size = (grid_size_x * grid_size_y * grid_size_z) as usize;
        let mut aquifer_cache = Vec::with_capacity(total_grid_size);
        aquifer_cache.resize_with(total_grid_size, || None);
        let mut aquifer_location_cache = vec![0i64; total_grid_size];
        let mut aquifer_offset_cache = vec![0i16; total_grid_size];

        preload_aquifer_locations(
            &mut aquifer_offset_cache,
            &mut aquifer_location_cache,
            random_seed,
            min_grid_x,
            min_grid_y,
            min_grid_z,
            grid_size_x,
            grid_size_z,
            grid_size_y,
        );

        let this = Self {
            barrier_noise,
            fluid_level_floodedness_noise,
            fluid_level_spread_noise,
            lava_noise,
            erosion,
            depth,
            preliminary_surface_density,
            preliminary_surface_upper_bound,
            aquifer_cache: RefCell::new(aquifer_cache),
            aquifer_location_cache,
            aquifer_offset_cache,
            global_fluid_picker,
            skip_sampling_above_y: Cell::new(0),
            min_y,
            max_y,
            min_grid_x,
            min_grid_y,
            min_grid_z,
            grid_size_x,
            grid_size_z,
            random_seed,
            preliminary_surface_lower_bound,
            preliminary_surface_cell_height,
            cached_point_context: CachedPointContext::new(chunk_cache),
            preliminary_surface_level_cache: RefCell::new(LruLongIntMap::new(
                PRELIMINARY_SURFACE_CACHE_CAP,
            )),
            cached_barrier_noise: Cell::new(f64::NAN),
            should_schedule_fluid_update: Cell::new(false),
            blocks,
        };

        // Construction tail (needs initialized cache fields).
        let max_adjusted_surface_level = adjust_surface_level(this.max_preliminary_surface_level(
            from_grid_x(min_grid_x, 0),
            from_grid_z(min_grid_z, 0),
            from_grid_x(max_grid_x, 9),
            from_grid_z(max_grid_z, 9),
        ));
        let skip_sampling_above_grid_y = grid_y(max_adjusted_surface_level + 12) + 1;
        this.skip_sampling_above_y
            .set(from_grid_y(skip_sampling_above_grid_y, 11) - 1);

        this
    }

    /// Java: `@Nullable BlockState computeSubstance(FunctionContext, double)`(L139-272).
    pub fn compute_substance(
        &self,
        context: &dyn FunctionContext,
        density: f64,
    ) -> Option<BlockRuntimeId> {
        if density > 0.0 {
            self.should_schedule_fluid_update.set(false);
            return None;
        }

        let pos_x = context.block_x();
        let pos_y = context.block_y();
        let pos_z = context.block_z();
        let global_fluid = self.global_fluid_picker.compute_fluid(pos_x, pos_y, pos_z);
        let global_at_pos = global_fluid.at(pos_y, self.blocks.air);

        if pos_y > self.skip_sampling_above_y.get() {
            self.should_schedule_fluid_update.set(false);
            return Some(global_at_pos);
        }

        if is_lava(global_at_pos, &self.blocks) {
            self.should_schedule_fluid_update.set(false);
            return Some(self.blocks.lava);
        }

        let x_anchor = grid_x(pos_x - 5);
        let y_anchor = grid_y(pos_y + 1);
        let z_anchor = grid_z(pos_z - 5);

        let mut distance_sqr1 = i32::MAX;
        let mut distance_sqr2 = i32::MAX;
        let mut distance_sqr3 = i32::MAX;
        let mut distance_sqr4 = i32::MAX;
        let mut closest_index1 = 0usize;
        let mut closest_index2 = 0usize;
        let mut closest_index3 = 0usize;
        let mut closest_index4 = 0usize;

        for x1 in 0..=1 {
            for y1 in -1..=1 {
                for z1 in 0..=1 {
                    let spaced_grid_x = x_anchor + x1;
                    let spaced_grid_y = y_anchor + y1;
                    let spaced_grid_z = z_anchor + z1;
                    let index = self.get_index(spaced_grid_x, spaced_grid_y, spaced_grid_z);
                    let packed_offset = self.aquifer_offset_cache[index];
                    let dx = from_grid_x(spaced_grid_x, unpack_offset_x(packed_offset)) - pos_x;
                    let dy = from_grid_y(spaced_grid_y, unpack_offset_y(packed_offset)) - pos_y;
                    let dz = from_grid_z(spaced_grid_z, unpack_offset_z(packed_offset)) - pos_z;
                    let new_distance = dx * dx + dy * dy + dz * dz;
                    if distance_sqr1 >= new_distance {
                        closest_index4 = closest_index3;
                        closest_index3 = closest_index2;
                        closest_index2 = closest_index1;
                        closest_index1 = index;
                        distance_sqr4 = distance_sqr3;
                        distance_sqr3 = distance_sqr2;
                        distance_sqr2 = distance_sqr1;
                        distance_sqr1 = new_distance;
                    } else if distance_sqr2 >= new_distance {
                        closest_index4 = closest_index3;
                        closest_index3 = closest_index2;
                        closest_index2 = index;
                        distance_sqr4 = distance_sqr3;
                        distance_sqr3 = distance_sqr2;
                        distance_sqr2 = new_distance;
                    } else if distance_sqr3 >= new_distance {
                        closest_index4 = closest_index3;
                        closest_index3 = index;
                        distance_sqr4 = distance_sqr3;
                        distance_sqr3 = new_distance;
                    } else if distance_sqr4 >= new_distance {
                        closest_index4 = index;
                        distance_sqr4 = new_distance;
                    }
                }
            }
        }

        let closest_status1 = self.get_aquifer_status(closest_index1);
        let similarity12 = similarity(distance_sqr1, distance_sqr2);
        let fluid_state = closest_status1.at(pos_y, self.blocks.air);
        if similarity12 <= 0.0 {
            if similarity12 >= FLOWING_UPDATE_SIMILARITY {
                let closest_status2 = self.get_aquifer_status(closest_index2);
                self.should_schedule_fluid_update
                    .set(closest_status1 != closest_status2);
            } else {
                self.should_schedule_fluid_update.set(false);
            }
            return Some(fluid_state);
        }

        if is_water(fluid_state, &self.blocks)
            && is_lava(
                self.global_fluid_picker
                    .compute_fluid(pos_x, pos_y - 1, pos_z)
                    .at(pos_y - 1, self.blocks.air),
                &self.blocks,
            )
        {
            self.should_schedule_fluid_update.set(true);
            return Some(fluid_state);
        }

        self.cached_barrier_noise.set(f64::NAN);
        let closest_status2 = self.get_aquifer_status(closest_index2);
        let barrier12 =
            similarity12 * self.calculate_pressure(context, &closest_status1, &closest_status2);
        if density + barrier12 > 0.0 {
            self.should_schedule_fluid_update.set(false);
            return Some(self.blocks.stone);
        }

        let closest_status3 = self.get_aquifer_status(closest_index3);
        let similarity13 = similarity(distance_sqr1, distance_sqr3);
        if similarity13 > 0.0 {
            let barrier13 = similarity12
                * similarity13
                * self.calculate_pressure(context, &closest_status1, &closest_status3);
            if density + barrier13 > 0.0 {
                self.should_schedule_fluid_update.set(false);
                return Some(self.blocks.stone);
            }
        }

        let similarity23 = similarity(distance_sqr2, distance_sqr3);
        if similarity23 > 0.0 {
            let barrier23 = similarity12
                * similarity23
                * self.calculate_pressure(context, &closest_status2, &closest_status3);
            if density + barrier23 > 0.0 {
                self.should_schedule_fluid_update.set(false);
                return Some(self.blocks.stone);
            }
        }

        let may_flow12 = closest_status1 != closest_status2;
        let may_flow23 =
            similarity23 >= FLOWING_UPDATE_SIMILARITY && closest_status2 != closest_status3;
        let may_flow13 =
            similarity13 >= FLOWING_UPDATE_SIMILARITY && closest_status1 != closest_status3;
        if !may_flow12 && !may_flow23 && !may_flow13 {
            self.should_schedule_fluid_update.set(
                similarity13 >= FLOWING_UPDATE_SIMILARITY
                    && similarity(distance_sqr1, distance_sqr4) >= FLOWING_UPDATE_SIMILARITY
                    && closest_status1 != self.get_aquifer_status(closest_index4),
            );
        } else {
            self.should_schedule_fluid_update.set(true);
        }

        Some(fluid_state)
    }

    /// Java: `boolean shouldScheduleFluidUpdate()`(L274-276).
    pub fn should_schedule_fluid_update(&self) -> bool {
        self.should_schedule_fluid_update.get()
    }

    /// Java: `private int getIndex(int, int, int)`(L278-283).
    fn get_index(&self, grid_x: i32, grid_y: i32, grid_z: i32) -> usize {
        let x = grid_x - self.min_grid_x;
        let y = grid_y - self.min_grid_y;
        let z = grid_z - self.min_grid_z;
        ((y * self.grid_size_z + z) * self.grid_size_x + x) as usize
    }

    /// Java: `private double calculatePressure(FunctionContext, FluidStatus, FluidStatus)`(L309-368).
    fn calculate_pressure(
        &self,
        context: &dyn FunctionContext,
        status_closest1: &FluidStatus,
        status_closest2: &FluidStatus,
    ) -> f64 {
        let pos_y = context.block_y();
        let type1 = status_closest1.at(pos_y, self.blocks.air);
        let type2 = status_closest2.at(pos_y, self.blocks.air);
        if (!is_lava(type1, &self.blocks) || !is_water(type2, &self.blocks))
            && (!is_water(type1, &self.blocks) || !is_lava(type2, &self.blocks))
        {
            let fluid_y_diff = status_closest1
                .fluid_level
                .wrapping_sub(status_closest2.fluid_level)
                .abs();
            if fluid_y_diff == 0 {
                return 0.0;
            }

            let average_fluid_y = 0.5
                * (status_closest1
                    .fluid_level
                    .wrapping_add(status_closest2.fluid_level) as f64);
            let how_far_above_average_fluid_point = pos_y as f64 + 0.5 - average_fluid_y;
            let base_value = fluid_y_diff as f64 / 2.0;
            let top_bias = 0.0;
            let furthest_rocks_from_top_bias = 2.5;
            let furthest_holes_from_top_bias = 1.5;
            let bottom_bias = 3.0;
            let furthest_rocks_from_bottom_bias = 10.0;
            let furthest_holes_from_bottom_bias = 3.0;
            let distance_from_barrier_edge_towards_middle =
                base_value - how_far_above_average_fluid_point.abs();
            let gradient;
            if how_far_above_average_fluid_point > 0.0 {
                let center_point = top_bias + distance_from_barrier_edge_towards_middle;
                if center_point > 0.0 {
                    gradient = center_point / furthest_holes_from_top_bias;
                } else {
                    gradient = center_point / furthest_rocks_from_top_bias;
                }
            } else {
                let center_point = bottom_bias + distance_from_barrier_edge_towards_middle;
                if center_point > 0.0 {
                    gradient = center_point / furthest_holes_from_bottom_bias;
                } else {
                    gradient = center_point / furthest_rocks_from_bottom_bias;
                }
            }

            let amplitude = 2.0;
            let noise_value;
            if (-amplitude..=amplitude).contains(&gradient) {
                let current_noise_value = self.cached_barrier_noise.get();
                if current_noise_value.is_nan() {
                    let barrier_noise = self.barrier_noise.compute(context);
                    self.cached_barrier_noise.set(barrier_noise);
                    noise_value = barrier_noise;
                } else {
                    noise_value = current_noise_value;
                }
            } else {
                noise_value = 0.0;
            }

            return amplitude * (noise_value + gradient);
        }
        2.0
    }

    /// Java: `private FluidStatus getAquiferStatus(int)`(L370-379).
    fn get_aquifer_status(&self, index: usize) -> FluidStatus {
        if let Some(old_status) = self.aquifer_cache.borrow()[index] {
            return old_status;
        }
        let location = self.aquifer_location_cache[index];
        let status = self.compute_fluid(unpack_x(location), unpack_y(location), unpack_z(location));
        self.aquifer_cache.borrow_mut()[index] = Some(status);
        status
    }

    /// Java: `private FluidStatus computeFluid(int, int, int)`(L381-415).
    fn compute_fluid(&self, x: i32, y: i32, z: i32) -> FluidStatus {
        let global_fluid = self.global_fluid_picker.compute_fluid(x, y, z);
        let mut lowest_preliminary_surface = i32::MAX;
        let top_of_aquifer_cell = y + Y_SPACING;
        let bottom_of_aquifer_cell = y - Y_SPACING;
        let mut surface_at_center_is_under_global_fluid_level = false;

        for offset in &SURFACE_SAMPLING_OFFSETS_IN_CHUNKS {
            let sample_x = x + (offset[0] << 4);
            let sample_z = z + (offset[1] << 4);
            let preliminary_surface_level = self.preliminary_surface_level(sample_x, sample_z);
            let adjusted_surface_level = adjust_surface_level(preliminary_surface_level);
            let start = offset[0] == 0 && offset[1] == 0;
            if start && bottom_of_aquifer_cell > adjusted_surface_level {
                return global_fluid;
            }

            let top_of_aquifer_cell_pokes_above_surface =
                top_of_aquifer_cell > adjusted_surface_level;
            if top_of_aquifer_cell_pokes_above_surface || start {
                let global_fluid_at_surface = self.global_fluid_picker.compute_fluid(
                    sample_x,
                    adjusted_surface_level,
                    sample_z,
                );
                if !is_air(
                    global_fluid_at_surface.at(adjusted_surface_level, self.blocks.air),
                    &self.blocks,
                ) {
                    if start {
                        surface_at_center_is_under_global_fluid_level = true;
                    }
                    if top_of_aquifer_cell_pokes_above_surface {
                        return global_fluid_at_surface;
                    }
                }
            }
            lowest_preliminary_surface = lowest_preliminary_surface.min(preliminary_surface_level);
        }

        let fluid_surface_level = self.compute_surface_level(
            x,
            y,
            z,
            &global_fluid,
            lowest_preliminary_surface,
            surface_at_center_is_under_global_fluid_level,
        );
        FluidStatus {
            fluid_level: fluid_surface_level,
            fluid_type: self.compute_fluid_type(x, y, z, &global_fluid, fluid_surface_level),
        }
    }

    /// Java: `private int computeSurfaceLevel(int, int, int, FluidStatus, int, boolean)`(L421-452).
    fn compute_surface_level(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global_fluid: &FluidStatus,
        lowest_preliminary_surface: i32,
        surface_at_center_is_under_global_fluid_level: bool,
    ) -> i32 {
        let context = self.cached_point_context.set(x, y, z);
        let partially_floodedness;
        let fully_floodedness;
        if is_deep_dark_region(&self.erosion, &self.depth, context) {
            partially_floodedness = -1.0;
            fully_floodedness = -1.0;
        } else {
            let distance_below_surface = lowest_preliminary_surface + SURFACE_LEVEL_Y_OFFSET - y;
            let floodedness_factor = if surface_at_center_is_under_global_fluid_level {
                clamped_map(distance_below_surface as f64, 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let floodedness_noise_value = clamp_f64(
                self.fluid_level_floodedness_noise.compute(context),
                -1.0,
                1.0,
            );
            let fully_flooded_threshold = map(floodedness_factor, 1.0, 0.0, -0.3, 0.8);
            let partially_flooded_threshold = map(floodedness_factor, 1.0, 0.0, -0.8, 0.4);
            partially_floodedness = floodedness_noise_value - partially_flooded_threshold;
            fully_floodedness = floodedness_noise_value - fully_flooded_threshold;
        }

        if fully_floodedness > 0.0 {
            return global_fluid.fluid_level;
        }
        if partially_floodedness > 0.0 {
            return self.compute_randomized_fluid_surface_level(
                x,
                y,
                z,
                lowest_preliminary_surface,
            );
        }
        WAY_BELOW_MIN_Y
    }

    /// Java: `private int computeRandomizedFluidSurfaceLevel(int, int, int, int)`(L454-465).
    fn compute_randomized_fluid_surface_level(
        &self,
        x: i32,
        y: i32,
        z: i32,
        lowest_preliminary_surface: i32,
    ) -> i32 {
        let fluid_level_cell_x = x.div_euclid(16);
        let fluid_level_cell_y = y.div_euclid(40);
        let fluid_level_cell_z = z.div_euclid(16);
        let fluid_cell_middle_y = fluid_level_cell_y * 40 + 20;
        let fluid_level_spread =
            self.fluid_level_spread_noise
                .compute(self.cached_point_context.set(
                    fluid_level_cell_x,
                    fluid_level_cell_y,
                    fluid_level_cell_z,
                ))
                * 10.0;
        let fluid_level_spread_quantized = quantize(fluid_level_spread, 3);
        let target_fluid_surface_level = fluid_cell_middle_y + fluid_level_spread_quantized;
        lowest_preliminary_surface.min(target_fluid_surface_level)
    }

    /// Java: `private BlockState computeFluidType(int, int, int, FluidStatus, int)`(L467-479).
    fn compute_fluid_type(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global_fluid: &FluidStatus,
        fluid_surface_level: i32,
    ) -> BlockRuntimeId {
        let mut fluid_type = global_fluid.fluid_type;
        if fluid_surface_level <= -10
            && fluid_surface_level != WAY_BELOW_MIN_Y
            && !is_lava(global_fluid.fluid_type, &self.blocks)
        {
            let fluid_type_cell_x = x.div_euclid(64);
            let fluid_type_cell_y = y.div_euclid(40);
            let fluid_type_cell_z = z.div_euclid(64);
            let lava_noise_value = self.lava_noise.compute(self.cached_point_context.set(
                fluid_type_cell_x,
                fluid_type_cell_y,
                fluid_type_cell_z,
            ));
            if lava_noise_value.abs() > 0.3 {
                fluid_type = self.blocks.lava;
            }
        }
        fluid_type
    }

    /// Java: `private int preliminarySurfaceLevel(int, int)`(L481-506).
    fn preliminary_surface_level(&self, world_x: i32, world_z: i32) -> i32 {
        let key = ((world_x as i64) << 32) ^ (world_z as i64 & 0xFFFF_FFFF);
        let cached = self
            .preliminary_surface_level_cache
            .borrow_mut()
            .get_and_move_to_last(key);
        if cached != i32::MIN {
            return cached;
        }

        let lower_y = self.min_y.max(self.preliminary_surface_lower_bound);
        let mut upper_y = self
            .preliminary_surface_upper_bound
            .compute(self.cached_point_context.set(world_x, 0, world_z))
            .floor() as i32;
        upper_y = self.max_y.min(
            upper_y.div_euclid(self.preliminary_surface_cell_height)
                * self.preliminary_surface_cell_height,
        );

        let mut result = lower_y;
        if upper_y > lower_y {
            let mut y = upper_y;
            while y >= lower_y {
                if self
                    .preliminary_surface_density
                    .compute(self.cached_point_context.set(world_x, y, world_z))
                    > 0.0
                {
                    result = y;
                    break;
                }
                y -= self.preliminary_surface_cell_height;
            }
        }
        let mut cache = self.preliminary_surface_level_cache.borrow_mut();
        cache.put_and_move_to_last(key, result);
        if cache.len() > PRELIMINARY_SURFACE_CACHE_CAP {
            cache.remove_first();
        }
        result
    }

    /// Java: `private int maxPreliminarySurfaceLevel(int, int, int, int)`(L508-516).
    fn max_preliminary_surface_level(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> i32 {
        let mut max_surface = i32::MIN;
        let mut x = min_x;
        while x <= max_x {
            let mut z = min_z;
            while z <= max_z {
                max_surface = max_surface.max(self.preliminary_surface_level(x, z));
                z += 4;
            }
            x += 4;
        }
        if max_surface == i32::MIN {
            63
        } else {
            max_surface
        }
    }
}

// ---------------------------------------------------------------------------
// Static utilities.
// ---------------------------------------------------------------------------

/// Preload aquifer locations (construction-time only).
///
/// Standalone function borrowing a local array.
#[allow(clippy::too_many_arguments)]
fn preload_aquifer_locations(
    aquifer_offset_cache: &mut [i16],
    aquifer_location_cache: &mut [i64],
    random_seed: i64,
    min_grid_x: i32,
    min_grid_y: i32,
    min_grid_z: i32,
    grid_size_x: i32,
    grid_size_z: i32,
    grid_size_y: i32,
) {
    // Per-use RNG is always reseeded first, so the initial seed
    // never affects output; it starts at 0.
    let mut random = Xoroshiro128::new(0);
    for y in 0..grid_size_y {
        let grid_y = min_grid_y + y;
        for z in 0..grid_size_z {
            let grid_z = min_grid_z + z;
            for x in 0..grid_size_x {
                let grid_x = min_grid_x + x;
                random.set_seed(mix_seed(random_seed, grid_x, grid_y, grid_z));
                let offset_x = random.next_int_max(X_RANGE);
                let offset_y = random.next_int_max(Y_RANGE);
                let offset_z = random.next_int_max(Z_RANGE);
                let index = ((y * grid_size_z + z) * grid_size_x + x) as usize;
                aquifer_offset_cache[index] = pack_offset(offset_x, offset_y, offset_z);
                aquifer_location_cache[index] = pack(
                    from_grid_x(grid_x, offset_x),
                    from_grid_y(grid_y, offset_y),
                    from_grid_z(grid_z, offset_z),
                );
            }
        }
    }
}

/// Java: `private static int adjustSurfaceLevel(int)`(L417-419).
fn adjust_surface_level(preliminary_surface_level: i32) -> i32 {
    preliminary_surface_level + SURFACE_LEVEL_Y_OFFSET
}

/// Java: `private static int gridX(int)`(L518-520).
fn grid_x(block_coord: i32) -> i32 {
    block_coord >> 4
}

/// Java: `private static int fromGridX(int, int)`(L522-524).
fn from_grid_x(grid_coord: i32, block_offset: i32) -> i32 {
    (grid_coord << 4) + block_offset
}

/// Java: `private static int gridY(int)`(L526-528)——`Math.floorDiv(..., 12)`.
fn grid_y(block_coord: i32) -> i32 {
    block_coord.div_euclid(12)
}

/// Java: `private static int fromGridY(int, int)`(L530-532).
fn from_grid_y(grid_coord: i32, block_offset: i32) -> i32 {
    grid_coord * 12 + block_offset
}

/// Java: `private static int gridZ(int)`(L534-536).
fn grid_z(block_coord: i32) -> i32 {
    block_coord >> 4
}

/// Java: `private static int fromGridZ(int, int)`(L538-540).
fn from_grid_z(grid_coord: i32, block_offset: i32) -> i32 {
    (grid_coord << 4) + block_offset
}

/// Java: `private static long pack(int, int, int)`(L542-544).
fn pack(x: i32, y: i32, z: i32) -> i64 {
    (((x & 0x3FF_FFFF) as i64) << 38) | (((z & 0x3FF_FFFF) as i64) << 12) | (y & 0xFFF) as i64
}

/// Java: `private static short packOffset(int, int, int)`(L546-548).
fn pack_offset(x: i32, y: i32, z: i32) -> i16 {
    ((x << 8) | (y << 4) | z) as i16
}

/// Java: `private static int unpackOffsetX(int)`(L550-552).
fn unpack_offset_x(packed: i16) -> i32 {
    ((packed as i32) >> 8) & 0xF
}

/// Java: `private static int unpackOffsetY(int)`(L554-556).
fn unpack_offset_y(packed: i16) -> i32 {
    ((packed as i32) >> 4) & 0xF
}

/// Java: `private static int unpackOffsetZ(int)`(L558-560).
fn unpack_offset_z(packed: i16) -> i32 {
    (packed as i32) & 0xF
}

/// Java: `private static int unpackX(long)`(L562-564).
fn unpack_x(packed: i64) -> i32 {
    (packed >> 38) as i32
}

/// Java: `private static int unpackY(long)`(L566-568).
fn unpack_y(packed: i64) -> i32 {
    ((packed << 52) >> 52) as i32
}

/// Java: `private static int unpackZ(long)`(L570-572).
fn unpack_z(packed: i64) -> i32 {
    ((packed << 26) >> 38) as i32
}

/// Java: `private static boolean isDeepDarkRegion(DensityFunction, DensityFunction, FunctionContext)`(L574-576).
fn is_deep_dark_region(
    erosion: &Arc<dyn DensityFunction>,
    depth: &Arc<dyn DensityFunction>,
    context: &dyn FunctionContext,
) -> bool {
    erosion.compute(context) < -0.225 && depth.compute(context) > 0.9
}

/// Java: `static double similarity(int, int)`(L578-580).
///
/// `FLOWING_UPDATE_SIMILARITY` needs const evaluation;
/// computed in a const fn.
const fn similarity(distance_sqr1: i32, distance_sqr2: i32) -> f64 {
    1.0 - (distance_sqr2 - distance_sqr1) as f64 / 25.0
}

/// Java: `static double clampedMap(double, double, double, double, double)`(L582-588).
fn clamped_map(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    if in_min == in_max {
        return if value < in_min { out_min } else { out_max };
    }
    let t = clamp_f64((value - in_min) / (in_max - in_min), 0.0, 1.0);
    out_min + (out_max - out_min) * t
}

/// Java: `static double map(double, double, double, double, double)`(L590-596).
fn map(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    if in_min == in_max {
        return out_min;
    }
    let t = (value - in_min) / (in_max - in_min);
    out_min + (out_max - out_min) * t
}

/// Java: `static int quantize(double, int)`(L598-600).
fn quantize(value: f64, step: i32) -> i32 {
    (value / step as f64).floor() as i32 * step
}

/// Java: `static long mixSeed(long, int, int, int)`(L602-613).
fn mix_seed(seed: i64, x: i32, y: i32, z: i32) -> i64 {
    let mut mixed = seed;
    mixed ^= (x as i64).wrapping_mul(341873128712);
    mixed ^= (y as i64).wrapping_mul(132897987541);
    mixed ^= (z as i64).wrapping_mul(42317861);
    mixed ^= ((mixed as u64) >> 33) as i64;
    mixed = mixed.wrapping_mul(0xff51afd7ed558ccd_u64 as i64);
    mixed ^= ((mixed as u64) >> 33) as i64;
    mixed = mixed.wrapping_mul(0xc4ceb9fe1a85ec53_u64 as i64);
    mixed ^= ((mixed as u64) >> 33) as i64;
    mixed
}

/// Lava check by runtime id equality.
fn is_lava(state: BlockRuntimeId, blocks: &MaterialBlocks) -> bool {
    state == blocks.lava
}

/// Java: `static boolean isWater(BlockState)`(L619-621).
fn is_water(state: BlockRuntimeId, blocks: &MaterialBlocks) -> bool {
    state == blocks.water
}

/// Java: `static boolean isAir(BlockState)`(L623-625).
fn is_air(state: BlockRuntimeId, blocks: &MaterialBlocks) -> bool {
    state == blocks.air
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::densityfunction::common::constant;
    use crate::worldgen::densityfunction::function::SinglePointContext;

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

    // -----------------------------------------------------------------------
    // Constants and static utilities.
    // -----------------------------------------------------------------------

    #[test]
    fn flowing_update_similarity_value() {
        // Java: similarity(100, 144) = 1 - 44/25 = -0.76
        assert_eq!(FLOWING_UPDATE_SIMILARITY, 1.0 - 44.0 / 25.0);
    }

    #[test]
    fn similarity_is_decreasing() {
        // Larger distance gaps mean lower similarity; equal distances give 1.0.
        assert_eq!(similarity(5, 5), 1.0);
        assert!(similarity(5, 10) < 1.0);
        assert!(similarity(5, 30) < similarity(5, 10));
    }

    #[test]
    fn pack_unpack_roundtrip() {
        // x/z pack into 25-bit fields (bit 25 shifts into the i64 sign bit,
        // sign-extending on arithmetic shift); y is a 12-bit signed field.
        for (x, y, z) in [
            (3i32, -60i32, 7i32),
            (0, 0, 0),
            (100, 2047, 65535),
            (0x1FF_FFFF, -2048, 0),
        ] {
            let packed = pack(x, y, z);
            assert_eq!(unpack_x(packed), x);
            assert_eq!(unpack_y(packed), y);
            assert_eq!(unpack_z(packed), z);
        }
    }

    #[test]
    fn pack_high_x_bit_sign_extends_like_java() {
        // Set bit 25 shifts into the sign bit, unpacking negative.
        let packed = pack(0x3FF_FFFF, 0, 0);
        assert_eq!(unpack_x(packed), -1);
    }

    #[test]
    fn pack_offset_unpack_roundtrip() {
        // Offset ranges: x/z in [0, X_RANGE), y in [0, Y_RANGE).
        for x in 0..X_RANGE {
            for y in 0..Y_RANGE {
                for z in 0..Z_RANGE {
                    let packed = pack_offset(x, y, z);
                    assert_eq!(unpack_offset_x(packed), x);
                    assert_eq!(unpack_offset_y(packed), y);
                    assert_eq!(unpack_offset_z(packed), z);
                }
            }
        }
    }

    #[test]
    fn grid_conversions() {
        // gridX/gridZ: >> 4 (arithmetic shift, negatives round down).
        assert_eq!(grid_x(-5), -1);
        assert_eq!(grid_x(15), 0);
        assert_eq!(grid_z(-1), -1);
        // gridY:floorDiv(..., 12)
        assert_eq!(grid_y(13), 1);
        assert_eq!(grid_y(-1), -1);
        assert_eq!(grid_y(-12), -1);
        assert_eq!(grid_y(0), 0);
        // fromGrid round-trips (block offsets 0..=15 / 0..=11).
        assert_eq!(from_grid_x(grid_x(100), 5), 101);
        assert_eq!(from_grid_y(grid_y(37), 11), 47);
        // grid_z(-100) = -7 (rounds down); -7<<4 + 3 = -109.
        assert_eq!(from_grid_z(grid_z(-100), 3), -109);
    }

    #[test]
    fn quantize_matches_java_floor() {
        assert_eq!(quantize(7.5, 3), 6);
        assert_eq!(quantize(-7.5, 3), -9);
        assert_eq!(quantize(0.0, 3), 0);
    }

    #[test]
    fn mix_seed_deterministic_and_position_sensitive() {
        let a = mix_seed(42, 10, -20, 30);
        assert_eq!(a, mix_seed(42, 10, -20, 30));
        assert_ne!(a, mix_seed(42, 11, -20, 30));
        assert_ne!(a, mix_seed(43, 10, -20, 30));
    }

    // -----------------------------------------------------------------------
    // Insertion-ordered int map equivalent.
    // -----------------------------------------------------------------------

    #[test]
    fn lru_map_semantics() {
        let mut map = LruLongIntMap::new(PRELIMINARY_SURFACE_CACHE_CAP);
        // Misses return the default (i32::MIN).
        assert_eq!(map.get_and_move_to_last(1), i32::MIN);
        map.put_and_move_to_last(1, 10);
        map.put_and_move_to_last(2, 20);
        assert_eq!(map.get_and_move_to_last(1), 10);
        // Overwriting a key moves it to the tail.
        map.put_and_move_to_last(2, 22);
        assert_eq!(map.get_and_move_to_last(2), 22);
        assert_eq!(map.len(), 2);
        // removeFirst evicts oldest (order [2, 1] becomes [1, 2]).
        map.remove_first();
        assert_eq!(map.get_and_move_to_last(1), i32::MIN);
        assert_eq!(map.get_and_move_to_last(2), 22);
    }

    // -----------------------------------------------------------------------
    // FluidPicker / FluidStatus
    // -----------------------------------------------------------------------

    #[test]
    fn fluid_status_at() {
        let blocks = test_blocks();
        let status = FluidStatus {
            fluid_level: 63,
            fluid_type: blocks.water,
        };
        assert_eq!(status.at(62, blocks.air), blocks.water);
        assert_eq!(status.at(63, blocks.air), blocks.air);
    }

    #[test]
    fn overworld_fluid_picker_thresholds() {
        let blocks = test_blocks();
        // seaLevel=63 ≥ lavaLevel=-54 → lavaThreshold = -54
        let picker = overworld_fluid_picker(63, &blocks);
        let below = picker.compute_fluid(0, -55, 0);
        assert_eq!(below.fluid_type, blocks.lava);
        assert_eq!(below.fluid_level, -54);
        let above = picker.compute_fluid(0, -54, 0);
        assert_eq!(above.fluid_type, blocks.water);
        assert_eq!(above.fluid_level, 63);
        // lavaThreshold equals seaLevel when sea is below lava level.
        let deep = overworld_fluid_picker(-60, &blocks);
        assert_eq!(deep.compute_fluid(0, -61, 0).fluid_type, blocks.lava);
        assert_eq!(deep.compute_fluid(0, -60, 0).fluid_type, blocks.water);
    }

    // -----------------------------------------------------------------------
    // Aquifer::computeSubstance
    // -----------------------------------------------------------------------

    /// Aquifer over all-constant density functions
    /// (surface at the lower bound -64; skipSamplingAboveY = -26).
    fn test_aquifer(blocks: &MaterialBlocks) -> Aquifer {
        Aquifer::new(
            0,
            0,
            42,
            Rc::new(ChunkCache::new()),
            constant(0.0), // barrierNoise
            constant(0.0), // fluidLevelFloodednessNoise
            constant(0.0), // fluidLevelSpreadNoise
            constant(0.0), // lavaNoise
            constant(1.0), // erosion (above -0.225: not deep dark)
            constant(0.0), // depth
            constant(0.0), // preliminarySurfaceDensity (always <= 0)
            constant(0.0), // preliminarySurfaceUpperBound
            -64,           // preliminarySurfaceLowerBound
            4,             // preliminarySurfaceCellHeight
            -64,           // minBlockY
            384,           // yBlockSize
            Arc::new(overworld_fluid_picker(63, blocks)),
            blocks.clone(),
        )
    }

    #[test]
    fn compute_substance_positive_density_returns_none() {
        let blocks = test_blocks();
        let aquifer = test_aquifer(&blocks);
        let ctx = SinglePointContext {
            block_x: 0,
            block_y: 0,
            block_z: 0,
        };
        assert!(aquifer.compute_substance(&ctx, 0.5).is_none());
        assert!(!aquifer.should_schedule_fluid_update());
    }

    #[test]
    fn compute_substance_above_skip_sampling_returns_global_fluid() {
        let blocks = test_blocks();
        let aquifer = test_aquifer(&blocks);
        // y=0 skips sampling: global fluid below sea level 63 is water.
        let ctx = SinglePointContext {
            block_x: 0,
            block_y: 0,
            block_z: 0,
        };
        assert_eq!(aquifer.compute_substance(&ctx, -0.5), Some(blocks.water));
    }

    #[test]
    fn compute_substance_lava_short_circuit() {
        let blocks = test_blocks();
        let aquifer = test_aquifer(&blocks);
        // y=-60 under the lava threshold: global fluid is lava, short-circuit.
        let ctx = SinglePointContext {
            block_x: 0,
            block_y: -60,
            block_z: 0,
        };
        assert_eq!(aquifer.compute_substance(&ctx, -1.0), Some(blocks.lava));
    }

    #[test]
    fn compute_substance_full_path_deterministic() {
        let blocks = test_blocks();
        let a1 = test_aquifer(&blocks);
        let a2 = test_aquifer(&blocks);
        // y=-40 takes the full 12-neighbor sampling path.
        let ctx = SinglePointContext {
            block_x: 8,
            block_y: -40,
            block_z: 8,
        };
        let r1 = a1.compute_substance(&ctx, -1.0);
        let r2 = a2.compute_substance(&ctx, -1.0);
        assert_eq!(r1, r2);
        // Every full-path branch returns Some (fluid/stone).
        assert!(r1.is_some());
        // Repeated evaluation hits the cache consistently.
        assert_eq!(a1.compute_substance(&ctx, -1.0), r1);
    }
}
