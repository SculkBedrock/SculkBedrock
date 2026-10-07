//! Port of the `feature/decoration/` series.
//!
//! | Rust item | Upstream source |
//! |---|---|
//! | [`DecorationBlockTable`] | per-feature static `BlockState` constants plus tag checks |
//! | [`GroupedDiscFeature`] | `decoration/GroupedDiscFeature.java` |
//! | [`SurfaceGenerateFeature`] | `decoration/SurfaceGenerateFeature.java` |
//! | [`ScatterOverworldFlowerFeature`] | `decoration/ScatterOverworldFlowerFeature.java` |
//! | [`ScatterPlainsFlowerFeature`] | `decoration/ScatterPlainsFlowerFeature.java` |
//! | [`ScatterBrownMushroomFeature`] | `decoration/ScatterBrownMushroomFeature.java` |
//! | [`ScatterRedMushroomFeature`] | `decoration/ScatterRedMushroomFeature.java` |
//! | [`ScatterDryGrassFeature`] | `decoration/ScatterDryGrassFeature.java` |
//! | [`SwampFlowerFeature`] | `decoration/SwampFlowerFeature.java` |
//! | [`DeadBushFeature`] | `decoration/DeadBushFeature.java` |
//! | [`ScatterSweetBerryBushFeature`] | `decoration/ScatterSweetBerryBushFeature.java` |
//! | [`TallGrassPatchFeature`] | `decoration/TallGrassPatchFeature.java` |
//! | [`TallGrassGenerateFeature`] | `decoration/TallGrassGenerateFeature.java` |
//! | [`TallFernPatchFeature`] | `decoration/TallFernPatchFeature.java` |
//! | [`TaigaGrassFeature`] | `decoration/TaigaGrassFeature.java` |
//! | [`JungleGrassFeature`] | `decoration/JungleGrassFeature.java` |
//! | [`BushFeature`] | `decoration/BushFeature.java` |
//! | [`DesertCactusFeature`] | `decoration/DesertCactusFeature.java` |
//! | [`PumpkinGenerateFeature`] | `decoration/PumpkinGenerateFeature.java` |
//! | [`ReedsFeature`] | `decoration/ReedsFeature.java` |
//! | [`WaterlilyFeature`] | `decoration/WaterlilyFeature.java` |
//! | [`SunflowerDoublePlantPatchFeature`] | `decoration/SunflowerDouplePlantPatchFeature.java` |
//!
//! Port notes:
//! - Upstream biome gating comes from biome-definition `consolidatedFeatures`
//!   lists; this port filters per column against biome-id lists (sourced entry
//!   by entry from `.tools/gamedata/biome_features.json`).
//! - Upstream `GroupedDiscFeature` discs may spill across chunks (reading
//!   neighbors); single-chunk generation skips non-current columns (matching
//!   the upstream skip when the neighbor chunk is unloaded).
//! - `ScatterDryGrassFeature.getSourceBlock` seeds randomly by time upstream
//!   (nondeterministic); this port uses the feature random instead, with
//!   identical 1/3 tall-grass odds.
//! - Features writing chunks directly (cactus/waterlily/reeds) maintain the
//!   heightmap by hand (matching the heightmap update inside `setBlockState`).

use std::collections::HashSet;
use std::sync::Arc;

use sc_log::t_log;
use sc_world::block_dictionary::BlockStateDictionary;
use sc_world::chunk::BlockRuntimeId;

use crate::worldgen::biome::biome_id::*;
use crate::worldgen::chunk::WorldgenChunk;
use crate::worldgen::context::{BlockManager, ChunkGenerateContext};
use crate::worldgen::feature::object::{DIRT_TAG_MEMBERS, LIQUIDS, NON_SOLID_BLOCKS};
use crate::worldgen::feature::{CountGenerateFeature, GenerateFeature};
use crate::worldgen::math::{java_string_hashcode, random_range};
use crate::worldgen::random::RandomSourceProvider;
use crate::worldgen::stages::chunk_hash;
use crate::worldgen::stages::terrain::SEA_LEVEL;

// ---------------------------------------------------------------------------
// DecorationBlockTable (per-feature constants plus tag sets).
// ---------------------------------------------------------------------------

/// Official `minecraft:sand` tag members.
const SAND_TAG_MEMBERS: [&str; 3] = [
    "minecraft:sand",
    "minecraft:red_sand",
    "minecraft:suspicious_sand",
];

/// Block table plus predicate sets for surface decoration.
///
/// Predicate sets approximate the upstream property/tag system:
/// - `dirt`: `BlockTags.DIRT` members;
/// - `sand`:`BlockTags.SAND`;
/// - `non_solid`/`liquids`: solid/flowable approximations.
pub struct DecorationBlockTable {
    pub air: BlockRuntimeId,
    // Flowers (default states).
    pub dandelion: BlockRuntimeId,
    pub poppy: BlockRuntimeId,
    pub blue_orchid: BlockRuntimeId,
    pub azure_bluet: BlockRuntimeId,
    pub cornflower: BlockRuntimeId,
    pub oxeye_daisy: BlockRuntimeId,
    /// 9 plains-disc flowers (switch order 0-8).
    pub plains_flowers: [BlockRuntimeId; 9],
    // Grass/ferns.
    pub short_grass: BlockRuntimeId,
    pub tall_grass_lower: BlockRuntimeId,
    pub tall_grass_upper: BlockRuntimeId,
    pub fern: BlockRuntimeId,
    pub large_fern_lower: BlockRuntimeId,
    pub large_fern_upper: BlockRuntimeId,
    // Dry grass.
    pub short_dry_grass: BlockRuntimeId,
    pub tall_dry_grass: BlockRuntimeId,
    // Bushes/dead bushes/mushrooms.
    pub dead_bush: BlockRuntimeId,
    pub bush: BlockRuntimeId,
    pub brown_mushroom: BlockRuntimeId,
    pub red_mushroom: BlockRuntimeId,
    // Sweet berry bushes (growth=max).
    pub sweet_berry_bush: BlockRuntimeId,
    // Cacti.
    pub cactus: BlockRuntimeId,
    pub cactus_flower: BlockRuntimeId,
    // Pumpkins/reeds/lilies/sunflowers.
    pub pumpkin: BlockRuntimeId,
    pub reeds: BlockRuntimeId,
    pub waterlily: BlockRuntimeId,
    pub sunflower_lower: BlockRuntimeId,
    pub sunflower_upper: BlockRuntimeId,
    // Predicate sets.
    dirt: HashSet<BlockRuntimeId>,
    sand: HashSet<BlockRuntimeId>,
    grass_block: HashSet<BlockRuntimeId>,
    coarse_dirt: HashSet<BlockRuntimeId>,
    water: HashSet<BlockRuntimeId>,
    non_solid: HashSet<BlockRuntimeId>,
    liquids: HashSet<BlockRuntimeId>,
}

impl DecorationBlockTable {
    /// Build from the core palette (missing entries fall back to air plus warn).
    pub fn from_core_palette() -> Self {
        Self::build(None)
    }

    /// Build from a block-bundle snapshot (declared defaults win; multi-state
    /// exact queries still use the dictionary).
    pub fn from_block_snapshot(snapshot: &sc_block::block_json::BlockJsonSnapshot) -> Self {
        Self::build(Some(snapshot))
    }

    fn build(snapshot: Option<&sc_block::block_json::BlockJsonSnapshot>) -> Self {
        let dictionary = BlockStateDictionary::global();
        let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        let lookup = |name: &str| match crate::blocks_table::resolve_block_default(snapshot, name) {
            Some(hash) => BlockRuntimeId(hash),
            None => {
                log::warn!("{}", t_log!("console.worldgen.table_missing", table = "decoration", name = format!("{name:?}")));
                air
            }
        };
        let lookup_state = |name: &str, key: &str, value: &str| {
            // Exact states prefer the snapshot (always at hand here) over the
            // global dictionary (dlopened plugin copies may hold an empty one).
            if let Some(hash) = snapshot.and_then(|s| s.find_state_hash(name, &[(key, value)])) {
                return BlockRuntimeId(hash);
            }
            match dictionary.find_hash_by_state(name, key, value) {
                Some(hash) => BlockRuntimeId(hash),
                None => {
                    log::warn!("{}", t_log!("console.worldgen.table_state", table = "decoration", name = name, detail = format!("[{key}={value}]")));
                    lookup(name)
                }
            }
        };

        // Java: `populateFlower` switch(0-8).
        let plains_flowers = [
            lookup("minecraft:azure_bluet"),
            lookup("minecraft:cornflower"),
            lookup("minecraft:dandelion"),
            lookup("minecraft:oxeye_daisy"),
            lookup("minecraft:poppy"),
            lookup("minecraft:orange_tulip"),
            lookup("minecraft:pink_tulip"),
            lookup("minecraft:red_tulip"),
            lookup("minecraft:white_tulip"),
        ];
        // Two-high plants (upper_block_bit false/true).
        let tall_grass_lower = lookup_state("minecraft:tall_grass", "upper_block_bit", "false");
        let tall_grass_upper = lookup_state("minecraft:tall_grass", "upper_block_bit", "true");
        let large_fern_lower = lookup_state("minecraft:large_fern", "upper_block_bit", "false");
        let large_fern_upper = lookup_state("minecraft:large_fern", "upper_block_bit", "true");
        let sunflower_lower = lookup_state("minecraft:sunflower", "upper_block_bit", "false");
        let sunflower_upper = lookup_state("minecraft:sunflower", "upper_block_bit", "true");
        // Sweet berry bushes and reeds at max growth.
        let sweet_berry_bush = lookup_state("minecraft:sweet_berry_bush", "growth", "7");
        let reeds = lookup_state("minecraft:reeds", "age", "15");

        // Name lists expand to full state sets (snapshot first for the same
        // empty-dictionary reason as above).
        let states_of = |name: &str| -> HashSet<BlockRuntimeId> {
            if let Some(snap) = snapshot {
                let hashes = snap.state_hashes_of(name);
                if !hashes.is_empty() {
                    return hashes.into_iter().map(BlockRuntimeId).collect();
                }
            }
            let hashes: Vec<u32> = dictionary
                .entries_for(name)
                .into_iter()
                .map(|(hash, _)| hash)
                .collect();
            if hashes.is_empty() {
                let mut set = HashSet::new();
                set.insert(lookup(name));
                set
            } else {
                hashes.into_iter().map(BlockRuntimeId).collect()
            }
        };
        let states_of_many = |names: &[&str]| -> HashSet<BlockRuntimeId> {
            let mut set = HashSet::new();
            for name in names {
                set.extend(states_of(name));
            }
            set
        };

        let mut non_solid = states_of_many(&NON_SOLID_BLOCKS);
        non_solid.insert(air);
        let liquids = states_of_many(&LIQUIDS);

        Self {
            air,
            dandelion: lookup("minecraft:dandelion"),
            poppy: lookup("minecraft:poppy"),
            blue_orchid: lookup("minecraft:blue_orchid"),
            azure_bluet: lookup("minecraft:azure_bluet"),
            cornflower: lookup("minecraft:cornflower"),
            oxeye_daisy: lookup("minecraft:oxeye_daisy"),
            plains_flowers,
            short_grass: lookup("minecraft:short_grass"),
            tall_grass_lower,
            tall_grass_upper,
            fern: lookup("minecraft:fern"),
            large_fern_lower,
            large_fern_upper,
            short_dry_grass: lookup("minecraft:short_dry_grass"),
            tall_dry_grass: lookup("minecraft:tall_dry_grass"),
            dead_bush: lookup("minecraft:dead_bush"),
            bush: lookup("minecraft:bush"),
            brown_mushroom: lookup("minecraft:brown_mushroom"),
            red_mushroom: lookup("minecraft:red_mushroom"),
            sweet_berry_bush,
            cactus: lookup("minecraft:cactus"),
            cactus_flower: lookup("minecraft:cactus_flower"),
            pumpkin: lookup("minecraft:pumpkin"),
            reeds,
            waterlily: lookup("minecraft:waterlily"),
            sunflower_lower,
            sunflower_upper,
            dirt: states_of_many(&DIRT_TAG_MEMBERS),
            sand: states_of_many(&SAND_TAG_MEMBERS),
            grass_block: states_of("minecraft:grass_block"),
            coarse_dirt: states_of("minecraft:coarse_dirt"),
            water: states_of("minecraft:water"),
            non_solid,
            liquids,
        }
    }

    /// `block.hasTag(BlockTags.DIRT)`.
    pub fn is_dirt(&self, block: BlockRuntimeId) -> bool {
        self.dirt.contains(&block)
    }

    /// `block.hasTag(BlockTags.SAND)`.
    pub fn is_sand(&self, block: BlockRuntimeId) -> bool {
        self.sand.contains(&block)
    }

    /// Java: `GroupedDiscFeature.isSupportValid`(L127-129)——DIRT || SAND.
    pub fn is_dirt_or_sand(&self, block: BlockRuntimeId) -> bool {
        self.dirt.contains(&block) || self.sand.contains(&block)
    }

    /// Java: `Supportable.isSupportGrass`——`instanceof BlockGrassBlock`.
    pub fn is_grass_block(&self, block: BlockRuntimeId) -> bool {
        self.grass_block.contains(&block)
    }

    /// Java: `SurfaceGenerateFeature.isSupportValid`(L43-45)——
    /// `hasTag(DIRT) && !(instanceof BlockCoarseDirt)`.
    pub fn is_dirt_not_coarse(&self, block: BlockRuntimeId) -> bool {
        self.dirt.contains(&block) && !self.coarse_dirt.contains(&block)
    }

    /// Water id check (any liquid_depth).
    pub fn is_water(&self, block: BlockRuntimeId) -> bool {
        self.water.contains(&block)
    }

    /// `isSolid()` approximation (shared with the tree block table).
    pub fn is_solid(&self, block: BlockRuntimeId) -> bool {
        !self.non_solid.contains(&block)
    }

    /// Flowable approximation: non-solid set plus liquids
    /// (air/plants/liquids wash through).
    pub fn can_be_flowed_into(&self, block: BlockRuntimeId) -> bool {
        self.non_solid.contains(&block) || self.liquids.contains(&block)
    }
}

// ---------------------------------------------------------------------------
// SupportProbe (coordinate context for support checks).
// ---------------------------------------------------------------------------

/// Carries block id plus chunk and world coordinates,
/// equivalent to the positioned block object.
pub struct SupportProbe<'a> {
    pub chunk: &'a WorldgenChunk,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub block: BlockRuntimeId,
}

impl<'a> SupportProbe<'a> {
    /// Build from in-chunk coordinates.
    pub fn from_local(chunk: &'a WorldgenChunk, local_x: u8, y: i32, local_z: u8) -> Self {
        Self {
            chunk,
            x: (chunk.x() << 4) + local_x as i32,
            y,
            z: (chunk.z() << 4) + local_z as i32,
            block: chunk.block_state(local_x, y, local_z, 0),
        }
    }
}

// ---------------------------------------------------------------------------
// GroupedDiscFeature.
// ---------------------------------------------------------------------------

/// Java: `abstract class GroupedDiscFeature extends CountGenerateFeature`.
///
/// Each populate centers a disc of decoration on a random source column
/// (`getBase()+nextBoundedInt(getRandom())` calls).
pub trait GroupedDiscFeature: CountGenerateFeature {
    /// Java: `abstract BlockState getSourceBlock()`.
    fn source_block(&self, random: &mut dyn RandomSourceProvider) -> BlockRuntimeId;

    /// Java: `abstract int getMinRadius()`.
    fn get_min_radius(&self) -> i32;

    /// Java: `abstract int getMaxRadius()`.
    fn get_max_radius(&self) -> i32;

    /// Placement probability (default 1.0).
    fn get_probability(&self) -> f64 {
        1.0
    }

    /// Base count override (default 0).
    fn disc_base(&self) -> i32 {
        0
    }

    /// Random count override (default 0).
    fn disc_random(&self) -> i32 {
        0
    }

    /// Decoration block table.
    fn disc_table(&self) -> &DecorationBlockTable;

    /// Support check (default DIRT or SAND).
    fn disc_is_support_valid(&self, probe: &SupportProbe<'_>) -> bool {
        self.disc_table().is_dirt_or_sand(probe.block)
    }

    /// Column Y lookup (default heightmap).
    fn get_y(&self, chunk: &WorldgenChunk, x: u8, z: u8) -> i32 {
        chunk.height_map(x, z)
    }

    /// Biome gate (per-column list approximation).
    fn disc_can_spawn_here(&self, biome_id: i32) -> bool {
        let _ = biome_id;
        true
    }

    /// Java: `void populate(ChunkGenerateContext, RandomSourceProvider)`
    ///(L44-125).
    fn disc_populate(
        &self,
        ctx: &mut ChunkGenerateContext<'_>,
        random: &mut dyn RandomSourceProvider,
    ) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let random_x = random.next_int_max(15);
        let random_z = random.next_int_max(15);
        let height = self.get_y(ctx.chunk, random_x as u8, random_z as u8);
        let source_x = (chunk_x << 4) + random_x;
        let source_z = (chunk_z << 4) + random_z;
        let probability = self.get_probability();
        let always_place = probability >= 1.0;

        // Air required above the source column.
        if ctx
            .chunk
            .block_state(random_x as u8, height + 1, random_z as u8, 0)
            != self.disc_table().air
        {
            return;
        }
        // Biome gate (source column).
        if !self.disc_can_spawn_here(ctx.chunk.biome_id(random_x as u8, height, random_z as u8)) {
            return;
        }

        let chunk_ref: &WorldgenChunk = ctx.chunk;
        let mut object = BlockManager::with_chunk(chunk_ref);
        let source_block = self.source_block(random);
        // Radius rolls between min and max inclusive.
        let radius = random_range(random, self.get_min_radius(), self.get_max_radius());
        let radius_squared = radius * radius;
        let mut placed_any = false;

        for x in source_x - radius..=source_x + radius {
            let dx = x - source_x;
            let dx2 = dx * dx;
            for z in source_z - radius..=source_z + radius {
                let dz = z - source_z;
                // Disc membership check.
                if dx2 + dz * dz > radius_squared {
                    continue;
                }
                // Single-chunk approximation: other columns equal unloaded
                // neighbor chunks upstream (skipped, no randomness consumed).
                if (x >> 4) != chunk_x || (z >> 4) != chunk_z {
                    continue;
                }
                // Probability check (short-circuited when always placing).
                if !always_place && random.next_double() >= probability {
                    continue;
                }
                let local_x = (x & 0xF) as u8;
                let local_z = (z & 0xF) as u8;
                let support_y = self.get_y(chunk_ref, local_x, local_z);
                // Air required above the target column.
                if chunk_ref.block_state(local_x, support_y + 1, local_z, 0)
                    != self.disc_table().air
                {
                    continue;
                }
                let probe = SupportProbe::from_local(chunk_ref, local_x, support_y, local_z);
                if self.disc_is_support_valid(&probe) {
                    object.set_block_state_at(x, support_y + 1, z, 0, source_block);
                    placed_any = true;
                }
            }
        }

        // queueObject only when something placed.
        if placed_any {
            let places = object.into_places();
            ctx.queue_object(places);
        }
    }
}

// ---------------------------------------------------------------------------
// SurfaceGenerateFeature.
// ---------------------------------------------------------------------------

/// Java: `abstract class SurfaceGenerateFeature extends CountGenerateFeature`.
///
/// Random columns scan down from the heightmap to support, then decorate.
pub trait SurfaceGenerateFeature: CountGenerateFeature {
    /// Java: `abstract void place(BlockManager, int x, int y, int z)`.
    fn place(
        &self,
        manager: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        random: &mut dyn RandomSourceProvider,
    );

    /// Java: `abstract int getBase()`.
    fn surface_base(&self) -> i32;

    /// Java: `abstract int getRandom()`.
    fn surface_random(&self) -> i32;

    /// Decoration block table.
    fn surface_table(&self) -> &DecorationBlockTable;

    /// Support check (default DIRT that is not
    /// CoarseDirt).
    fn surface_is_support_valid(&self, block: BlockRuntimeId) -> bool {
        self.surface_table().is_dirt_not_coarse(block)
    }

    /// Biome gate (per-column list approximation).
    fn surface_can_spawn_here(&self, biome_id: i32) -> bool {
        let _ = biome_id;
        true
    }

    /// Java: `void populate(ChunkGenerateContext, RandomSourceProvider)`
    ///(L18-39).
    fn surface_populate(
        &self,
        ctx: &mut ChunkGenerateContext<'_>,
        random: &mut dyn RandomSourceProvider,
    ) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let x = random.next_bounded_int(15);
        let z = random.next_bounded_int(15);
        let mut y = ctx.chunk.height_map(x as u8, z as u8);
        let world_x = (chunk_x << 4) + x;
        let world_z = (chunk_z << 4) + z;

        // Biome gate (column).
        if !self.surface_can_spawn_here(ctx.chunk.biome_id(x as u8, y, z as u8)) {
            return;
        }

        // Scan down to valid support (y >= SEA_LEVEL - 1).
        while !self.surface_is_support_valid(ctx.chunk.block_state(x as u8, y, z as u8, 0))
            && y >= SEA_LEVEL - 1
        {
            y -= 1;
        }
        // Java L32-38.
        if y >= SEA_LEVEL - 1
            && self.surface_is_support_valid(ctx.chunk.block_state(x as u8, y, z as u8, 0))
        {
            let chunk_ref: &WorldgenChunk = ctx.chunk;
            let mut manager = BlockManager::with_chunk(chunk_ref);
            if manager.get_block_if_cached_or_loaded(world_x, y + 1, world_z)
                != self.surface_table().air
            {
                return;
            }
            let mut object = BlockManager::with_chunk(chunk_ref);
            self.place(&mut object, world_x, y + 1, world_z, random);
            let places = object.into_places();
            ctx.queue_object(places);
        }
    }
}

// ---------------------------------------------------------------------------
// Shared template macros.
// ---------------------------------------------------------------------------

/// `GroupedDiscFeature` subclass template: `apply` calls `count_apply`, 
/// `populate` calls `disc_populate`, base/random come from disc overrides.
macro_rules! impl_disc_feature {
    ($ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.count_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
        impl CountGenerateFeature for $ty {
            fn get_base(&self) -> i32 {
                self.disc_base()
            }
            fn get_random(&self) -> i32 {
                self.disc_random()
            }
            fn populate(
                &self,
                ctx: &mut ChunkGenerateContext<'_>,
                random: &mut dyn RandomSourceProvider,
            ) {
                self.disc_populate(ctx, random);
            }
        }
    };
}

/// `SurfaceGenerateFeature` subclass template. 
/// `populate` → `surface_populate`.
macro_rules! impl_surface_feature {
    ($ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.count_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
        impl CountGenerateFeature for $ty {
            fn get_base(&self) -> i32 {
                self.surface_base()
            }
            fn get_random(&self) -> i32 {
                self.surface_random()
            }
            fn populate(
                &self,
                ctx: &mut ChunkGenerateContext<'_>,
                random: &mut dyn RandomSourceProvider,
            ) {
                self.surface_populate(ctx, random);
            }
        }
    };
}

/// Template for `CountGenerateFeature` subclasses with custom populate.
macro_rules! impl_count_feature {
    ($ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.count_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
        impl CountGenerateFeature for $ty {
            fn get_base(&self) -> i32 {
                self.count_base()
            }
            fn get_random(&self) -> i32 {
                self.count_random()
            }
            fn populate(
                &self,
                ctx: &mut ChunkGenerateContext<'_>,
                random: &mut dyn RandomSourceProvider,
            ) {
                self.count_populate(ctx, random);
            }
        }
    };
}

/// Biome-id list check.
fn biome_in(biome_id: i32, list: &[i32]) -> bool {
    list.contains(&biome_id)
}

/// Direct chunk writes plus heightmap maintenance.
fn set_block_with_heightmap(
    chunk: &mut WorldgenChunk,
    x: u8,
    y: i32,
    z: u8,
    block: BlockRuntimeId,
) {
    chunk.set_block_state(x, y, z, 0, block);
    if block != BlockRuntimeId(sc_world::block_dictionary::air_runtime_id())
        && y > chunk.height_map(x, z)
    {
        chunk.set_height_map(x, z, y);
    }
}

// ---------------------------------------------------------------------------
// Biome-id lists (sourced from .tools/gamedata/biome_features.json).
// ---------------------------------------------------------------------------

/// Shared pumpkin/reeds list (78 biomes).
const PUMPKIN_REEDS_BIOMES: &[i32] = &[
    DEEP_OCEAN,
    OCEAN,
    FOREST_HILLS,
    FOREST,
    SUNFLOWER_PLAINS,
    PLAINS,
    DESERT_HILLS,
    DESERT_MUTATED,
    EXTREME_HILLS_PLUS_TREES,
    DESERT,
    EXTREME_HILLS_MUTATED,
    STONE_BEACH,
    EXTREME_HILLS_EDGE,
    EXTREME_HILLS,
    FLOWER_FOREST,
    TAIGA_HILLS,
    TAIGA_MUTATED,
    TAIGA,
    SWAMPLAND_MUTATED,
    SWAMPLAND,
    JUNGLE_EDGE,
    RIVER,
    LEGACY_FROZEN_OCEAN,
    COLD_BEACH,
    FROZEN_RIVER,
    ICE_MOUNTAINS,
    ICE_PLAINS_SPIKES,
    ICE_PLAINS,
    MUSHROOM_ISLAND_SHORE,
    MUSHROOM_ISLAND,
    BEACH,
    JUNGLE_HILLS,
    JUNGLE_MUTATED,
    JUNGLE,
    JUNGLE_EDGE_MUTATED,
    BIRCH_FOREST_HILLS,
    BIRCH_FOREST_MUTATED,
    BIRCH_FOREST,
    BIRCH_FOREST_HILLS_MUTATED,
    ROOFED_FOREST_MUTATED,
    ROOFED_FOREST,
    COLD_TAIGA_HILLS,
    COLD_TAIGA_MUTATED,
    COLD_TAIGA,
    MEGA_TAIGA_HILLS,
    REDWOOD_TAIGA_MUTATED,
    MEGA_TAIGA,
    REDWOOD_TAIGA_HILLS_MUTATED,
    EXTREME_HILLS_PLUS_TREES_MUTATED,
    SAVANNA_PLATEAU,
    SAVANNA_MUTATED,
    SAVANNA,
    SAVANNA_PLATEAU_MUTATED,
    MESA_BRYCE,
    MESA,
    MESA_PLATEAU_STONE_MUTATED,
    MESA_PLATEAU_STONE,
    MESA_PLATEAU_MUTATED,
    MESA_PLATEAU,
    WARM_OCEAN,
    DEEP_WARM_OCEAN,
    LUKEWARM_OCEAN,
    DEEP_LUKEWARM_OCEAN,
    COLD_OCEAN,
    DEEP_COLD_OCEAN,
    FROZEN_OCEAN,
    DEEP_FROZEN_OCEAN,
    BAMBOO_JUNGLE_HILLS,
    BAMBOO_JUNGLE,
    JAGGED_PEAKS,
    FROZEN_PEAKS,
    SNOWY_SLOPES,
    GROVE,
    MEADOW,
    LUSH_CAVES,
    DRIPSTONE_CAVES,
    STONY_PEAKS,
    DEEP_DARK,
    MANGROVE_SWAMP,
    CHERRY_GROVE,
    PALE_GARDEN,
    SULFUR_CAVES,
];

/// Overworld flower scatter list
/// (PUMPKIN_REEDS minus plains/flower/swamp/mushroom/mangrove/pale/badlands).
const OVERWORLD_FLOWER_BIOMES: &[i32] = &[
    DEEP_OCEAN,
    OCEAN,
    FOREST_HILLS,
    FOREST,
    DESERT_HILLS,
    DESERT_MUTATED,
    EXTREME_HILLS_PLUS_TREES,
    DESERT,
    EXTREME_HILLS_MUTATED,
    STONE_BEACH,
    EXTREME_HILLS_EDGE,
    EXTREME_HILLS,
    TAIGA_HILLS,
    TAIGA_MUTATED,
    TAIGA,
    JUNGLE_EDGE,
    RIVER,
    LEGACY_FROZEN_OCEAN,
    COLD_BEACH,
    FROZEN_RIVER,
    ICE_MOUNTAINS,
    ICE_PLAINS_SPIKES,
    ICE_PLAINS,
    BEACH,
    JUNGLE_HILLS,
    JUNGLE_MUTATED,
    JUNGLE,
    JUNGLE_EDGE_MUTATED,
    BIRCH_FOREST_HILLS,
    BIRCH_FOREST_MUTATED,
    BIRCH_FOREST,
    BIRCH_FOREST_HILLS_MUTATED,
    ROOFED_FOREST_MUTATED,
    ROOFED_FOREST,
    COLD_TAIGA_HILLS,
    COLD_TAIGA_MUTATED,
    COLD_TAIGA,
    MEGA_TAIGA_HILLS,
    REDWOOD_TAIGA_MUTATED,
    MEGA_TAIGA,
    REDWOOD_TAIGA_HILLS_MUTATED,
    EXTREME_HILLS_PLUS_TREES_MUTATED,
    SAVANNA_PLATEAU,
    SAVANNA_MUTATED,
    SAVANNA,
    SAVANNA_PLATEAU_MUTATED,
    WARM_OCEAN,
    DEEP_WARM_OCEAN,
    LUKEWARM_OCEAN,
    DEEP_LUKEWARM_OCEAN,
    COLD_OCEAN,
    DEEP_COLD_OCEAN,
    FROZEN_OCEAN,
    DEEP_FROZEN_OCEAN,
    BAMBOO_JUNGLE_HILLS,
    BAMBOO_JUNGLE,
    JAGGED_PEAKS,
    FROZEN_PEAKS,
    SNOWY_SLOPES,
    GROVE,
    MEADOW,
    LUSH_CAVES,
    DRIPSTONE_CAVES,
    STONY_PEAKS,
    DEEP_DARK,
    SULFUR_CAVES,
];

/// Tall grass scatter list
/// (PUMPKIN_REEDS minus jungle/taiga/mushroom/bamboo).
const TALL_GRASS_PATCH_BIOMES: &[i32] = &[
    DEEP_OCEAN,
    OCEAN,
    FOREST_HILLS,
    FOREST,
    SUNFLOWER_PLAINS,
    PLAINS,
    DESERT_HILLS,
    DESERT_MUTATED,
    EXTREME_HILLS_PLUS_TREES,
    DESERT,
    EXTREME_HILLS_MUTATED,
    STONE_BEACH,
    EXTREME_HILLS_EDGE,
    EXTREME_HILLS,
    FLOWER_FOREST,
    SWAMPLAND_MUTATED,
    SWAMPLAND,
    RIVER,
    LEGACY_FROZEN_OCEAN,
    COLD_BEACH,
    FROZEN_RIVER,
    ICE_MOUNTAINS,
    ICE_PLAINS_SPIKES,
    ICE_PLAINS,
    BEACH,
    BIRCH_FOREST_HILLS,
    BIRCH_FOREST_MUTATED,
    BIRCH_FOREST,
    BIRCH_FOREST_HILLS_MUTATED,
    ROOFED_FOREST_MUTATED,
    ROOFED_FOREST,
    EXTREME_HILLS_PLUS_TREES_MUTATED,
    SAVANNA_PLATEAU,
    SAVANNA_MUTATED,
    SAVANNA,
    SAVANNA_PLATEAU_MUTATED,
    MESA_BRYCE,
    MESA,
    MESA_PLATEAU_STONE_MUTATED,
    MESA_PLATEAU_STONE,
    MESA_PLATEAU_MUTATED,
    MESA_PLATEAU,
    WARM_OCEAN,
    DEEP_WARM_OCEAN,
    LUKEWARM_OCEAN,
    DEEP_LUKEWARM_OCEAN,
    COLD_OCEAN,
    DEEP_COLD_OCEAN,
    FROZEN_OCEAN,
    DEEP_FROZEN_OCEAN,
    JAGGED_PEAKS,
    FROZEN_PEAKS,
    SNOWY_SLOPES,
    GROVE,
    MEADOW,
    LUSH_CAVES,
    DRIPSTONE_CAVES,
    STONY_PEAKS,
    DEEP_DARK,
    MANGROVE_SWAMP,
    CHERRY_GROVE,
    PALE_GARDEN,
    SULFUR_CAVES,
];

/// Brown/red mushroom scatter list
/// (PUMPKIN_REEDS minus pale garden).
const MUSHROOM_BIOMES: &[i32] = &[
    DEEP_OCEAN,
    OCEAN,
    FOREST_HILLS,
    FOREST,
    SUNFLOWER_PLAINS,
    PLAINS,
    DESERT_HILLS,
    DESERT_MUTATED,
    EXTREME_HILLS_PLUS_TREES,
    DESERT,
    EXTREME_HILLS_MUTATED,
    STONE_BEACH,
    EXTREME_HILLS_EDGE,
    EXTREME_HILLS,
    FLOWER_FOREST,
    TAIGA_HILLS,
    TAIGA_MUTATED,
    TAIGA,
    SWAMPLAND_MUTATED,
    SWAMPLAND,
    JUNGLE_EDGE,
    RIVER,
    LEGACY_FROZEN_OCEAN,
    COLD_BEACH,
    FROZEN_RIVER,
    ICE_MOUNTAINS,
    ICE_PLAINS_SPIKES,
    ICE_PLAINS,
    MUSHROOM_ISLAND_SHORE,
    MUSHROOM_ISLAND,
    BEACH,
    JUNGLE_HILLS,
    JUNGLE_MUTATED,
    JUNGLE,
    JUNGLE_EDGE_MUTATED,
    BIRCH_FOREST_HILLS,
    BIRCH_FOREST_MUTATED,
    BIRCH_FOREST,
    BIRCH_FOREST_HILLS_MUTATED,
    ROOFED_FOREST_MUTATED,
    ROOFED_FOREST,
    COLD_TAIGA_HILLS,
    COLD_TAIGA_MUTATED,
    COLD_TAIGA,
    MEGA_TAIGA_HILLS,
    REDWOOD_TAIGA_MUTATED,
    MEGA_TAIGA,
    REDWOOD_TAIGA_HILLS_MUTATED,
    EXTREME_HILLS_PLUS_TREES_MUTATED,
    SAVANNA_PLATEAU,
    SAVANNA_MUTATED,
    SAVANNA,
    SAVANNA_PLATEAU_MUTATED,
    MESA_BRYCE,
    MESA,
    MESA_PLATEAU_STONE_MUTATED,
    MESA_PLATEAU_STONE,
    MESA_PLATEAU_MUTATED,
    MESA_PLATEAU,
    WARM_OCEAN,
    DEEP_WARM_OCEAN,
    LUKEWARM_OCEAN,
    DEEP_LUKEWARM_OCEAN,
    COLD_OCEAN,
    DEEP_COLD_OCEAN,
    FROZEN_OCEAN,
    DEEP_FROZEN_OCEAN,
    BAMBOO_JUNGLE_HILLS,
    BAMBOO_JUNGLE,
    JAGGED_PEAKS,
    FROZEN_PEAKS,
    SNOWY_SLOPES,
    GROVE,
    MEADOW,
    LUSH_CAVES,
    DRIPSTONE_CAVES,
    STONY_PEAKS,
    DEEP_DARK,
    MANGROVE_SWAMP,
    CHERRY_GROVE,
    SULFUR_CAVES,
];

/// Dry grass scatter list (desert plus badlands).
const DRY_GRASS_BIOMES: &[i32] = &[
    DESERT_HILLS,
    DESERT_MUTATED,
    DESERT,
    MESA_BRYCE,
    MESA,
    MESA_PLATEAU_STONE_MUTATED,
    MESA_PLATEAU_STONE,
    MESA_PLATEAU_MUTATED,
    MESA_PLATEAU,
];

/// Swamp flower scatter list.
const SWAMP_FLOWER_BIOMES: &[i32] = &[SWAMPLAND_MUTATED, SWAMPLAND];

/// Dead bush list.
const DEAD_BUSH_BIOMES: &[i32] = &[
    DESERT_HILLS,
    DESERT_MUTATED,
    DESERT,
    SWAMPLAND_MUTATED,
    SWAMPLAND,
    MEGA_TAIGA_HILLS,
    REDWOOD_TAIGA_MUTATED,
    MEGA_TAIGA,
    REDWOOD_TAIGA_HILLS_MUTATED,
    MESA_BRYCE,
    MESA,
    MESA_PLATEAU_STONE_MUTATED,
    MESA_PLATEAU_STONE,
    MESA_PLATEAU_MUTATED,
    MESA_PLATEAU,
    MANGROVE_SWAMP,
];

/// Taiga lists (`scatter_sweet_berry_bush_feature` /
/// `fern_double_plant_patch_feature` / `taiga_tall_grass_feature`).
const TAIGA_FAMILY_BIOMES: &[i32] = &[
    TAIGA_HILLS,
    TAIGA_MUTATED,
    TAIGA,
    COLD_TAIGA_HILLS,
    COLD_TAIGA_MUTATED,
    COLD_TAIGA,
    MEGA_TAIGA_HILLS,
    REDWOOD_TAIGA_MUTATED,
    MEGA_TAIGA,
    REDWOOD_TAIGA_HILLS_MUTATED,
];

/// `grass_double_plant_patch_feature`).
const GRASS_DOUBLE_PLANT_BIOMES: &[i32] = &[
    SUNFLOWER_PLAINS,
    PLAINS,
    SAVANNA_PLATEAU,
    SAVANNA,
    MEADOW,
    CHERRY_GROVE,
];

/// Jungle tall grass list (bamboo).
const JUNGLE_GRASS_BIOMES: &[i32] = &[BAMBOO_JUNGLE_HILLS, BAMBOO_JUNGLE];

/// Bush scatter list.
const BUSH_BIOMES: &[i32] = &[
    FOREST_HILLS,
    FOREST,
    SUNFLOWER_PLAINS,
    PLAINS,
    EXTREME_HILLS_PLUS_TREES,
    EXTREME_HILLS_MUTATED,
    EXTREME_HILLS_EDGE,
    EXTREME_HILLS,
    RIVER,
    FROZEN_RIVER,
    BIRCH_FOREST_HILLS,
    BIRCH_FOREST_MUTATED,
    BIRCH_FOREST,
    BIRCH_FOREST_HILLS_MUTATED,
    EXTREME_HILLS_PLUS_TREES_MUTATED,
];

/// Post-surface cactus rules list.
const CACTUS_BIOMES: &[i32] = &[DESERT_HILLS, DESERT_MUTATED, DESERT];

/// Waterlily fixup list.
const WATERLILY_BIOMES: &[i32] = &[SWAMPLAND_MUTATED, SWAMPLAND, MANGROVE_SWAMP];

/// Sunflower patch list.
const SUNFLOWER_BIOMES: &[i32] = &[SUNFLOWER_PLAINS];

// ---------------------------------------------------------------------------
// GroupedDiscFeature subclasses.
// ---------------------------------------------------------------------------

/// Red mushroom scatter (probability 0.1; custom support plus
/// downward getY scan; mushrooms need shade or mushroom islands).
pub struct ScatterRedMushroomFeature {
    table: Arc<DecorationBlockTable>,
}

impl ScatterRedMushroomFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_disc_feature!(
    ScatterRedMushroomFeature,
    "minecraft:scatter_red_mushroom_feature"
);

impl GroupedDiscFeature for ScatterRedMushroomFeature {
    fn source_block(&self, _random: &mut dyn RandomSourceProvider) -> BlockRuntimeId {
        self.table.red_mushroom
    }

    fn get_min_radius(&self) -> i32 {
        1
    }

    fn get_max_radius(&self) -> i32 {
        2
    }

    /// Probability widens from float to double.
    fn get_probability(&self) -> f64 {
        0.1f32 as f64
    }

    fn disc_base(&self) -> i32 {
        -7
    }

    fn disc_random(&self) -> i32 {
        8
    }

    fn disc_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn disc_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, MUSHROOM_BIOMES)
    }

    /// Java L52-56: `block.isSolid() && (heightMap != y || biome == MUSHROOM_ISLAND)`.
    fn disc_is_support_valid(&self, probe: &SupportProbe<'_>) -> bool {
        self.table.is_solid(probe.block)
            && (probe
                .chunk
                .height_map((probe.x & 0xF) as u8, (probe.z & 0xF) as u8)
                != probe.y
                || probe
                    .chunk
                    .biome_id((probe.x & 0xF) as u8, probe.y, (probe.z & 0xF) as u8)
                    == MUSHROOM_ISLAND)
    }

    /// Scan down for the first y with air above and valid support.
    fn get_y(&self, chunk: &WorldgenChunk, x: u8, z: u8) -> i32 {
        let start_y = chunk.height_map(x, z);
        let mut y = start_y;
        while y > chunk.min_y() {
            let above_air = chunk.block_state(x, y + 1, z, 0) == self.table.air;
            if above_air {
                let probe = SupportProbe::from_local(chunk, x, y, z);
                if self.disc_is_support_valid(&probe) {
                    return y;
                }
            }
            y -= 1;
        }
        start_y
    }
}

/// Java: `ScatterBrownMushroomFeature`(extends ScatterRedMushroomFeature,
/// Only blocks/counts change; support/getY overrides inline here.
pub struct ScatterBrownMushroomFeature {
    table: Arc<DecorationBlockTable>,
}

impl ScatterBrownMushroomFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_disc_feature!(
    ScatterBrownMushroomFeature,
    "minecraft:scatter_brown_mushroom_feature"
);

impl GroupedDiscFeature for ScatterBrownMushroomFeature {
    fn source_block(&self, _random: &mut dyn RandomSourceProvider) -> BlockRuntimeId {
        self.table.brown_mushroom
    }

    fn get_min_radius(&self) -> i32 {
        1
    }

    fn get_max_radius(&self) -> i32 {
        2
    }

    fn get_probability(&self) -> f64 {
        0.1f32 as f64
    }

    fn disc_base(&self) -> i32 {
        -3
    }

    fn disc_random(&self) -> i32 {
        4
    }

    fn disc_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn disc_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, MUSHROOM_BIOMES)
    }

    fn disc_is_support_valid(&self, probe: &SupportProbe<'_>) -> bool {
        self.table.is_solid(probe.block)
            && (probe
                .chunk
                .height_map((probe.x & 0xF) as u8, (probe.z & 0xF) as u8)
                != probe.y
                || probe
                    .chunk
                    .biome_id((probe.x & 0xF) as u8, probe.y, (probe.z & 0xF) as u8)
                    == MUSHROOM_ISLAND)
    }

    fn get_y(&self, chunk: &WorldgenChunk, x: u8, z: u8) -> i32 {
        let start_y = chunk.height_map(x, z);
        let mut y = start_y;
        while y > chunk.min_y() {
            let above_air = chunk.block_state(x, y + 1, z, 0) == self.table.air;
            if above_air {
                let probe = SupportProbe::from_local(chunk, x, y, z);
                if self.disc_is_support_valid(&probe) {
                    return y;
                }
            }
            y -= 1;
        }
        start_y
    }
}

/// Dry grass scatter (radius 3-4; probability 0.4; 1/3 tall).
pub struct ScatterDryGrassFeature {
    table: Arc<DecorationBlockTable>,
}

impl ScatterDryGrassFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_disc_feature!(
    ScatterDryGrassFeature,
    "minecraft:scatter_dry_grass_feature"
);

impl GroupedDiscFeature for ScatterDryGrassFeature {
    /// Dry grass picks by thirds (time-seeded upstream; feature random here
    /// with identical odds).
    fn source_block(&self, random: &mut dyn RandomSourceProvider) -> BlockRuntimeId {
        if random.next_int_max(3) == 0 {
            self.table.tall_dry_grass
        } else {
            self.table.short_dry_grass
        }
    }

    fn get_min_radius(&self) -> i32 {
        3
    }

    fn get_max_radius(&self) -> i32 {
        4
    }

    fn get_probability(&self) -> f64 {
        0.4f32 as f64
    }

    fn disc_base(&self) -> i32 {
        -10
    }

    fn disc_random(&self) -> i32 {
        12
    }

    fn disc_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn disc_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, DRY_GRASS_BIOMES)
    }
}

/// Swamp flowers (blue orchid discs, radius 1-2, probability 0.7).
pub struct SwampFlowerFeature {
    table: Arc<DecorationBlockTable>,
}

impl SwampFlowerFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_disc_feature!(SwampFlowerFeature, "minecraft:scatter_swamp_flower_feature");

impl GroupedDiscFeature for SwampFlowerFeature {
    fn source_block(&self, _random: &mut dyn RandomSourceProvider) -> BlockRuntimeId {
        self.table.blue_orchid
    }

    fn get_min_radius(&self) -> i32 {
        1
    }

    fn get_max_radius(&self) -> i32 {
        2
    }

    /// Java L34-36: `return 0.7f`.
    fn get_probability(&self) -> f64 {
        0.7f32 as f64
    }

    fn disc_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn disc_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, SWAMP_FLOWER_BIOMES)
    }
}

/// Dead bushes (radius 3-4, probability 0.2).
pub struct DeadBushFeature {
    table: Arc<DecorationBlockTable>,
}

impl DeadBushFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_disc_feature!(DeadBushFeature, "minecraft:dead_bush_feature");

impl GroupedDiscFeature for DeadBushFeature {
    fn source_block(&self, _random: &mut dyn RandomSourceProvider) -> BlockRuntimeId {
        self.table.dead_bush
    }

    fn get_min_radius(&self) -> i32 {
        3
    }

    fn get_max_radius(&self) -> i32 {
        4
    }

    fn get_probability(&self) -> f64 {
        0.2f32 as f64
    }

    fn disc_base(&self) -> i32 {
        -10
    }

    fn disc_random(&self) -> i32 {
        12
    }

    fn disc_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn disc_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, DEAD_BUSH_BIOMES)
    }
}

// ---------------------------------------------------------------------------
// SurfaceGenerateFeature subclasses.
// ---------------------------------------------------------------------------

/// Sweet berry bushes at max growth.
pub struct ScatterSweetBerryBushFeature {
    table: Arc<DecorationBlockTable>,
}

impl ScatterSweetBerryBushFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_surface_feature!(
    ScatterSweetBerryBushFeature,
    "minecraft:scatter_sweet_berry_bush_feature"
);

impl SurfaceGenerateFeature for ScatterSweetBerryBushFeature {
    fn place(
        &self,
        manager: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        _random: &mut dyn RandomSourceProvider,
    ) {
        manager.set_block_state_at(x, y, z, 0, self.table.sweet_berry_bush);
    }

    fn surface_base(&self) -> i32 {
        -66
    }

    fn surface_random(&self) -> i32 {
        70
    }

    fn surface_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn surface_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, TAIGA_FAMILY_BIOMES)
    }
}

/// Tall grass two-high plants.
pub struct TallGrassGenerateFeature {
    table: Arc<DecorationBlockTable>,
}

impl TallGrassGenerateFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_surface_feature!(
    TallGrassGenerateFeature,
    "minecraft:grass_double_plant_patch_feature"
);

impl SurfaceGenerateFeature for TallGrassGenerateFeature {
    fn place(
        &self,
        manager: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        _random: &mut dyn RandomSourceProvider,
    ) {
        // Two-high plants need air above.
        if manager.get_block_if_cached_or_loaded(x, y + 1, z) == self.table.air {
            manager.set_block_state_at(x, y, z, 0, self.table.tall_grass_lower);
            manager.set_block_state_at(x, y + 1, z, 0, self.table.tall_grass_upper);
        }
    }

    fn surface_base(&self) -> i32 {
        5
    }

    fn surface_random(&self) -> i32 {
        0
    }

    fn surface_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn surface_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, GRASS_DOUBLE_PLANT_BIOMES)
    }
}

/// Tall fern two-high plants.
pub struct TallFernPatchFeature {
    table: Arc<DecorationBlockTable>,
}

impl TallFernPatchFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_surface_feature!(
    TallFernPatchFeature,
    "minecraft:fern_double_plant_patch_feature"
);

impl SurfaceGenerateFeature for TallFernPatchFeature {
    fn place(
        &self,
        manager: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        _random: &mut dyn RandomSourceProvider,
    ) {
        if manager.get_block_if_cached_or_loaded(x, y + 1, z) == self.table.air {
            manager.set_block_state_at(x, y, z, 0, self.table.large_fern_lower);
            manager.set_block_state_at(x, y + 1, z, 0, self.table.large_fern_upper);
        }
    }

    fn surface_base(&self) -> i32 {
        5
    }

    fn surface_random(&self) -> i32 {
        0
    }

    fn surface_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn surface_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, TAIGA_FAMILY_BIOMES)
    }
}

/// Taiga grass (position-seeded: 1/7 short grass, else fern).
pub struct TaigaGrassFeature {
    table: Arc<DecorationBlockTable>,
}

impl TaigaGrassFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_surface_feature!(TaigaGrassFeature, "minecraft:taiga_tall_grass_feature");

impl SurfaceGenerateFeature for TaigaGrassFeature {
    /// Java L16-18: `random.setSeed(x + y + z).nextInt(7) == 0 ? SHORT_GRASS : FERN`
    /// (Position reseeding reuses the shared random.)
    fn place(
        &self,
        manager: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        random: &mut dyn RandomSourceProvider,
    ) {
        random.set_seed((x + y + z) as i64);
        let block = if random.next_int_max(7) == 0 {
            self.table.short_grass
        } else {
            self.table.fern
        };
        manager.set_block_state_at(x, y, z, 0, block);
    }

    fn surface_base(&self) -> i32 {
        8
    }

    fn surface_random(&self) -> i32 {
        0
    }

    fn surface_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    /// No biome references this feature directly (only via rules ids);
    /// it stays taiga-scoped by semantics.
    fn surface_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, TAIGA_FAMILY_BIOMES)
    }
}

/// Jungle grass (1/7 fern, else short grass; count 100+10).
pub struct JungleGrassFeature {
    table: Arc<DecorationBlockTable>,
}

impl JungleGrassFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_surface_feature!(JungleGrassFeature, "minecraft:jungle_tall_grass_feature");

impl SurfaceGenerateFeature for JungleGrassFeature {
    /// Java L16-18: `random.nextInt(7) == 0 ? FERN : SHORT_GRASS`.
    fn place(
        &self,
        manager: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        random: &mut dyn RandomSourceProvider,
    ) {
        let block = if random.next_int_max(7) == 0 {
            self.table.fern
        } else {
            self.table.short_grass
        };
        manager.set_block_state_at(x, y, z, 0, block);
    }

    fn surface_base(&self) -> i32 {
        100
    }

    fn surface_random(&self) -> i32 {
        10
    }

    fn surface_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn surface_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, JUNGLE_GRASS_BIOMES)
    }
}

/// Bushes.
pub struct BushFeature {
    table: Arc<DecorationBlockTable>,
}

impl BushFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_surface_feature!(BushFeature, "minecraft:scatter_bush_feature");

impl SurfaceGenerateFeature for BushFeature {
    fn place(
        &self,
        manager: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        _random: &mut dyn RandomSourceProvider,
    ) {
        manager.set_block_state_at(x, y, z, 0, self.table.bush);
    }

    fn surface_base(&self) -> i32 {
        8
    }

    fn surface_random(&self) -> i32 {
        0
    }

    fn surface_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn surface_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, BUSH_BIOMES)
    }
}

/// Sparse pumpkins (-2000 + 2015).
pub struct PumpkinGenerateFeature {
    table: Arc<DecorationBlockTable>,
}

impl PumpkinGenerateFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }
}

impl_surface_feature!(PumpkinGenerateFeature, "minecraft:pumpkin_feature");

impl SurfaceGenerateFeature for PumpkinGenerateFeature {
    fn place(
        &self,
        manager: &mut BlockManager<'_>,
        x: i32,
        y: i32,
        z: i32,
        _random: &mut dyn RandomSourceProvider,
    ) {
        manager.set_block_state_at(x, y, z, 0, self.table.pumpkin);
    }

    fn surface_base(&self) -> i32 {
        -2000
    }

    fn surface_random(&self) -> i32 {
        2015
    }

    fn surface_table(&self) -> &DecorationBlockTable {
        &self.table
    }

    fn surface_can_spawn_here(&self, biome_id: i32) -> bool {
        biome_in(biome_id, PUMPKIN_REEDS_BIOMES)
    }
}

// ---------------------------------------------------------------------------
// CountGenerateFeature subclasses with custom populate.
// ---------------------------------------------------------------------------

/// Overworld flower scatter (dandelion/poppy discs, 0.2 probability).
pub struct ScatterOverworldFlowerFeature {
    table: Arc<DecorationBlockTable>,
}

impl ScatterOverworldFlowerFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }

    fn count_base(&self) -> i32 {
        -7
    }

    fn count_random(&self) -> i32 {
        8
    }

    /// Java L26-66.
    fn count_populate(
        &self,
        ctx: &mut ChunkGenerateContext<'_>,
        random: &mut dyn RandomSourceProvider,
    ) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let random_x = random.next_int_max(15);
        let random_z = random.next_int_max(15);
        let source_x = (chunk_x << 4) + random_x;
        let source_z = (chunk_z << 4) + random_z;

        // Biome gate (source column).
        let height_at = ctx.chunk.height_map(random_x as u8, random_z as u8);
        if !biome_in(
            ctx.chunk
                .biome_id(random_x as u8, height_at, random_z as u8),
            OVERWORLD_FLOWER_BIOMES,
        ) {
            return;
        }

        let chunk_ref: &WorldgenChunk = ctx.chunk;
        let mut object = BlockManager::with_chunk(chunk_ref);
        let radius = random_range(random, 2, 3);
        let radius_squared = radius * radius;

        // Flower chosen by nextBoolean.
        let state = if random.next_boolean() {
            self.table.dandelion
        } else {
            self.table.poppy
        };

        for x in source_x - radius..=source_x + radius {
            for z in source_z - radius..=source_z + radius {
                // isChunkGenerated (single-chunk: current chunk only).
                if (x >> 4) != chunk_x || (z >> 4) != chunk_z {
                    continue;
                }
                let dx = x - source_x;
                let dz = z - source_z;
                // Disc plus skip on nextFloat >= 0.2.
                if dx * dx + dz * dz > radius_squared || random.next_float() >= 0.2f32 {
                    continue;
                }
                let local_x = (x & 0xF) as u8;
                let local_z = (z & 0xF) as u8;
                let y = chunk_ref.height_map(local_x, local_z);
                // Air required above.
                if chunk_ref.block_state(local_x, y + 1, local_z, 0) != self.table.air {
                    continue;
                }
                // Java L58-61: isSupportDirt.
                let support = chunk_ref.block_state(local_x, y, local_z, 0);
                if self.table.is_dirt(support) {
                    object.set_block_state_at(x, y + 1, z, 0, state);
                }
            }
        }

        // queueObject runs unconditionally.
        let places = object.into_places();
        ctx.queue_object(places);
    }
}

impl_count_feature!(
    ScatterOverworldFlowerFeature,
    "minecraft:scatter_overworld_flower_feature"
);

/// Plains flower scatter (9 flowers, 0.1 probability, grass support).
pub struct ScatterPlainsFlowerFeature {
    table: Arc<DecorationBlockTable>,
}

impl ScatterPlainsFlowerFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }

    fn count_base(&self) -> i32 {
        -7
    }

    fn count_random(&self) -> i32 {
        8
    }

    /// Java L17-45.
    fn count_populate(
        &self,
        ctx: &mut ChunkGenerateContext<'_>,
        random: &mut dyn RandomSourceProvider,
    ) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let random_x = random.next_int_max(15);
        let random_z = random.next_int_max(15);
        let source_x = (chunk_x << 4) + random_x;
        let source_z = (chunk_z << 4) + random_z;

        // Biome gate (source column).
        let height_at = ctx.chunk.height_map(random_x as u8, random_z as u8);
        if !biome_in(
            ctx.chunk
                .biome_id(random_x as u8, height_at, random_z as u8),
            &[PLAINS, SUNFLOWER_PLAINS],
        ) {
            return;
        }

        let chunk_ref: &WorldgenChunk = ctx.chunk;
        let mut object = BlockManager::with_chunk(chunk_ref);
        let radius = random_range(random, 2, 3);

        for x in source_x - radius..=source_x + radius {
            for z in source_z - radius..=source_z + radius {
                let dx = x - source_x;
                let dz = z - source_z;
                if dx * dx + dz * dz > radius * radius {
                    continue;
                }
                // Java L33: nextFloat() < 0.1f.
                if random.next_float() < 0.1f32 {
                    let local_x = (x & 0xF) as u8;
                    let local_z = (z & 0xF) as u8;
                    let height = chunk_ref.height_map(local_x, local_z);
                    // Grass tops get flowers (no above-air check, matching upstream).
                    let top = chunk_ref.block_state(local_x, height, local_z, 0);
                    if self.table.is_grass_block(top) {
                        // Java L37: populateFlower(nextBoundedInt(8), ...)
                        let flower = random.next_bounded_int(8);
                        let state = self.table.plains_flowers[(flower as usize).min(8)];
                        object.set_block_state_at(x, height + 1, z, 0, state);
                    }
                }
            }
        }

        let places = object.into_places();
        ctx.queue_object(places);
    }
}

impl_count_feature!(
    ScatterPlainsFlowerFeature,
    "minecraft:scatter_plains_flower_feature"
);

/// Sunflower patches (0.3 probability).
pub struct SunflowerDoublePlantPatchFeature {
    table: Arc<DecorationBlockTable>,
}

impl SunflowerDoublePlantPatchFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }

    fn count_base(&self) -> i32 {
        -1
    }

    fn count_random(&self) -> i32 {
        3
    }

    /// Java L21-52.
    fn count_populate(
        &self,
        ctx: &mut ChunkGenerateContext<'_>,
        random: &mut dyn RandomSourceProvider,
    ) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let random_x = random.next_int_max(15);
        let random_z = random.next_int_max(15);
        let source_x = (chunk_x << 4) + random_x;
        let source_z = (chunk_z << 4) + random_z;

        // Biome gate (source column).
        let height_at = ctx.chunk.height_map(random_x as u8, random_z as u8);
        if !biome_in(
            ctx.chunk
                .biome_id(random_x as u8, height_at, random_z as u8),
            SUNFLOWER_BIOMES,
        ) {
            return;
        }

        let chunk_ref: &WorldgenChunk = ctx.chunk;
        let mut object = BlockManager::with_chunk(chunk_ref);
        let radius = random_range(random, 2, 3);

        for x in source_x - radius..=source_x + radius {
            for z in source_z - radius..=source_z + radius {
                let dx = x - source_x;
                let dz = z - source_z;
                if dx * dx + dz * dz > radius * radius {
                    continue;
                }
                // Java L37: nextFloat() < 0.3f.
                if random.next_float() < 0.3f32 {
                    let local_x = (x & 0xF) as u8;
                    let local_z = (z & 0xF) as u8;
                    let height = chunk_ref.height_map(local_x, local_z);
                    // Supported grass grows two-high plants
                    // (no above-air check, matching upstream).
                    let top = chunk_ref.block_state(local_x, height, local_z, 0);
                    if self.table.is_grass_block(top) {
                        object.set_block_state_at(x, height + 1, z, 0, self.table.sunflower_lower);
                        object.set_block_state_at(x, height + 2, z, 0, self.table.sunflower_upper);
                    }
                }
            }
        }

        let places = object.into_places();
        ctx.queue_object(places);
    }
}

impl_count_feature!(
    SunflowerDoublePlantPatchFeature,
    "minecraft:sunflower_double_plant_patch_feature"
);

/// Short grass patches (mushroom islands excluded, custom scan floor).
pub struct TallGrassPatchFeature {
    table: Arc<DecorationBlockTable>,
}

impl TallGrassPatchFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }

    fn count_base(&self) -> i32 {
        10
    }

    fn count_random(&self) -> i32 {
        0
    }

    /// Custom populate: scan down above SEA_LEVEL, excluding mushroom islands.
    fn count_populate(
        &self,
        ctx: &mut ChunkGenerateContext<'_>,
        random: &mut dyn RandomSourceProvider,
    ) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        // Chunk center biome decides mushroom islands.
        let center_biome = ctx.chunk.biome_id(7, ctx.chunk.height_map(7, 7), 7);
        if center_biome == MUSHROOM_ISLAND {
            return;
        }

        let x = random.next_bounded_int(15);
        let z = random.next_bounded_int(15);
        let mut y = ctx.chunk.height_map(x as u8, z as u8);
        let world_x = (chunk_x << 4) + x;
        let world_z = (chunk_z << 4) + z;

        // Biome gate (column).
        if !biome_in(
            ctx.chunk.biome_id(x as u8, y, z as u8),
            TALL_GRASS_PATCH_BIOMES,
        ) {
            return;
        }

        // Java L38-41: while (!isSupportValid && y > SEA_LEVEL) y--.
        while !self
            .table
            .is_dirt_not_coarse(ctx.chunk.block_state(x as u8, y, z as u8, 0))
            && y > SEA_LEVEL
        {
            y -= 1;
        }

        // Java L43-45: y < SEA_LEVEL || !isSupportValid → return.
        if y < SEA_LEVEL
            || !self
                .table
                .is_dirt_not_coarse(ctx.chunk.block_state(x as u8, y, z as u8, 0))
        {
            return;
        }

        // Air required above.
        let chunk_ref: &WorldgenChunk = ctx.chunk;
        let mut manager = BlockManager::with_chunk(chunk_ref);
        if manager.get_block_if_cached_or_loaded(world_x, y + 1, world_z) != self.table.air {
            return;
        }

        // Java L53-55: place + queueObject.
        let mut object = BlockManager::with_chunk(chunk_ref);
        object.set_block_state_at(world_x, y + 1, world_z, 0, self.table.short_grass);
        let places = object.into_places();
        ctx.queue_object(places);
    }
}

impl_count_feature!(
    TallGrassPatchFeature,
    "minecraft:scatter_tall_grass_feature"
);

/// Desert cacti (columns plus half flowers; direct chunk writes).
pub struct DesertCactusFeature {
    table: Arc<DecorationBlockTable>,
}

impl DesertCactusFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }

    fn count_base(&self) -> i32 {
        2
    }

    fn count_random(&self) -> i32 {
        0
    }

    /// Java L35-62.
    fn count_populate(
        &self,
        ctx: &mut ChunkGenerateContext<'_>,
        random: &mut dyn RandomSourceProvider,
    ) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let x = random.next_bounded_int(13) + 1;
        let z = random.next_bounded_int(13) + 1;
        let y = ctx.chunk.height_map(x as u8, z as u8) + 1;
        let mut height = 1;
        let range = random.next_bounded_int(18);
        if range >= 16 {
            height = 3;
        } else if range >= 11 {
            height = 2;
        }

        // Biome gate (column).
        if !biome_in(ctx.chunk.biome_id(x as u8, y, z as u8), CACTUS_BIOMES) {
            return;
        }

        // Support needs the SAND tag.
        if !self
            .table
            .is_sand(ctx.chunk.block_state(x as u8, y - 1, z as u8, 0))
        {
            return;
        }
        // Four horizontal neighbors must flow through.
        // (x/z in 1..=13 keeps neighbors in-chunk).
        let world_x = (chunk_x << 4) + x;
        let world_z = (chunk_z << 4) + z;
        let sides_ok = [
            (world_x + 1, world_z),
            (world_x - 1, world_z),
            (world_x, world_z + 1),
            (world_x, world_z - 1),
        ]
        .iter()
        .all(|&(sx, sz)| {
            let side = ctx
                .chunk
                .block_state((sx & 0xF) as u8, y, (sz & 0xF) as u8, 0);
            self.table.can_be_flowed_into(side)
        });
        if !sides_ok {
            return;
        }

        // Cactus columns plus half top-flowers.
        if y > 0 {
            for i in 0..=height {
                set_block_with_heightmap(ctx.chunk, x as u8, y + i, z as u8, self.table.cactus);
            }
            if random.next_boolean() {
                set_block_with_heightmap(
                    ctx.chunk,
                    x as u8,
                    y + height + 1,
                    z as u8,
                    self.table.cactus_flower,
                );
            }
        }
    }
}

impl_count_feature!(
    DesertCactusFeature,
    "minecraft:desert_after_surface_cactus_feature_rules"
);

/// Waterlilies (sea-level water surface plus direct chunk writes).
pub struct WaterlilyFeature {
    table: Arc<DecorationBlockTable>,
}

impl WaterlilyFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }

    fn count_base(&self) -> i32 {
        4
    }

    fn count_random(&self) -> i32 {
        2
    }

    /// Java L36-46.
    fn count_populate(
        &self,
        ctx: &mut ChunkGenerateContext<'_>,
        random: &mut dyn RandomSourceProvider,
    ) {
        let x = random.next_int_max(15);
        let z = random.next_int_max(15);
        let y = ctx.chunk.height_map(x as u8, z as u8);
        if y == SEA_LEVEL {
            // Biome gate (column).
            if !biome_in(ctx.chunk.biome_id(x as u8, y, z as u8), WATERLILY_BIOMES) {
                return;
            }
            // Water surfaces grow lilies.
            if self
                .table
                .is_water(ctx.chunk.block_state(x as u8, y, z as u8, 0))
            {
                set_block_with_heightmap(ctx.chunk, x as u8, y + 1, z as u8, self.table.waterlily);
            }
        }
    }
}

impl_count_feature!(
    WaterlilyFeature,
    "minecraft:fixup_waterlily_position_feature"
);

// ---------------------------------------------------------------------------
// ReedsFeature.
// ---------------------------------------------------------------------------

/// Reeds (1/20 chunks, sea level plus up to 3 high).
pub struct ReedsFeature {
    table: Arc<DecorationBlockTable>,
}

impl ReedsFeature {
    pub fn new(table: Arc<DecorationBlockTable>) -> Self {
        Self { table }
    }

    /// Support subset: reeds below, or DIRT/SAND with adjacent water
    /// (frosted-ice/layer-1 approximations omitted).
    fn reeds_support_valid(&self, chunk: &WorldgenChunk, x: i32, y: i32, z: i32) -> bool {
        let block = chunk.block_state((x & 0xF) as u8, y, (z & 0xF) as u8, 0);
        if block == self.table.reeds {
            return true;
        }
        if !self.table.is_dirt_or_sand(block) {
            return false;
        }
        [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]
            .iter()
            .any(|&(nx, nz)| {
                self.table
                    .is_water(chunk.block_state((nx & 0xF) as u8, y, (nz & 0xF) as u8, 0))
            })
    }
}

impl GenerateFeature for ReedsFeature {
    /// Seed is `level.getSeed() ^ chunkHash` (no name hash).
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let seed = ctx.level_seed() ^ chunk_hash(chunk_x, chunk_z);
        let mut random = crate::worldgen::random::Xoroshiro128::new(seed);
        let _ = java_string_hashcode(self.name());

        // 1/20 chunk chance.
        if random.next_int_max(20) != 0 {
            return;
        }
        // Java L27: maxReed = random.nextInt(3).
        let mut max_reed = random.next_int_max(3);

        for x in 0..16i32 {
            for z in 0..16i32 {
                if ctx.chunk.height_map(x as u8, z as u8) == SEA_LEVEL {
                    // Biome gate (column).
                    if !biome_in(
                        ctx.chunk.biome_id(x as u8, SEA_LEVEL, z as u8),
                        PUMPKIN_REEDS_BIOMES,
                    ) {
                        continue;
                    }
                    let world_x = (chunk_x << 4) + x;
                    let world_z = (chunk_z << 4) + z;
                    if self.reeds_support_valid(ctx.chunk, world_x, SEA_LEVEL, world_z) {
                        // 3-high reeds.
                        for i in 1..4 {
                            set_block_with_heightmap(
                                ctx.chunk,
                                x as u8,
                                SEA_LEVEL + i,
                                z as u8,
                                self.table.reeds,
                            );
                        }
                        // Java L35: --maxReed <= 0 → return.
                        max_reed -= 1;
                        if max_reed <= 0 {
                            return;
                        }
                    }
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        "minecraft:reeds_feature"
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::holder::normal::NormalObjectHolder;
    use crate::worldgen::material::MaterialBlocks;
    use crate::worldgen::random::Xoroshiro128;
    use sc_world::chunk::ChunkPosition;

    /// Test block table with hand-registered ids.
    ///
    /// air must be the real `air_runtime_id()` (empty layers read it back);
    /// dummy 0 would fail every above-air check.
    fn test_table() -> DecorationBlockTable {
        let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        let id = |n: u32| BlockRuntimeId(n);
        let set = |ids: &[u32]| {
            ids.iter()
                .map(|n| BlockRuntimeId(*n))
                .collect::<HashSet<_>>()
        };
        DecorationBlockTable {
            air,
            dandelion: id(20),
            poppy: id(21),
            blue_orchid: id(22),
            azure_bluet: id(23),
            cornflower: id(24),
            oxeye_daisy: id(25),
            plains_flowers: [
                id(23),
                id(24),
                id(20),
                id(25),
                id(21),
                id(26),
                id(27),
                id(28),
                id(29),
            ],
            short_grass: id(30),
            tall_grass_lower: id(31),
            tall_grass_upper: id(32),
            fern: id(33),
            large_fern_lower: id(34),
            large_fern_upper: id(35),
            short_dry_grass: id(36),
            tall_dry_grass: id(37),
            dead_bush: id(38),
            bush: id(39),
            brown_mushroom: id(40),
            red_mushroom: id(41),
            sweet_berry_bush: id(42),
            cactus: id(43),
            cactus_flower: id(44),
            pumpkin: id(45),
            reeds: id(46),
            waterlily: id(47),
            sunflower_lower: id(48),
            sunflower_upper: id(49),
            dirt: set(&[1, 2, 3]),
            sand: set(&[5, 6]),
            grass_block: set(&[2]),
            coarse_dirt: set(&[3]),
            water: set(&[8]),
            non_solid: set(&[air.0, 30, 33]),
            liquids: set(&[8]),
        }
    }

    fn test_holder() -> NormalObjectHolder {
        let id = |n: u32| BlockRuntimeId(n);
        NormalObjectHolder::new(
            Xoroshiro128::new(42),
            MaterialBlocks {
                air: id(0),
                water: id(8),
                lava: id(9),
                stone: id(1),
                granite: id(10),
                tuff: id(11),
                copper_ore: id(12),
                deepslate_iron_ore: id(13),
                raw_copper_block: id(14),
                raw_iron_block: id(15),
            },
        )
    }

    /// Test grass chunk: every column tops grass_block at y=70,
    /// one biome id per column.
    fn grass_chunk(biome_id: i32) -> WorldgenChunk {
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        for x in 0..16u8 {
            for z in 0..16u8 {
                wc.set_block_state(x, 70, z, 0, BlockRuntimeId(2));
                wc.set_height_map(x, z, 70);
                wc.set_biome_id(x, 70, z, biome_id);
                wc.set_biome_id(x, 71, z, biome_id);
            }
        }
        wc
    }

    #[test]
    fn biome_lists_sane() {
        assert!(biome_in(PLAINS, PUMPKIN_REEDS_BIOMES));
        assert!(!biome_in(HELL, PUMPKIN_REEDS_BIOMES));
        assert!(biome_in(SWAMPLAND, WATERLILY_BIOMES));
        assert!(!biome_in(PLAINS, SUNFLOWER_BIOMES));
        assert!(biome_in(SUNFLOWER_PLAINS, SUNFLOWER_BIOMES));
        assert!(biome_in(DESERT, CACTUS_BIOMES));
        assert!(!biome_in(PLAINS, CACTUS_BIOMES));
        assert!(biome_in(TAIGA, TAIGA_FAMILY_BIOMES));
        assert_eq!(PUMPKIN_REEDS_BIOMES.len(), 82);
        assert_eq!(MUSHROOM_BIOMES.len(), 81);
    }

    #[test]
    fn surface_feature_places_grass_on_dirt() {
        let table = Arc::new(test_table());
        // Taiga grass: TAIGA biome, 8 base populates.
        let feature = TaigaGrassFeature::new(table.clone());
        let holder = test_holder();
        let mut wc = grass_chunk(TAIGA);
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 12345, -64, 319);
        feature.apply(&mut ctx);
        // Feature writes buffer in root; submit before they reach chunks.
        ctx.apply_root_to_chunk();

        // Some of 8 attempts should place fern/short_grass.
        let mut placed = 0;
        for x in 0..16u8 {
            for z in 0..16u8 {
                let above = ctx.chunk.block_state(x, 71, z, 0);
                if above == table.fern || above == table.short_grass {
                    placed += 1;
                }
            }
        }
        assert!(placed > 0, "taiga grass should place ferns, got {placed}");
    }

    #[test]
    fn surface_feature_biome_gated() {
        let table = Arc::new(test_table());
        // Jungle grass skips TAIGA biomes.
        let feature = JungleGrassFeature::new(table.clone());
        let holder = test_holder();
        let mut wc = grass_chunk(TAIGA);
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 12345, -64, 319);
        feature.apply(&mut ctx);
        for x in 0..16u8 {
            for z in 0..16u8 {
                let above = ctx.chunk.block_state(x, 71, z, 0);
                assert!(
                    above != table.fern && above != table.short_grass,
                    "jungle grass must not spawn in taiga"
                );
            }
        }
    }

    #[test]
    fn pumpkin_sparse() {
        let table = Arc::new(test_table());
        let feature = PumpkinGenerateFeature::new(table.clone());
        let holder = test_holder();
        // Most chunks place zero times; occasional single placements.
        let mut any_chunk_placed = false;
        for cx in 0..40 {
            let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(cx, 0));
            let mut wc = WorldgenChunk::new(chunk);
            for x in 0..16u8 {
                for z in 0..16u8 {
                    wc.set_block_state(x, 70, z, 0, BlockRuntimeId(2));
                    wc.set_height_map(x, z, 70);
                    wc.set_biome_id(x, 70, z, PLAINS);
                    wc.set_biome_id(x, 71, z, PLAINS);
                }
            }
            let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 777, -64, 319);
            feature.apply(&mut ctx);
            // Feature writes buffer in root; submit before they reach chunks.
            ctx.apply_root_to_chunk();
            for x in 0..16u8 {
                for z in 0..16u8 {
                    if ctx.chunk.block_state(x, 71, z, 0) == table.pumpkin {
                        any_chunk_placed = true;
                    }
                }
            }
        }
        assert!(any_chunk_placed, "pumpkin should appear in 40 chunks");
    }

    #[test]
    fn waterlily_on_swamp_water() {
        let table = Arc::new(test_table());
        let feature = WaterlilyFeature::new(table.clone());
        let holder = test_holder();
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        for x in 0..16u8 {
            for z in 0..16u8 {
                // Sea level water (heightmap = SEA_LEVEL).
                wc.set_block_state(x, SEA_LEVEL, z, 0, BlockRuntimeId(8));
                wc.set_height_map(x, z, SEA_LEVEL);
                wc.set_biome_id(x, SEA_LEVEL, z, SWAMPLAND);
                wc.set_biome_id(x, SEA_LEVEL + 1, z, SWAMPLAND);
            }
        }
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 999, -64, 319);
        feature.apply(&mut ctx);
        let mut placed = 0;
        for x in 0..16u8 {
            for z in 0..16u8 {
                if ctx.chunk.block_state(x, SEA_LEVEL + 1, z, 0) == table.waterlily {
                    placed += 1;
                }
            }
        }
        assert!(placed > 0, "waterlily should place, got {placed}");
    }

    /// Snapshot-first exact states: upper halves resolve from the bundle
    /// snapshot even when the global dictionary has no tall_grass (the
    /// dlopened-plugin case that used to warn and fall back to defaults).
    #[test]
    fn snapshot_exact_states_survive_empty_dictionary() {
        use sc_block::block_json::compile_bundle;
        use sc_packloader::block::{
            fingerprint_bundle, parse_block_file, BlockBundleBudgets, BlockJsonBundle,
        };
        let air_json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let grass_json = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {
                    "identifier": "minecraft:tall_grass",
                    "states": [{"name": "upper_block_bit", "values": [false, true]}]
                },
                "components": {},
                "sc:default_state": {"upper_block_bit": false},
                "sc:protocol_runtime_ids": [10, 11]
            }
        }"#;
        let raws = [
            (
                "definitions/blocks/minecraft/air.block.json",
                air_json.as_bytes().to_vec(),
            ),
            (
                "definitions/blocks/minecraft/tall_grass.block.json",
                grass_json.as_bytes().to_vec(),
            ),
        ];
        let budgets = BlockBundleBudgets::default();
        let mut refs: Vec<(&str, &[u8])> =
            raws.iter().map(|(p, b)| (*p, b.as_slice())).collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let files = refs
            .iter()
            .map(|(p, b)| parse_block_file("test", p, b, &budgets).unwrap())
            .collect();
        let bundle = BlockJsonBundle {
            schema_version: 1,
            network_id_mode: "hashed".to_string(),
            fingerprint: fingerprint_bundle(&refs, "hashed"),
            files,
        };
        let snap = compile_bundle(&bundle, "test", &budgets, &|_| None, &|_| true)
            .expect("tall_grass bundle compiles")
            .0;
        let table = DecorationBlockTable::from_block_snapshot(&snap);
        let lower = snap
            .find_state_hash("minecraft:tall_grass", &[("upper_block_bit", "false")])
            .expect("lower half in snapshot");
        let upper = snap
            .find_state_hash("minecraft:tall_grass", &[("upper_block_bit", "true")])
            .expect("upper half in snapshot");
        assert_ne!(lower, upper);
        assert_eq!(table.tall_grass_lower, BlockRuntimeId(lower));
        assert_eq!(table.tall_grass_upper, BlockRuntimeId(upper));
    }
}
