//! 树木特征：按生物群系混合放置各类树木、灌木与蜂巢。
//!
//! Tree features: placing trees, bushes, and beehives mixed by biome.

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

/// 1% fallen-tree roll.
fn fallen_roll(random: &mut Xoroshiro128) -> bool {
    random.next_int_max(100) == 0
}

// ---------------------------------------------------------------------------
// ObjectGeneratorFeature family.
// ---------------------------------------------------------------------------

/// 生成竹林树木的生成特征：数量 10–20，使用丛林大树生成器。
///
/// Generation feature for bamboo-jungle trees, placing 10-20 jungle big trees.
pub struct BambooJungleTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl BambooJungleTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, 
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

/// 生成樱花树的生成特征：数量 2–4。
///
/// Generation feature for cherry trees, placing 2-4 per attempt.
pub struct CherryTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl CherryTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, 
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

/// 在冰面生成树木的生成特征：数量基数 -20、随机范围 1，不做群系过滤。
///
/// Generation feature for ice-surface trees with base count -20 and random range 1, without a biome filter.
pub struct IceSurfaceTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl IceSurfaceTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, IceSurfaceTreeFeature, "minecraft:ice_surface_trees_feature");

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

/// 生成丛林灌木的生成特征：数量 5–6。
///
/// Generation feature for jungle bushes, placing 5-6 per attempt.
pub struct JungleBushFeature {
    table: Arc<TreeBlockTable>,
}

impl JungleBushFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, JungleBushFeature, "minecraft:jungle_bush");

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

/// 生成丛林边缘树木的生成特征：数量 1–2。
///
/// Generation feature for jungle-edge trees, placing 1-2 per attempt.
pub struct JungleEdgeTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl JungleEdgeTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, 
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

/// 生成红树林树木的生成特征：数量 12–15。
///
/// Generation feature for mangrove trees, placing 12-15 per attempt.
pub struct MangroveTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl MangroveTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, 
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

/// 生成苍白花园树木的生成特征：数量 8–10，并以四分之一概率尝试生成嘎枝之心。
///
/// Generation feature for pale-garden trees, placing 8-10 per attempt with a one-in-four chance to try placing a creaking heart.
pub struct PaleGardenTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl PaleGardenTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, 
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

/// 生成黑森林树木的生成特征：数量 8–10，使用深色橡木。
///
/// Generation feature for roofed-forest trees, placing 8-10 dark oaks per attempt.
pub struct RoofedForestTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl RoofedForestTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, 
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

/// 生成热带草原树木的生成特征：数量 2–4，约三分之一为原版橡木，其余为金合欢。
///
/// Generation feature for savanna trees, placing 2-4 per attempt with roughly one third legacy oaks and the rest acacias.
pub struct SavannaTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl SavannaTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, 
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

/// 生成变种热带草原树木的生成特征：数量 2–4，约三分之一为倒伏或原版橡木，其余为金合欢。
///
/// Generation feature for mutated savanna trees, placing 2-4 per attempt with roughly one third fallen or legacy oaks and the rest acacias.
pub struct SavannaMutatedTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl SavannaMutatedTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, 
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

/// 生成沼泽橡木的生成特征：数量 3–5，树高 7–8，并以可替换方块判定支撑。
///
/// Generation feature for swamp oaks, placing 3-5 per attempt with heights 7-8 and a replaceable-block support check.
pub struct SwampTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl SwampTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(object, SwampTreeFeature, "minecraft:swamp_oak_tree_feature");

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

/// 生成变种桦木森林树木的生成特征：高大桦木与倒伏树木混合。
///
/// Generation feature for mutated birch-forest trees, mixing tall birches with fallen trees.
pub struct BirchForestMutatedTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl BirchForestMutatedTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(grid, 
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

/// 生成雪林树木的生成特征：四分之一为大型云杉，生成后为云杉叶覆雪。
///
/// Generation feature for grove trees, using large spruces for one quarter of placements and adding snow to spruce leaves afterwards.
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

/// 生成丛林树木的生成特征：按四种树型混合放置。
///
/// Generation feature for jungle trees, mixing four tree shapes.
pub struct JungleTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl JungleTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(grid, JungleTreeFeature, "minecraft:jungle_surface_trees_feature");

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

/// 生成巨型针叶林树木的生成特征：五分之二为大型云杉，其余为小型云杉。
///
/// Generation feature for mega-taiga trees, using big spruces for two fifths of placements and small spruces otherwise.
pub struct MegaTaigaTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl MegaTaigaTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(grid, 
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

/// 生成针叶林树木的生成特征：百分之一为倒伏云杉，并在生成后处理积雪。
///
/// Generation feature for taiga trees, placing fallen spruces for one percent of attempts and applying snow afterwards.
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

/// 生成桦木森林树木的生成特征：数量 7–8，蜂巢概率 0.00035。
///
/// Generation feature for birch-forest trees, placing 7-8 per attempt with a beehive chance of 0.00035.
pub struct BirchForestTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl BirchForestTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(legacy, 
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

/// 生成繁花森林树木的生成特征：数量 8，蜂巢概率 0.03。
///
/// Generation feature for flower-forest trees, placing 8 per attempt with a beehive chance of 0.03.
pub struct FlowerForestTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl FlowerForestTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(legacy, 
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

/// 生成森林树木的生成特征：数量 7–8，蜂巢概率 0.00035。
///
/// Generation feature for forest trees, placing 7-8 per attempt with a beehive chance of 0.00035.
pub struct ForestTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl ForestTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(legacy, ForestTreeFeature, "minecraft:forest_surface_trees_feature");

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

/// 生成草甸树木的生成特征：数量 1，必定附带蜂巢。
///
/// Generation feature for meadow trees, placing one tree with a guaranteed beehive.
pub struct MeadowTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl MeadowTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(legacy, MeadowTreeFeature, "minecraft:meadow_surface_trees_feature");

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

/// 生成恶地树木的生成特征：数量 5–6。
///
/// Generation feature for mesa trees, placing 5-6 per attempt.
pub struct MesaTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl MesaTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(legacy, MesaTreeFeature, "minecraft:mesa_tree_feature");

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

/// 生成平原树木的生成特征：数量 1，蜂巢概率 0.05，另有约二十分之十九的概率不生成。
///
/// Generation feature for plains trees, placing one tree with a beehive chance of 0.05 and otherwise skipping with probability about nineteen in twenty.
pub struct PlainsTreeFeature {
    table: Arc<TreeBlockTable>,
}

impl PlainsTreeFeature {
    pub fn new(table: Arc<TreeBlockTable>) -> Self {
        Self { table }
    }
}

crate::impl_feature!(legacy, PlainsTreeFeature, "minecraft:plains_surface_trees_feature");

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
