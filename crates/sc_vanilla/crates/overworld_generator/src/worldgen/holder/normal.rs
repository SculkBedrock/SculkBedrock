//! Normal world-generation object holder and its sub-holders.
//!
//! Holds 5 sub-holders: `BiomeHolder`, `TerrainHolder`, `SurfaceHolder`,
//! `SurfaceOverwriteHolder`, and `FeatureHolder`.
//!
//! Key points:
//! - The terrain holder reads biome noise through an explicitly passed `&BiomeHolder`.
//! - Per-chunk aquifer state lives in `Rc<RefCell<Option<Aquifer>>>` (single-threaded use).
//! - Material branches are dedicated structs implementing the filler trait.
//! - RNG order: `fork()` advances shared state, `identical()` does not; each
//!   holder constructor takes `&mut Xoroshiro128` to keep the exact sequence.

use crate::worldgen::densityfunction::base3d;
use crate::worldgen::densityfunction::caves;
use crate::worldgen::densityfunction::common::{cache_all_in_cell, noise_scaled_xy};
use crate::worldgen::densityfunction::continents;
use crate::worldgen::densityfunction::depth;
use crate::worldgen::densityfunction::erosion;
use crate::worldgen::densityfunction::factor;
use crate::worldgen::densityfunction::function::{DensityFunction, FunctionContext};
use crate::worldgen::densityfunction::jaggedness;
use crate::worldgen::densityfunction::offset;
use crate::worldgen::densityfunction::ore_veins;
use crate::worldgen::densityfunction::ridges;
use crate::worldgen::densityfunction::ridges_folded;
use crate::worldgen::densityfunction::sloped_cheese;
use crate::worldgen::holder::ObjectHolder;
use crate::worldgen::material::aquifer::{self, Aquifer, FluidPicker};
use crate::worldgen::material::filler::{MaterialFiller, MultiMaterial};
use crate::worldgen::material::ore_veinifier::OreVeinifier;
use crate::worldgen::material::MaterialBlocks;
use crate::worldgen::noise::f::SimplexF;
use crate::worldgen::noise::noise::NormalNoise;
use crate::worldgen::noise::simplex::SimplexNoise;
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use sc_world::chunk::BlockRuntimeId;

// ---------------------------------------------------------------------------
// AquiferMaterialFiller: aquifer branch of the material chain
// ---------------------------------------------------------------------------

/// Aquifer branch of the material chain.
///
/// Holds shared `Rc<RefCell<Option<Aquifer>>>` aquifer state plus the wrapped
/// (cell-cached) density function.
struct AquiferMaterialFiller {
    aquifer: Rc<RefCell<Option<Aquifer>>>,
    wrapped: Arc<dyn DensityFunction>,
}

impl MaterialFiller for AquiferMaterialFiller {
    /// Returns no block without an active aquifer, else the aquifer substance.
    fn calculate(&self, context: &dyn FunctionContext) -> Option<BlockRuntimeId> {
        let density = self.wrapped.compute(context);
        let borrow = self.aquifer.borrow();
        match borrow.as_ref() {
            None => None,
            Some(aquifer) => aquifer.compute_substance(context, density),
        }
    }
}

// ---------------------------------------------------------------------------
// StoneMaterialFiller: stone branch of the material chain
// ---------------------------------------------------------------------------

/// Stone branch of the material chain: stone where the density is positive.
struct StoneMaterialFiller {
    wrapped: Arc<dyn DensityFunction>,
    stone: BlockRuntimeId,
}

impl MaterialFiller for StoneMaterialFiller {
    fn calculate(&self, context: &dyn FunctionContext) -> Option<BlockRuntimeId> {
        if self.wrapped.compute(context) > 0.0 {
            Some(self.stone)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// NormalObjectHolder
// ---------------------------------------------------------------------------

/// Root holder owning the biome, terrain, surface, overwrite, and feature holders.
pub struct NormalObjectHolder {
    random: Xoroshiro128,
    biome_holder: BiomeHolder,
    terrain_holder: TerrainHolder,
    surface_holder: SurfaceHolder,
    surface_overwrite_holder: SurfaceOverwriteHolder,
    feature_holder: FeatureHolder,
}

impl NormalObjectHolder {
    /// Builds all sub-holders in RNG order.
    ///
    /// `blocks` injects the static block references.
    pub fn new(mut random: Xoroshiro128, blocks: MaterialBlocks) -> Self {
        let biome_holder = BiomeHolder::new(&mut random);
        let terrain_holder = TerrainHolder::new(&mut random, &biome_holder, blocks);
        let surface_holder = SurfaceHolder::new(&mut random);
        let surface_overwrite_holder = SurfaceOverwriteHolder::new(&mut random);
        let feature_holder = FeatureHolder::new(&mut random);
        Self {
            random,
            biome_holder,
            terrain_holder,
            surface_holder,
            surface_overwrite_holder,
            feature_holder,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn biome_holder(&self) -> &BiomeHolder {
        &self.biome_holder
    }
    pub fn terrain_holder(&self) -> &TerrainHolder {
        &self.terrain_holder
    }
    pub fn surface_holder(&self) -> &SurfaceHolder {
        &self.surface_holder
    }
    pub fn surface_overwrite_holder(&self) -> &SurfaceOverwriteHolder {
        &self.surface_overwrite_holder
    }
    pub fn feature_holder(&self) -> &FeatureHolder {
        &self.feature_holder
    }
}

impl ObjectHolder for NormalObjectHolder {}

// ---------------------------------------------------------------------------
// BiomeHolder
// ---------------------------------------------------------------------------

/// Biome noise holder (continentalness/temperature/humidity/erosion/weirdness/offset/jagged).
pub struct BiomeHolder {
    random: Xoroshiro128,
    continental_noise: Arc<NormalNoise>,
    temperature_noise: Arc<NormalNoise>,
    humidity_noise: Arc<NormalNoise>,
    erosion_noise: Arc<NormalNoise>,
    weirdness_noise: Arc<NormalNoise>,
    offset_noise: Arc<NormalNoise>,
    jagged_noise: Arc<NormalNoise>,
}

impl BiomeHolder {
    /// Builds the biome noises.
    ///
    /// Seven `fork()` calls advance the shared RNG in order.
    pub fn new(random: &mut Xoroshiro128) -> Self {
        let continental_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(
                &mut f,
                -9,
                &[1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0],
            ))
        };
        let temperature_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(
                &mut f,
                -10,
                &[1.5, 0.0, 1.0, 0.0, 0.0, 0.0],
            ))
        };
        let humidity_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(
                &mut f,
                -8,
                &[1.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            ))
        };
        let erosion_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -9, &[1.0, 1.0, 0.0, 1.0, 1.0]))
        };
        let weirdness_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(
                &mut f,
                -7,
                &[1.0, 2.0, 1.0, 0.0, 0.0, 0.0],
            ))
        };
        let offset_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -3, &[1.0, 1.0, 1.0, 0.0]))
        };
        let jagged_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -16, &[1.0; 16]))
        };
        Self {
            random: *random,
            continental_noise,
            temperature_noise,
            humidity_noise,
            erosion_noise,
            weirdness_noise,
            offset_noise,
            jagged_noise,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn continental_noise(&self) -> &Arc<NormalNoise> {
        &self.continental_noise
    }
    pub fn temperature_noise(&self) -> &Arc<NormalNoise> {
        &self.temperature_noise
    }
    pub fn humidity_noise(&self) -> &Arc<NormalNoise> {
        &self.humidity_noise
    }
    pub fn erosion_noise(&self) -> &Arc<NormalNoise> {
        &self.erosion_noise
    }
    pub fn weirdness_noise(&self) -> &Arc<NormalNoise> {
        &self.weirdness_noise
    }
    pub fn offset_noise(&self) -> &Arc<NormalNoise> {
        &self.offset_noise
    }
    pub fn jagged_noise(&self) -> &Arc<NormalNoise> {
        &self.jagged_noise
    }
}

// ---------------------------------------------------------------------------
// TerrainHolder
// ---------------------------------------------------------------------------

/// Terrain density-function holder (continents/erosion/ridges/caves/veins/materials/aquifer).
///
/// Reads biome noise through the explicitly passed `&BiomeHolder`.
pub struct TerrainHolder {
    random: Xoroshiro128,
    surface_noise: Arc<NormalNoise>,
    jagged: Arc<NormalNoise>,
    density_function: Arc<dyn DensityFunction>,
    continents: Arc<dyn DensityFunction>,
    erosion: Arc<dyn DensityFunction>,
    ridges: Arc<dyn DensityFunction>,
    ridges_folded: Arc<dyn DensityFunction>,
    offset: Arc<dyn DensityFunction>,
    depth: Arc<dyn DensityFunction>,
    factor: Arc<dyn DensityFunction>,
    jaggedness: Arc<dyn DensityFunction>,
    base3d: Arc<dyn DensityFunction>,
    sloped_cheese: Arc<dyn DensityFunction>,
    barrier_noise: Arc<NormalNoise>,
    fluid_level_floodedness_noise: Arc<NormalNoise>,
    fluid_level_spread_noise: Arc<NormalNoise>,
    lava_noise: Arc<NormalNoise>,
    pillar: Arc<NormalNoise>,
    pillar_rareness: Arc<NormalNoise>,
    pillar_thickness: Arc<NormalNoise>,
    spaghetti_2d: Arc<NormalNoise>,
    spaghetti_2d_elevation: Arc<NormalNoise>,
    spaghetti_2d_modulator: Arc<NormalNoise>,
    spaghetti_2d_thickness: Arc<NormalNoise>,
    spaghetti_3d_first: Arc<NormalNoise>,
    spaghetti_3d_second: Arc<NormalNoise>,
    spaghetti_3d_rarity: Arc<NormalNoise>,
    spaghetti_3d_thickness: Arc<NormalNoise>,
    spaghetti_roughness: Arc<NormalNoise>,
    spaghetti_roughness_modulator: Arc<NormalNoise>,
    cave_entrance: Arc<NormalNoise>,
    cave_layer: Arc<NormalNoise>,
    cave_cheese: Arc<NormalNoise>,
    noodle: Arc<NormalNoise>,
    noodle_thickness: Arc<NormalNoise>,
    noodle_ridge_a: Arc<NormalNoise>,
    noodle_ridge_b: Arc<NormalNoise>,
    vein_toggle_noise: Arc<NormalNoise>,
    vein_a_noise: Arc<NormalNoise>,
    vein_b_noise: Arc<NormalNoise>,
    ore_gap_noise: Arc<NormalNoise>,
    cave_detector: Arc<dyn DensityFunction>,
    vein_toggle: Arc<dyn DensityFunction>,
    vein_ridged: Arc<dyn DensityFunction>,
    vein_gap: Arc<dyn DensityFunction>,
    preliminary_surface_density: Arc<dyn DensityFunction>,
    preliminary_surface_upper_bound: Arc<dyn DensityFunction>,
    multi_material: MultiMaterial,
    /// Per-chunk aquifer state (single-threaded use).
    aquifer: Rc<RefCell<Option<Aquifer>>>,
    /// Stashed `MaterialBlocks` used when `begin_aquifer` builds the `Aquifer`.
    blocks: MaterialBlocks,
}

impl TerrainHolder {
    /// Builds terrain noises, density functions, veinifier, and materials.
    ///
    /// `biome_holder` supplies the biome noises.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        random: &mut Xoroshiro128,
        biome_holder: &BiomeHolder,
        blocks: MaterialBlocks,
    ) -> Self {
        // --- `identical()` draws (no RNG advance) ---
        let surface_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -6, &[1.0, 1.0, 1.0]))
        };
        let jagged = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -16, &[1.0; 16]))
        };

        // --- BiomeHolder noises (shared shift/continental/erosion/ridge/jagged inputs) ---
        let shift_noise = Arc::clone(biome_holder.offset_noise());
        let continentalness = Arc::clone(biome_holder.continental_noise());
        let erosion_noise = Arc::clone(biome_holder.erosion_noise());
        let ridge_noise = Arc::clone(biome_holder.weirdness_noise());
        let jagged_noise = Arc::clone(biome_holder.jagged_noise());

        // --- Density-function wiring (no RNG use) ---
        let continents = continents::overworld_continents_with_shift_noise(
            Arc::clone(&continentalness),
            Arc::clone(&shift_noise),
        );
        let erosion = erosion::overworld_erosion_with_shift_noise(
            Arc::clone(&erosion_noise),
            Arc::clone(&shift_noise),
        );
        let ridges = ridges::overworld_ridges_with_shift_noise(
            Arc::clone(&ridge_noise),
            Arc::clone(&shift_noise),
        );
        let ridges_folded = ridges_folded::overworld_ridges_folded(Arc::clone(&ridges));

        let offset = offset::overworld_offset(
            Arc::clone(&continents),
            Arc::clone(&erosion),
            Arc::clone(&ridges_folded),
        );
        let depth = depth::overworld_depth(Arc::clone(&offset));
        let factor = factor::overworld_factor(
            Arc::clone(&continents),
            Arc::clone(&erosion),
            Arc::clone(&ridges),
            Arc::clone(&ridges_folded),
        );
        let jaggedness = jaggedness::overworld_jaggedness(
            Arc::clone(&continents),
            Arc::clone(&erosion),
            Arc::clone(&ridges),
            Arc::clone(&ridges_folded),
        );

        // --- `identical()` for the 3D base (no RNG advance) ---
        let base3d = {
            let mut id = random.identical();
            base3d::overworld(&mut id)
        };

        // --- 28 ordered `fork()` draws (advance the RNG) ---
        let barrier_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -3, &[1.0, 1.0, 1.0]))
        };
        let fluid_level_floodedness_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[1.0, 1.0, 0.0, 1.0]))
        };
        let fluid_level_spread_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -5, &[1.0, 1.0, 1.0]))
        };
        let lava_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -1, &[1.0, 1.0]))
        };
        let pillar = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[1.0, 1.0]))
        };
        let pillar_rareness = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -8, &[1.0]))
        };
        let pillar_thickness = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -8, &[1.0]))
        };
        let spaghetti_2d = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[1.0]))
        };
        let spaghetti_2d_elevation = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -8, &[1.0]))
        };
        let spaghetti_2d_modulator = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -11, &[1.0]))
        };
        let spaghetti_2d_thickness = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -11, &[1.0]))
        };
        let spaghetti_3d_first = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[1.0]))
        };
        let spaghetti_3d_second = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[1.0]))
        };
        let spaghetti_3d_rarity = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -11, &[1.0]))
        };
        let spaghetti_3d_thickness = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -8, &[1.0]))
        };
        let spaghetti_roughness = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -5, &[1.0]))
        };
        let spaghetti_roughness_modulator = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -8, &[1.0]))
        };
        let cave_entrance = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[0.4, 0.5, 1.0]))
        };
        let cave_layer = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -8, &[1.0]))
        };
        let cave_cheese = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(
                &mut f,
                -8,
                &[0.5, 1.0, 2.0, 1.0, 2.0, 1.0, 0.0, 2.0, 0.0],
            ))
        };
        let noodle = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -8, &[1.0]))
        };
        let noodle_thickness = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -8, &[1.0]))
        };
        let noodle_ridge_a = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[1.0]))
        };
        let noodle_ridge_b = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[1.0]))
        };
        let vein_toggle_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -8, &[1.0]))
        };
        let vein_a_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[1.0]))
        };
        let vein_b_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -7, &[1.0]))
        };
        let ore_gap_noise = {
            let mut f = random.fork();
            Arc::new(NormalNoise::new(&mut f, -5, &[1.0]))
        };

        // --- Post-wiring density functions (no RNG) ---
        let sloped_cheese = sloped_cheese::overworld_sloped_cheese(
            Arc::clone(&depth),
            Arc::clone(&jaggedness),
            Arc::clone(&factor),
            Arc::clone(&base3d),
            Arc::clone(&jagged_noise),
        );
        let density_function = caves::final_density(
            Arc::clone(&sloped_cheese),
            Arc::clone(&spaghetti_roughness),
            Arc::clone(&spaghetti_roughness_modulator),
            Arc::clone(&spaghetti_2d_thickness),
            Arc::clone(&spaghetti_2d_modulator),
            Arc::clone(&spaghetti_2d),
            Arc::clone(&spaghetti_2d_elevation),
            Arc::clone(&spaghetti_3d_rarity),
            Arc::clone(&spaghetti_3d_thickness),
            Arc::clone(&spaghetti_3d_first),
            Arc::clone(&spaghetti_3d_second),
            Arc::clone(&cave_entrance),
            Arc::clone(&cave_layer),
            Arc::clone(&cave_cheese),
            Arc::clone(&pillar),
            Arc::clone(&pillar_rareness),
            Arc::clone(&pillar_thickness),
            Arc::clone(&noodle),
            Arc::clone(&noodle_thickness),
            Arc::clone(&noodle_ridge_a),
            Arc::clone(&noodle_ridge_b),
        );
        let preliminary_surface_density =
            caves::preliminary_surface_level(Arc::clone(&offset), Arc::clone(&factor));
        let preliminary_surface_upper_bound =
            caves::preliminary_surface_level_upper_bound(Arc::clone(&offset), Arc::clone(&factor));

        // --- `next_long()` draws the ore-vein seed (advances the RNG) ---
        let ore_vein_seed = random.next_long();

        // --- Ore-vein density functions (no RNG) ---
        let vein_toggle = ore_veins::overworld_vein_toggle(Arc::clone(&vein_toggle_noise));
        let vein_ridged =
            ore_veins::overworld_vein_ridged(Arc::clone(&vein_a_noise), Arc::clone(&vein_b_noise));
        let vein_gap = ore_veins::overworld_vein_gap(Arc::clone(&ore_gap_noise));

        let ore_veinifier = Arc::new(OreVeinifier::new(
            Arc::clone(&vein_toggle),
            Arc::clone(&vein_ridged),
            Arc::clone(&vein_gap),
            ore_vein_seed,
            blocks.clone(),
        ));

        // --- Material chain wiring ---
        let aquifer: Rc<RefCell<Option<Aquifer>>> = Rc::new(RefCell::new(None));
        let wrapped = cache_all_in_cell(Arc::clone(&density_function));

        let aquifer_filler = Arc::new(AquiferMaterialFiller {
            aquifer: Rc::clone(&aquifer),
            wrapped: Arc::clone(&wrapped),
        });
        let stone_filler = Arc::new(StoneMaterialFiller {
            wrapped,
            stone: blocks.stone,
        });

        let multi_material = MultiMaterial::new(vec![
            aquifer_filler as Arc<dyn MaterialFiller>,
            ore_veinifier as Arc<dyn MaterialFiller>,
            stone_filler as Arc<dyn MaterialFiller>,
        ]);

        // Clone before moving density_function into Self.
        let cave_detector = Arc::clone(&density_function);

        Self {
            random: *random,
            surface_noise,
            jagged,
            density_function,
            continents,
            erosion,
            ridges,
            ridges_folded,
            offset,
            depth,
            factor,
            jaggedness,
            base3d,
            sloped_cheese,
            barrier_noise,
            fluid_level_floodedness_noise,
            fluid_level_spread_noise,
            lava_noise,
            pillar,
            pillar_rareness,
            pillar_thickness,
            spaghetti_2d,
            spaghetti_2d_elevation,
            spaghetti_2d_modulator,
            spaghetti_2d_thickness,
            spaghetti_3d_first,
            spaghetti_3d_second,
            spaghetti_3d_rarity,
            spaghetti_3d_thickness,
            spaghetti_roughness,
            spaghetti_roughness_modulator,
            cave_entrance,
            cave_layer,
            cave_cheese,
            noodle,
            noodle_thickness,
            noodle_ridge_a,
            noodle_ridge_b,
            vein_toggle_noise,
            vein_a_noise,
            vein_b_noise,
            ore_gap_noise,
            cave_detector,
            vein_toggle,
            vein_ridged,
            vein_gap,
            preliminary_surface_density,
            preliminary_surface_upper_bound,
            multi_material,
            aquifer,
            blocks,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn surface_noise(&self) -> &Arc<NormalNoise> {
        &self.surface_noise
    }
    pub fn jagged(&self) -> &Arc<NormalNoise> {
        &self.jagged
    }
    pub fn density_function(&self) -> &Arc<dyn DensityFunction> {
        &self.density_function
    }
    /// Continent function (used by the overworld biome picker).
    pub fn continents(&self) -> &Arc<dyn DensityFunction> {
        &self.continents
    }
    /// Erosion function.
    pub fn erosion(&self) -> &Arc<dyn DensityFunction> {
        &self.erosion
    }
    /// Ridge function.
    pub fn ridges(&self) -> &Arc<dyn DensityFunction> {
        &self.ridges
    }
    pub fn sloped_cheese(&self) -> &Arc<dyn DensityFunction> {
        &self.sloped_cheese
    }
    pub fn preliminary_surface_density(&self) -> &Arc<dyn DensityFunction> {
        &self.preliminary_surface_density
    }
    pub fn preliminary_surface_upper_bound(&self) -> &Arc<dyn DensityFunction> {
        &self.preliminary_surface_upper_bound
    }
    pub fn multi_material(&self) -> &MultiMaterial {
        &self.multi_material
    }

    /// Starts per-chunk aquifer sampling.
    ///
    /// Takes plain scalar inputs (chunk position/level seed) instead of chunk/level types.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_aquifer(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        level_seed: i64,
        chunk_cache: Rc<crate::worldgen::densityfunction::function::ChunkCache>,
        min_y: i32,
        y_block_size: i32,
        sea_level: i32,
    ) {
        let global_fluid_picker: Arc<dyn FluidPicker> =
            Arc::new(aquifer::overworld_fluid_picker(sea_level, &self.blocks));
        let aquifer = Aquifer::new(
            chunk_x,
            chunk_z,
            level_seed,
            chunk_cache,
            noise_scaled_xy(Arc::clone(&self.barrier_noise), 1.0, 0.5),
            noise_scaled_xy(Arc::clone(&self.fluid_level_floodedness_noise), 1.0, 0.67),
            noise_scaled_xy(
                Arc::clone(&self.fluid_level_spread_noise),
                1.0,
                0.7142857142857143,
            ),
            noise_scaled_xy(Arc::clone(&self.lava_noise), 1.0, 1.0),
            Arc::clone(&self.erosion),
            Arc::clone(&self.depth),
            Arc::clone(&self.preliminary_surface_density),
            Arc::clone(&self.preliminary_surface_upper_bound),
            -64,
            8,
            min_y,
            y_block_size,
            global_fluid_picker,
            self.blocks.clone(),
        );
        *self.aquifer.borrow_mut() = Some(aquifer);
    }

    /// Ends per-chunk aquifer sampling.
    pub fn end_aquifer(&self) {
        *self.aquifer.borrow_mut() = None;
    }
}

// ---------------------------------------------------------------------------
// SurfaceHolder
// ---------------------------------------------------------------------------

/// Surface-noise holder for the surface-data stage.
pub struct SurfaceHolder {
    random: Xoroshiro128,
    noise: Arc<NormalNoise>,
}

impl SurfaceHolder {
    /// Builds the surface noise.
    ///
    /// Uses `identical()` (no RNG advance).
    pub fn new(random: &mut Xoroshiro128) -> Self {
        let noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -6, &[1.0, 1.0, 1.0]))
        };
        Self {
            random: *random,
            noise,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn noise(&self) -> &Arc<NormalNoise> {
        &self.noise
    }
}

// ---------------------------------------------------------------------------
// SurfaceOverwriteHolder
// ---------------------------------------------------------------------------

/// Surface-overwrite noise holder plus the lazily built clay-band cache.
pub struct SurfaceOverwriteHolder {
    random: Xoroshiro128,
    surface_noise: Arc<NormalNoise>,
    swamp_noise: Arc<NormalNoise>,
    clay_bands_offset_noise: Arc<NormalNoise>,
    badlands_pillar_noise: Arc<NormalNoise>,
    badlands_pillar_roof_noise: Arc<NormalNoise>,
    badlands_surface_noise: Arc<NormalNoise>,
    iceberg_pillar_noise: Arc<NormalNoise>,
    iceberg_pillar_roof_noise: Arc<NormalNoise>,
    iceberg_surface_noise: Arc<NormalNoise>,
    /// Clay-band cache, lazily built on first use.
    ///
    /// `RefCell` supports lazy init through a shared holder reference
    /// (single-threaded generation needs no locking).
    clay_bands_cache: RefCell<Vec<Option<BlockRuntimeId>>>,
}

impl SurfaceOverwriteHolder {
    /// Builds the overwrite noises.
    ///
    /// Nine `identical()` draws (no RNG advance).
    pub fn new(random: &mut Xoroshiro128) -> Self {
        let surface_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -6, &[1.0, 1.0, 1.0]))
        };
        let swamp_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -2, &[1.0]))
        };
        let clay_bands_offset_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -8, &[1.0]))
        };
        let badlands_pillar_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -2, &[1.0, 1.0, 1.0]))
        };
        let badlands_pillar_roof_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -8, &[1.0]))
        };
        let badlands_surface_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -6, &[1.0, 1.0, 1.0]))
        };
        let iceberg_pillar_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -6, &[1.0, 1.0, 1.0, 1.0]))
        };
        let iceberg_pillar_roof_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -3, &[1.0]))
        };
        let iceberg_surface_noise = {
            let mut id = random.identical();
            Arc::new(NormalNoise::new(&mut id, -6, &[1.0, 1.0, 1.0]))
        };
        Self {
            random: *random,
            surface_noise,
            swamp_noise,
            clay_bands_offset_noise,
            badlands_pillar_noise,
            badlands_pillar_roof_noise,
            badlands_surface_noise,
            iceberg_pillar_noise,
            iceberg_pillar_roof_noise,
            iceberg_surface_noise,
            clay_bands_cache: RefCell::new(vec![None; 192]),
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn surface_noise(&self) -> &Arc<NormalNoise> {
        &self.surface_noise
    }
    pub fn swamp_noise(&self) -> &Arc<NormalNoise> {
        &self.swamp_noise
    }
    pub fn clay_bands_offset_noise(&self) -> &Arc<NormalNoise> {
        &self.clay_bands_offset_noise
    }
    pub fn badlands_pillar_noise(&self) -> &Arc<NormalNoise> {
        &self.badlands_pillar_noise
    }
    pub fn badlands_pillar_roof_noise(&self) -> &Arc<NormalNoise> {
        &self.badlands_pillar_roof_noise
    }
    pub fn badlands_surface_noise(&self) -> &Arc<NormalNoise> {
        &self.badlands_surface_noise
    }
    pub fn iceberg_pillar_noise(&self) -> &Arc<NormalNoise> {
        &self.iceberg_pillar_noise
    }
    pub fn iceberg_pillar_roof_noise(&self) -> &Arc<NormalNoise> {
        &self.iceberg_pillar_roof_noise
    }
    pub fn iceberg_surface_noise(&self) -> &Arc<NormalNoise> {
        &self.iceberg_surface_noise
    }
    /// Returns the clay-band cache cell for lazy init by the caller.
    pub fn clay_bands_cache(&self) -> &RefCell<Vec<Option<BlockRuntimeId>>> {
        &self.clay_bands_cache
    }
}

// ---------------------------------------------------------------------------
// FeatureHolder
// ---------------------------------------------------------------------------

/// Feature-noise holder (clay/dripleaf, dripstone, moss, sculk, kelp, sulfur).
pub struct FeatureHolder {
    random: Xoroshiro128,
    random_clay_with_dripleaves_snap_to_floor: SimplexF,
    dripstone_cluster: SimplexF,
    moss_patch_snap_to_floor: SimplexF,
    moss_snap_to_ceiling: SimplexF,
    sculk_patch: SimplexF,
    kelp: SimplexNoise,
    sulfur_cave_gradient: SimplexNoise,
}

impl FeatureHolder {
    /// Builds the feature noises.
    ///
    /// Six `identical()` draws (no advance) plus one `fork()` (advances).
    pub fn new(random: &mut Xoroshiro128) -> Self {
        let random_clay_with_dripleaves_snap_to_floor = {
            let mut id = random.identical();
            SimplexF::new_with_expansion(&mut id, 1.0, 2.0 / 4.0, 1.0 / 15.0)
        };
        let dripstone_cluster = {
            let mut id = random.identical();
            SimplexF::new_with_expansion(&mut id, 30.0, 1.0 / 99.0, 1.0 / 15.0)
        };
        let moss_patch_snap_to_floor = {
            let mut id = random.identical();
            SimplexF::new_with_expansion(&mut id, 2.0, 2.0 / 4.0, 1.0 / 10.0)
        };
        let moss_snap_to_ceiling = {
            let mut id = random.identical();
            SimplexF::new_with_expansion(&mut id, 2.0, 2.0 / 4.0, 1.0 / 30.0)
        };
        let sculk_patch = {
            let mut id = random.identical();
            SimplexF::new_with_expansion(&mut id, 20.0, 1.0 / 99.0, 1.0 / 100.0)
        };
        let kelp = {
            let mut id = random.identical();
            SimplexNoise::new(&mut id, -7, &[1.0])
        };
        let sulfur_cave_gradient = {
            let mut f = random.fork();
            SimplexNoise::new(&mut f, -5, &[1.0, 0.0, 1.0])
        };
        Self {
            random: *random,
            random_clay_with_dripleaves_snap_to_floor,
            dripstone_cluster,
            moss_patch_snap_to_floor,
            moss_snap_to_ceiling,
            sculk_patch,
            kelp,
            sulfur_cave_gradient,
        }
    }

    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
    pub fn random_clay_with_dripleaves_snap_to_floor(&self) -> &SimplexF {
        &self.random_clay_with_dripleaves_snap_to_floor
    }
    pub fn dripstone_cluster(&self) -> &SimplexF {
        &self.dripstone_cluster
    }
    pub fn moss_patch_snap_to_floor(&self) -> &SimplexF {
        &self.moss_patch_snap_to_floor
    }
    pub fn moss_snap_to_ceiling(&self) -> &SimplexF {
        &self.moss_snap_to_ceiling
    }
    pub fn sculk_patch(&self) -> &SimplexF {
        &self.sculk_patch
    }
    pub fn kelp(&self) -> &SimplexNoise {
        &self.kelp
    }
    pub fn sulfur_cave_gradient(&self) -> &SimplexNoise {
        &self.sulfur_cave_gradient
    }
}
