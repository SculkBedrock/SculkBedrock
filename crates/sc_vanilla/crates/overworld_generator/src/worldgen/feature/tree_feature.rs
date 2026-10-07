//! Port of the `feature/tree/` series (22 tree features).
//!
//! | Rust item | Upstream source |
//! |---|---|
//! | [`BambooJungleTreeFeature`] | `tree/BambooJungleTreeFeature.java` |
//! | [`BirchForestMutatedTreeFeature`] | `tree/BirchForestMutatedTreeFeature.java` |
//! | [`BirchForestTreeFeature`] | `tree/BirchForestTreeFeature.java` |
//! | [`CherryTreeFeature`] | `tree/CherryTreeFeature.java` |
//! | [`FlowerForestTreeFeature`] | `tree/FlowerForestTreeFeature.java` |
//! | [`ForestTreeFeature`] | `tree/ForestTreeFeature.java` |
//! | [`GroveTreeFeature`] | `tree/GroveTreeFeature.java` |
//! | [`IceSurfaceTreeFeature`] | `tree/IceSurfaceTreeFeature.java` |
//! | [`JungleBushFeature`] | `tree/JungleBushFeature.java` |
//! | [`JungleEdgeTreeFeature`] | `tree/JungleEdgeTreeFeature.java` |
//! | [`JungleTreeFeature`] | `tree/JungleTreeFeature.java` |
//! | [`MangroveTreeFeature`] | `tree/MangroveTreeFeature.java` |
//! | [`MeadowTreeFeature`] | `tree/MeadowTreeFeature.java` |
//! | [`MegaTaigaTreeFeature`] | `tree/MegaTaigaTreeFeature.java` |
//! | [`MesaTreeFeature`] | `tree/MesaTreeFeature.java` |
//! | [`PaleGardenTreeFeature`] | `tree/PaleGardenTreeFeature.java` |
//! | [`PlainsTreeFeature`] | `tree/PlainsTreeFeature.java` |
//! | [`RoofedForestTreeFeature`] | `tree/RoofedForestTreeFeature.java` |
//! | [`SavannaMutatedTreeFeature`] | `tree/SavannaMutatedTreeFeature.java` |
//! | [`SavannaTreeFeature`] | `tree/SavannaTreeFeature.java` |
//! | [`SwampTreeFeature`] | `tree/SwampTreeFeature.java` |
//! | [`TaigaTreeFeature`] | `tree/TaigaTreeFeature.java` |
//!
//! Not ported (blocked on later stages): `MushroomIslandMushroomFeature`,
//! `MesaPlateauStoneTreeFeature`.
//!
//! Inheritance (`FlowerForestTreeFeature extends ForestTreeFeature`,
//! `MeadowTreeFeature extends PlainsTreeFeature`,
//! `SavannaMutatedTreeFeature extends SavannaTreeFeature`) expands into
//! standalone implementations (overrides inlined, shared items re-declared).
//!
//! Upstream quirks (faithfully kept):
//! - `FlowerForestTreeFeature.getMin()=8 > getMax()=6`:
//!   the range helper's `nextInt() % -1` is always 0, so amount is fixed at 8;
//! - `PlainsTreeFeature.getGenerator` returns null 19/20, aborting apply
//!(see [`crate::worldgen::feature::legacy_tree::LegacyTreeGeneratorFeature`]);
//! - `TaigaTreeFeature`/`GroveTreeFeature` snow post-processing reads after
//!   the tree lands in the chunk (immediate commit here equals the
//!   upstream root buffer).

use std::sync::Arc;

use sc_world::chunk::BlockRuntimeId;

use crate::worldgen::biome::biome_id::COLD_TAIGA;
use crate::worldgen::context::{BlockManager, ChunkGenerateContext};
use crate::worldgen::feature::legacy_tree::{
    biome_id_matches_tag, LegacyObjectWrapper, LegacyTreeGenerator, LegacyTreeGeneratorFeature,
    LegacyTreeKind,
};
use crate::worldgen::feature::object::{
    GriddedFeature, ObjectGenerator, ObjectGeneratorFeature, TreeBlockTable, WoodType,
};
use crate::worldgen::feature::tree::{
    BigSpruceTree, CherryTree, DarkOakTree, FallenTree, FancyOakTree, JungleBigTree, JungleBush,
    JungleTree, MangroveTree, PaleOakTree, SavannaTree, SmallSpruceTree, SwampOakTree,
};
use crate::worldgen::feature::GenerateFeature;
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};

/// Shared template for `ObjectGeneratorFeature` subclasses: `apply` calls `object_apply`.
macro_rules! impl_object_feature {
    ($ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.object_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
    };
}

/// Shared template for `GriddedFeature` subclasses: `apply` calls `grid_apply`.
macro_rules! impl_grid_feature {
    ($ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.grid_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
    };
}

/// Shared template for `LegacyTreeGeneratorFeature` subclasses: `apply` calls `legacy_apply`.
macro_rules! impl_legacy_feature {
    ($ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.legacy_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
    };
}

/// 1% fallen-tree roll.
fn fallen_roll(random: &mut Xoroshiro128) -> bool {
    random.next_int_max(100) == 0
}

// ---------------------------------------------------------------------------
// ObjectGeneratorFeature family.
// ---------------------------------------------------------------------------

/// Java: `BambooJungleTreeFeature`(bamboo tag;JungleBigTree(10, 20)).
pub struct BambooJungleTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl BambooJungleTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(
    BambooJungleTreeFeature,
    "minecraft:bamboo_jungle_surface_trees_feature"
);

impl ObjectGeneratorFeature for BambooJungleTreeFeature {
    fn get_generator(&self, _random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        Box::new(JungleBigTree::new(10, 20, self.table.clone()))
    }

    fn get_min(&self) -> i32 {
        -1
    }

    fn get_max(&self) -> i32 {
        1
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("bamboo", biome_id)
    }

    /// Java L39-41:`super.checkBlock(bl) && !(bl instanceof BlockBamboo)`.
    fn check_block(&self, block: BlockRuntimeId) -> bool {
        self.tree_table().check_block(block) && block != self.tree_table().bamboo
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `CherryTreeFeature`(cherry_grove tag;min 2 max 4).
pub struct CherryTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl CherryTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(
    CherryTreeFeature,
    "minecraft:cherry_grove_after_surface_cherry_tree_feature_rules"
);

impl ObjectGeneratorFeature for CherryTreeFeature {
    fn get_generator(&self, _random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        Box::new(CherryTree::new(self.table.clone()))
    }

    fn get_min(&self) -> i32 {
        2
    }

    fn get_max(&self) -> i32 {
        4
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("cherry_grove", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Ice-surface trees (no biome filter; min -20 max 1).
pub struct IceSurfaceTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl IceSurfaceTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(IceSurfaceTreeFeature, "minecraft:ice_surface_trees_feature");

impl ObjectGeneratorFeature for IceSurfaceTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        if fallen_roll(random) {
            Box::new(FallenTree::of_wood(self.table.clone(), WoodType::Spruce))
        } else {
            Box::new(SmallSpruceTree::new(self.table.clone()))
        }
    }

    fn get_min(&self) -> i32 {
        -20
    }

    fn get_max(&self) -> i32 {
        1
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Jungle bushes (jungle tag; default min/max 5-6).
pub struct JungleBushFeature {
    table: Arc<TreeBlockTable>,
}

impl JungleBushFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(JungleBushFeature, "minecraft:jungle_bush");

impl ObjectGeneratorFeature for JungleBushFeature {
    fn get_generator(&self, _random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        Box::new(JungleBush::new(self.table.clone()))
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("jungle", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `JungleEdgeTreeFeature`(edge tag;min 1 max 2).
pub struct JungleEdgeTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl JungleEdgeTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(
    JungleEdgeTreeFeature,
    "minecraft:legacy:jungle_edge_tree_feature"
);

impl ObjectGeneratorFeature for JungleEdgeTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        match random.next_int_max(5) {
            0 | 1 => {
                if fallen_roll(random) {
                    Box::new(FallenTree::of_wood(self.table.clone(), WoodType::Jungle))
                } else {
                    Box::new(JungleTree::new(self.table.clone(), 7, 8))
                }
            }
            _ => Box::new(FancyOakTree::new(self.table.clone())),
        }
    }

    fn get_min(&self) -> i32 {
        1
    }

    fn get_max(&self) -> i32 {
        2
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("edge", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `MangroveTreeFeature`(mangrove_swamp tag;min 12 max 15).
pub struct MangroveTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl MangroveTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(
    MangroveTreeFeature,
    "minecraft:mangrove_swamp_mangrove_tree_feature"
);

impl ObjectGeneratorFeature for MangroveTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        // Java L89-92: tall = nextFloat() > 0.15;beenest = nextFloat() < 0.04.
        let mut tree = MangroveTree::new(self.table.clone(), random.next_float() > 0.15);
        tree.with_bee_nest = random.next_float() < 0.04;
        Box::new(tree)
    }

    fn get_min(&self) -> i32 {
        12
    }

    fn get_max(&self) -> i32 {
        15
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("mangrove_swamp", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `PaleGardenTreeFeature`(pale_garden tag;min 8 max 10;
/// 1/4 chance of tryCreakingHeart.
pub struct PaleGardenTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl PaleGardenTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(
    PaleGardenTreeFeature,
    "minecraft:random_pale_oak_tree_feature"
);

impl ObjectGeneratorFeature for PaleGardenTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        let mut tree = PaleOakTree::new(self.table.clone());
        tree.try_creaking_heart = random.next_int_max(4) == 0;
        Box::new(tree)
    }

    fn get_min(&self) -> i32 {
        8
    }

    fn get_max(&self) -> i32 {
        10
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("pale_garden", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `RoofedForestTreeFeature`(roofed tag;min 8 max 10;DarkOakTree).
pub struct RoofedForestTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl RoofedForestTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(
    RoofedForestTreeFeature,
    "minecraft:roofed_forest_tree_feature_rules"
);

impl ObjectGeneratorFeature for RoofedForestTreeFeature {
    fn get_generator(&self, _random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        // The shared generator singleton becomes a fresh stateless build.
        Box::new(DarkOakTree::new(self.table.clone()))
    }

    fn get_min(&self) -> i32 {
        8
    }

    fn get_max(&self) -> i32 {
        10
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("roofed", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `SavannaTreeFeature`(savanna tag;min 2 max 4;
/// 1/3 legacy oak, otherwise SavannaTree.
pub struct SavannaTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl SavannaTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(
    SavannaTreeFeature,
    "minecraft:savanna_surface_trees_feature"
);

impl ObjectGeneratorFeature for SavannaTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        if random.next_int_max(3) == 0 {
            Box::new(LegacyObjectWrapper::new(LegacyTreeGenerator::new(
                LegacyTreeKind::Oak,
                self.table.clone(),
            )))
        } else {
            Box::new(SavannaTree::new(self.table.clone()))
        }
    }

    fn get_min(&self) -> i32 {
        2
    }

    fn get_max(&self) -> i32 {
        4
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("savanna", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `SavannaMutatedTreeFeature extends SavannaTreeFeature`(savanna tag;
/// min 2 max 4; 1/3 fallen/legacy oak, otherwise SavannaTree.
pub struct SavannaMutatedTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl SavannaMutatedTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(
    SavannaMutatedTreeFeature,
    "minecraft:savanna_mutated_surface_trees_feature"
);

impl ObjectGeneratorFeature for SavannaMutatedTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        if random.next_int_max(3) == 0 {
            if fallen_roll(random) {
                Box::new(FallenTree::new(self.table.clone()))
            } else {
                Box::new(LegacyTreeGenerator::new(
                    LegacyTreeKind::Oak,
                    self.table.clone(),
                ))
            }
        } else {
            Box::new(SavannaTree::new(self.table.clone()))
        }
    }

    fn get_min(&self) -> i32 {
        2
    }

    fn get_max(&self) -> i32 {
        4
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("savanna", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `SwampTreeFeature`(swamp tag;min 3 max 5;SwampOakTree(7, 8);
/// checkBlock overridden as canBeReplaced.
pub struct SwampTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl SwampTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_object_feature!(SwampTreeFeature, "minecraft:swamp_oak_tree_feature");

impl ObjectGeneratorFeature for SwampTreeFeature {
    fn get_generator(&self, _random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        Box::new(SwampOakTree::new(self.table.clone(), 7, 8))
    }

    fn get_min(&self) -> i32 {
        3
    }

    fn get_max(&self) -> i32 {
        5
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("swamp", biome_id)
    }

    /// checkBlock delegates to bl.canBeReplaced (liquids included).
    fn check_block(&self, block: BlockRuntimeId) -> bool {
        self.tree_table().can_be_replaced(block)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

// ---------------------------------------------------------------------------
// GriddedFeature family.
// ---------------------------------------------------------------------------

/// Mutated birch forest (no biome override; fancy tall birch
/// mixed with fallen trees).
pub struct BirchForestMutatedTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl BirchForestMutatedTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_grid_feature!(
    BirchForestMutatedTreeFeature,
    "minecraft:legacy:birch_forest_mutated_tree_feature"
);

impl ObjectGeneratorFeature for BirchForestMutatedTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        let fallen = fallen_roll(random);
        if random.next_boolean() {
            if fallen {
                Box::new(FallenTree::with_lengths(
                    self.table.clone(),
                    WoodType::Birch,
                    4,
                    10,
                ))
            } else {
                Box::new(LegacyTreeGenerator::new(
                    LegacyTreeKind::TallBirch,
                    self.table.clone(),
                ))
            }
        } else if fallen {
            Box::new(FallenTree::of_wood(self.table.clone(), WoodType::Birch))
        } else {
            Box::new(LegacyTreeGenerator::new(
                LegacyTreeKind::Birch,
                self.table.clone(),
            ))
        }
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

impl GriddedFeature for BirchForestMutatedTreeFeature {}

/// Java: `GroveTreeFeature`(grove tag;split 4;SmallSpruceTree;
/// Snow lands on spruce leaves after the tree.
pub struct GroveTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl GroveTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl GenerateFeature for GroveTreeFeature {
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        self.grid_apply(ctx);

        // Sweep the heightmap top block, snowing spruce leaves. Upstream reads
        // the uncommitted chunk (the tree sits in the root buffer, so terrain
        // tops can never be spruce leaves) and this path never triggers there;
        // the quirk stays for parity.
        let snow_layer = self.table.snow_layer;
        let spruce_leaves = self.table.leaves[WoodType::Spruce.index()];
        let chunk_ref: &crate::worldgen::chunk::WorldgenChunk = ctx.chunk;
        let mut object = BlockManager::with_chunk_and_seed(chunk_ref, ctx.level_seed());
        for x in 0..16u8 {
            for z in 0..16u8 {
                let y = ctx.chunk.height_map(x, z);
                let support = ctx.chunk.block_state(x, y, z, 0);
                if support == spruce_leaves {
                    let wx = x as i32 + (ctx.chunk.x() << 4);
                    let wz = z as i32 + (ctx.chunk.z() << 4);
                    object.set_block_state_at(wx, y + 1, wz, 0, snow_layer);
                }
            }
        }
        let places = object.into_places();
        ctx.queue_object(places);
    }

    fn name(&self) -> &'static str {
        "minecraft:grove_spruce_tree_feature"
    }
}

impl ObjectGeneratorFeature for GroveTreeFeature {
    fn get_generator(&self, _random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        Box::new(SmallSpruceTree::new(self.table.clone()))
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("grove", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

impl GriddedFeature for GroveTreeFeature {
    fn get_split(&self) -> i32 {
        4
    }
}

/// Jungle trees (jungle tag; four-way mix).
pub struct JungleTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl JungleTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_grid_feature!(JungleTreeFeature, "minecraft:jungle_surface_trees_feature");

impl ObjectGeneratorFeature for JungleTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        match random.next_int_max(10) {
            0 => Box::new(JungleBigTree::new(10, 20, self.table.clone())),
            4 | 5 | 6 => {
                if fallen_roll(random) {
                    Box::new(FallenTree::of_wood(self.table.clone(), WoodType::Jungle))
                } else {
                    Box::new(JungleTree::new(
                        self.table.clone(),
                        4 + random.next_bounded_int(7),
                        3,
                    ))
                }
            }
            7 | 8 => Box::new(FancyOakTree::new(self.table.clone())),
            _ => {
                if fallen_roll(random) {
                    Box::new(FallenTree::of_wood(self.table.clone(), WoodType::Jungle))
                } else {
                    Box::new(JungleTree::new(self.table.clone(), 7, 8))
                }
            }
        }
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("jungle", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

impl GriddedFeature for JungleTreeFeature {}

/// Java: `MegaTaigaTreeFeature`(taiga tag;distance 1;
/// 2/5 BigSpruceTree, otherwise SmallSpruceTree.
pub struct MegaTaigaTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl MegaTaigaTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_grid_feature!(
    MegaTaigaTreeFeature,
    "minecraft:mega_taiga_surface_trees_feature"
);

impl ObjectGeneratorFeature for MegaTaigaTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        if random.next_int_max(5) < 2 {
            Box::new(BigSpruceTree::new(self.table.clone()))
        } else {
            Box::new(SmallSpruceTree::new(self.table.clone()))
        }
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("taiga", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

impl GriddedFeature for MegaTaigaTreeFeature {
    fn get_distance_to_next_field(&self) -> i32 {
        1
    }
}

/// Taiga trees (taiga tag; 1% fallen spruce; snow post-processing).
pub struct TaigaTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl TaigaTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl GenerateFeature for TaigaTreeFeature {
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        self.grid_apply(ctx);

        // Snow over solid blocks in COLD_TAIGA biomes. `above` reads the root
        // buffer holding this round queued trunk, so the trunk bottom
        // is not buried in snow. air becomes snow layer 0; flowables
        // (plants/snow) become layer 1, keeping what sits below.
        let snow_layer = self.table.snow_layer;
        let air = self.table.air;
        let chunk_ref: &crate::worldgen::chunk::WorldgenChunk = ctx.chunk;
        let mut object = BlockManager::with_chunk_and_seed(chunk_ref, ctx.level_seed());
        for x in 0..16u8 {
            for z in 0..16u8 {
                let y = ctx.chunk.height_map(x, z);
                let support = ctx.chunk.block_state(x, y, z, 0);
                if self.table.is_solid(support) {
                    let wx = x as i32 + (ctx.chunk.x() << 4);
                    let wz = z as i32 + (ctx.chunk.z() << 4);
                    if ctx.chunk.biome_id(x, y, z) == COLD_TAIGA {
                        let above = ctx.root_cached_block(wx, y + 1, wz);
                        if above == air {
                            object.set_block_state_at(wx, y + 1, wz, 0, snow_layer);
                        } else if !self.table.is_solid(above) {
                            // Flowable approximation (only plants or snow can sit
                            // at heightmap+1; leaves never do).
                            object.set_block_state_at(wx, y + 1, wz, 1, snow_layer);
                        }
                    }
                }
            }
        }
        let places = object.into_places();
        ctx.queue_object(places);
    }

    fn name(&self) -> &'static str {
        "minecraft:taiga_surface_trees_feature"
    }
}

impl ObjectGeneratorFeature for TaigaTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator> {
        if fallen_roll(random) {
            Box::new(FallenTree::of_wood(self.table.clone(), WoodType::Spruce))
        } else {
            Box::new(SmallSpruceTree::new(self.table.clone()))
        }
    }

    fn can_spawn_here(&self, biome_id: i32) -> bool {
        biome_id_matches_tag("taiga", biome_id)
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

impl GriddedFeature for TaigaTreeFeature {}

// ---------------------------------------------------------------------------
// LegacyTreeGeneratorFeature family.
// ---------------------------------------------------------------------------

/// Birch forest (birch tag; min 7 max 8; beehive chance 0.00035).
pub struct BirchForestTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl BirchForestTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_legacy_feature!(
    BirchForestTreeFeature,
    "minecraft:birch_forest_surface_trees_feature"
);

impl LegacyTreeGeneratorFeature for BirchForestTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Option<Box<dyn ObjectGenerator>> {
        if fallen_roll(random) {
            Some(Box::new(FallenTree::of_wood(
                self.table.clone(),
                WoodType::Birch,
            )))
        } else {
            Some(Box::new(LegacyTreeGenerator::new(
                LegacyTreeKind::Birch,
                self.table.clone(),
            )))
        }
    }

    fn get_min(&self) -> i32 {
        7
    }

    fn get_max(&self) -> i32 {
        8
    }

    fn get_required_tag(&self) -> &'static str {
        "birch"
    }

    fn get_bee_nest_chance(&self) -> f32 {
        0.00035
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `FlowerForestTreeFeature extends ForestTreeFeature`
/// (flower_forest tag; min 8 max 6, quirk-fixed at 8; beehive chance 0.03).
pub struct FlowerForestTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl FlowerForestTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_legacy_feature!(
    FlowerForestTreeFeature,
    "minecraft:flower_forest_surface_trees_feature"
);

impl LegacyTreeGeneratorFeature for FlowerForestTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Option<Box<dyn ObjectGenerator>> {
        let fallen = fallen_roll(random);
        if random.next_int_max(10) < 6 {
            Some(Box::new(LegacyTreeGenerator::new(
                LegacyTreeKind::Oak,
                self.table.clone(),
            )))
        } else if fallen {
            Some(Box::new(FallenTree::of_wood(
                self.table.clone(),
                WoodType::Birch,
            )))
        } else {
            Some(Box::new(LegacyTreeGenerator::new(
                LegacyTreeKind::Birch,
                self.table.clone(),
            )))
        }
    }

    fn get_min(&self) -> i32 {
        8
    }

    fn get_max(&self) -> i32 {
        6
    }

    fn get_required_tag(&self) -> &'static str {
        "flower_forest"
    }

    fn get_bee_nest_chance(&self) -> f32 {
        0.03
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Forest trees (forest tag; min 7 max 8; beehive chance 0.00035).
pub struct ForestTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl ForestTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_legacy_feature!(ForestTreeFeature, "minecraft:forest_surface_trees_feature");

impl LegacyTreeGeneratorFeature for ForestTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Option<Box<dyn ObjectGenerator>> {
        let fallen = fallen_roll(random);
        if random.next_int_max(10) < 6 {
            if fallen {
                Some(Box::new(FallenTree::new(self.table.clone())))
            } else {
                Some(Box::new(LegacyTreeGenerator::new(
                    LegacyTreeKind::Oak,
                    self.table.clone(),
                )))
            }
        } else if fallen {
            Some(Box::new(FallenTree::of_wood(
                self.table.clone(),
                WoodType::Birch,
            )))
        } else {
            Some(Box::new(LegacyTreeGenerator::new(
                LegacyTreeKind::Birch,
                self.table.clone(),
            )))
        }
    }

    fn get_min(&self) -> i32 {
        7
    }

    fn get_max(&self) -> i32 {
        8
    }

    fn get_required_tag(&self) -> &'static str {
        "forest"
    }

    fn get_bee_nest_chance(&self) -> f32 {
        0.00035
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Java: `MeadowTreeFeature extends PlainsTreeFeature`
/// (meadow tag; min 1 max 1; beehive chance 1.0, always).
pub struct MeadowTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl MeadowTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_legacy_feature!(MeadowTreeFeature, "minecraft:meadow_surface_trees_feature");

impl LegacyTreeGeneratorFeature for MeadowTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Option<Box<dyn ObjectGenerator>> {
        // Null generator 19/20 aborts apply.
        if random.next_int_max(20) < 1 {
            if fallen_roll(random) {
                Some(Box::new(FallenTree::new(self.table.clone())))
            } else {
                Some(Box::new(LegacyTreeGenerator::new(
                    LegacyTreeKind::Oak,
                    self.table.clone(),
                )))
            }
        } else {
            None
        }
    }

    fn get_min(&self) -> i32 {
        1
    }

    fn get_max(&self) -> i32 {
        1
    }

    fn get_required_tag(&self) -> &'static str {
        "meadow"
    }

    fn get_bee_nest_chance(&self) -> f32 {
        1.0
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Mesa trees (mesa tag; default min/max 5-6).
pub struct MesaTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl MesaTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_legacy_feature!(MesaTreeFeature, "minecraft:mesa_tree_feature");

impl LegacyTreeGeneratorFeature for MesaTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Option<Box<dyn ObjectGenerator>> {
        if fallen_roll(random) {
            Some(Box::new(FallenTree::new(self.table.clone())))
        } else {
            Some(Box::new(LegacyTreeGenerator::new(
                LegacyTreeKind::Oak,
                self.table.clone(),
            )))
        }
    }

    fn get_required_tag(&self) -> &'static str {
        "mesa"
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

/// Plains trees (plains tag; min 1 max 1; beehive chance 0.05;
/// null 19/20 for the savanna effect).
pub struct PlainsTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl PlainsTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

impl_legacy_feature!(PlainsTreeFeature, "minecraft:plains_surface_trees_feature");

impl LegacyTreeGeneratorFeature for PlainsTreeFeature {
    fn get_generator(&self, random: &mut Xoroshiro128) -> Option<Box<dyn ObjectGenerator>> {
        if random.next_int_max(20) < 1 {
            if fallen_roll(random) {
                Some(Box::new(FallenTree::new(self.table.clone())))
            } else {
                Some(Box::new(LegacyTreeGenerator::new(
                    LegacyTreeKind::Oak,
                    self.table.clone(),
                )))
            }
        } else {
            None
        }
    }

    fn get_min(&self) -> i32 {
        1
    }

    fn get_max(&self) -> i32 {
        1
    }

    fn get_required_tag(&self) -> &'static str {
        "plains"
    }

    fn get_bee_nest_chance(&self) -> f32 {
        0.05
    }

    fn tree_table(&self) -> &TreeBlockTable {
        &self.table
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::chunk::WorldgenChunk;
    use crate::worldgen::holder::normal::NormalObjectHolder;
    use crate::worldgen::material::MaterialBlocks;
    use crate::worldgen::random::Xoroshiro128;
    use sc_world::chunk::{BlockRuntimeId, Chunk, ChunkPosition};

    fn test_material_blocks() -> MaterialBlocks {
        let id = |n: u32| BlockRuntimeId(n);
        MaterialBlocks {
            air: id(0),
            water: id(4),
            lava: id(28),
            stone: id(1),
            granite: id(34),
            tuff: id(35),
            copper_ore: id(12),
            deepslate_iron_ore: id(21),
            raw_copper_block: id(50),
            raw_iron_block: id(51),
        }
    }

    #[test]
    fn feature_names_match_upstream() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        assert_eq!(
            BambooJungleTreeFeature::new(table.clone()).name(),
            "minecraft:bamboo_jungle_surface_trees_feature"
        );
        assert_eq!(
            SavannaMutatedTreeFeature::new(table.clone()).name(),
            "minecraft:savanna_mutated_surface_trees_feature"
        );
        assert_eq!(
            GroveTreeFeature::new(table.clone()).name(),
            "minecraft:grove_spruce_tree_feature"
        );
        assert_eq!(
            TaigaTreeFeature::new(table).name(),
            "minecraft:taiga_surface_trees_feature"
        );
    }

    #[test]
    fn plains_generator_mostly_none() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let feature = PlainsTreeFeature::new(table);
        let mut random = Xoroshiro128::new(42);
        // Null comes up 19/20 (multi-seed sampling shows both outcomes).
        let mut some = 0;
        let mut none = 0;
        for _ in 0..200 {
            match feature.get_generator(&mut random) {
                Some(_) => some += 1,
                None => none += 1,
            }
        }
        assert!(
            some > 0 && none > 0,
            "plains generator should mix Some/None"
        );
    }

    #[test]
    fn flower_forest_amount_quirk_is_eight() {
        // Reversed range (8, 6) fixes amount at 8.
        let mut random = Xoroshiro128::new(7);
        for _ in 0..50 {
            let amount = crate::worldgen::math::random_range(&mut random, 8, 6);
            assert_eq!(amount, 8);
        }
    }

    #[test]
    fn bamboo_check_block_excludes_bamboo() {
        // Tests run without a version-pack palette (only air/unknown sentinels),
        // so unregistered entries fall back to air and bamboo becomes
        // indistinguishable from it. Register the 5 checked ids by hand.
        let dictionary = sc_world::block_dictionary::BlockStateDictionary::global();
        for name in [
            "minecraft:bamboo",
            "minecraft:water",
            "minecraft:flowing_water",
            "minecraft:lava",
            "minecraft:flowing_lava",
        ] {
            dictionary.record_with(
                sc_world::leveldb::block_hash::block_state_hash(name, None),
                || sc_world::block_dictionary::BlockStateEntry {
                    name: name.to_string(),
                    states: None,
                },
            );
        }
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let feature = BambooJungleTreeFeature::new(table.clone());
        // check_block excludes bamboo (air passes, bamboo does not).
        assert!(!feature.check_block(table.bamboo));
        assert!(feature.check_block(table.air));
    }

    #[test]
    fn swamp_check_block_allows_replaceable() {
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let feature = SwampTreeFeature::new(table.clone());
        // No liquid exclusion after override: water is replaceable too.
        assert!(feature.check_block(table.air));
        assert!(feature.check_block(table.water));
    }

    #[test]
    fn taiga_snow_pass_places_snow_on_solid_ground() {
        // Test chunk: grass top at height 70, COLD_TAIGA biome.
        let table = Arc::new(TreeBlockTable::from_core_palette());
        let mut chunk = Chunk::empty_overworld(ChunkPosition::new(0, 0));
        chunk.set_block_at(0, 8, 69, 8, table.dirt).unwrap();
        let mut wc = WorldgenChunk::new(chunk);
        wc.set_height_map(8, 8, 70);
        wc.set_biome_id(8, 70, 8, COLD_TAIGA);

        let holder = NormalObjectHolder::new(Xoroshiro128::new(42), test_material_blocks());
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);

        let feature = TaigaTreeFeature::new(table.clone());
        feature.apply(&mut ctx);

        // Solid dirt on top snows at y+1 (layer 0 or 1).
        let snow_at_0 = wc.block_state(8, 71, 8, 0) == table.snow_layer;
        let snow_at_1 = wc.block_state(8, 71, 8, 1) == table.snow_layer;
        assert!(
            snow_at_0 || snow_at_1,
            "snow layer should be placed above solid ground in cold taiga"
        );
    }
}
