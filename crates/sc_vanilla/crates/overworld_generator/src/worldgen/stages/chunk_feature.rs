//! Chunk feature stage: ore, tree, and surface-decoration features.
//!
//! Notes:
//! - The feature list is a hardcoded list of all overworld underground ores,
//!   with tree and decoration features appended afterwards; a future data-driven
//!   lookup can replace the static registration.
//! - Each feature applies directly to the chunk; there is no cross-chunk routing.
//! - The chunk state is set to `Populated` at the end of the stage.
//! - The hardcoded list does not depend on per-chunk biome scans; biome filtering
//!   is done per column by each feature.

use crate::blocks_table::OreBlockTable;
use crate::worldgen::chunk::ChunkState;
use crate::worldgen::context::ChunkGenerateContext;
use crate::worldgen::feature::decoration::{
    BushFeature, DeadBushFeature, DecorationBlockTable, DesertCactusFeature, JungleGrassFeature,
    PumpkinGenerateFeature, ReedsFeature, ScatterBrownMushroomFeature, ScatterDryGrassFeature,
    ScatterOverworldFlowerFeature, ScatterPlainsFlowerFeature, ScatterRedMushroomFeature,
    ScatterSweetBerryBushFeature, SunflowerDoublePlantPatchFeature, SwampFlowerFeature,
    TaigaGrassFeature, TallFernPatchFeature, TallGrassGenerateFeature, TallGrassPatchFeature,
    WaterlilyFeature,
};
use crate::worldgen::feature::object::TreeBlockTable;
use crate::worldgen::feature::ore::{
    build_overworld_ore_specs, EmeraldOreSurfaceFeature, OreFeature,
};
use crate::worldgen::feature::tree_feature::{
    BambooJungleTreeFeature, BirchForestMutatedTreeFeature, BirchForestTreeFeature,
    CherryTreeFeature, FlowerForestTreeFeature, ForestTreeFeature, GroveTreeFeature,
    IceSurfaceTreeFeature, JungleBushFeature, JungleEdgeTreeFeature, JungleTreeFeature,
    MangroveTreeFeature, MeadowTreeFeature, MegaTaigaTreeFeature, MesaTreeFeature,
    PaleGardenTreeFeature, PlainsTreeFeature, RoofedForestTreeFeature, SavannaMutatedTreeFeature,
    SavannaTreeFeature, SwampTreeFeature, TaigaTreeFeature,
};
use crate::worldgen::feature::GenerateFeature;
use crate::worldgen::stages::GenerateStage;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// NormalChunkFeatureStage: applies ore/tree/decoration features in order
// ---------------------------------------------------------------------------

/// Chunk feature stage.
///
/// Applies each registered feature in order, then sets the chunk state to
/// `Populated`. The feature list starts as all overworld underground ores;
/// tree and decoration features are appended by the `with_*` constructors,
/// ordered so biome filtering happens per column inside each feature.
pub struct NormalChunkFeatureStage {
    features: Vec<Arc<dyn GenerateFeature>>,
}

impl NormalChunkFeatureStage {
    /// Builds the stage with the ore feature list.
    pub fn new(ore_table: OreBlockTable) -> Self {
        let specs = build_overworld_ore_specs(&ore_table);
        let mut features: Vec<Arc<dyn GenerateFeature>> = specs
            .into_iter()
            .map(|spec| {
                Arc::new(OreFeature::new(ore_table.clone(), spec)) as Arc<dyn GenerateFeature>
            })
            .collect();
        // Surface emerald (count-based generate feature).
        features.push(Arc::new(EmeraldOreSurfaceFeature::new(ore_table)));
        Self { features }
    }

    /// Appends the 22 tree features after the ore list.
    ///
    /// Static full registration: biome filtering is done per column by each
    /// feature's own spawn/tag check.
    /// Not registered: mushroom-island mushroom and mesa-plateau stone-tree
    /// features (they need block types not covered here).
    pub fn with_tree_features(ore_table: OreBlockTable, tree_table: Arc<TreeBlockTable>) -> Self {
        let mut stage = Self::new(ore_table);
        let t = &tree_table;
        let tree_features: Vec<Arc<dyn GenerateFeature>> = vec![
            Arc::new(BambooJungleTreeFeature::new(t.clone())),
            Arc::new(BirchForestMutatedTreeFeature::new(t.clone())),
            Arc::new(BirchForestTreeFeature::new(t.clone())),
            Arc::new(CherryTreeFeature::new(t.clone())),
            Arc::new(FlowerForestTreeFeature::new(t.clone())),
            Arc::new(ForestTreeFeature::new(t.clone())),
            Arc::new(GroveTreeFeature::new(t.clone())),
            Arc::new(IceSurfaceTreeFeature::new(t.clone())),
            Arc::new(JungleBushFeature::new(t.clone())),
            Arc::new(JungleEdgeTreeFeature::new(t.clone())),
            Arc::new(JungleTreeFeature::new(t.clone())),
            Arc::new(MangroveTreeFeature::new(t.clone())),
            Arc::new(MeadowTreeFeature::new(t.clone())),
            Arc::new(MegaTaigaTreeFeature::new(t.clone())),
            Arc::new(MesaTreeFeature::new(t.clone())),
            Arc::new(PaleGardenTreeFeature::new(t.clone())),
            Arc::new(PlainsTreeFeature::new(t.clone())),
            Arc::new(RoofedForestTreeFeature::new(t.clone())),
            Arc::new(SavannaMutatedTreeFeature::new(t.clone())),
            Arc::new(SavannaTreeFeature::new(t.clone())),
            Arc::new(SwampTreeFeature::new(t.clone())),
            Arc::new(TaigaTreeFeature::new(t.clone())),
        ];
        stage.features.extend(tree_features);
        stage
    }

    /// Appends the 20 surface-decoration features after the tree features.
    ///
    /// Ordered flowers/grass first, then mushrooms, shrubs, and finally
    /// cactus/pumpkin/reeds/lily pads. Biome filtering is done per column by
    /// each feature's biome-id list.
    /// Not registered: mushroom-island mushroom and mesa-plateau stone-tree
    /// features (they need block types with cross-chunk routing).
    pub fn with_decoration_features(
        ore_table: OreBlockTable,
        tree_table: Arc<TreeBlockTable>,
        decoration_table: Arc<DecorationBlockTable>,
    ) -> Self {
        let mut stage = Self::with_tree_features(ore_table, tree_table);
        let d = &decoration_table;
        let decoration_features: Vec<Arc<dyn GenerateFeature>> = vec![
            // Flowers (patch/single).
            Arc::new(ScatterOverworldFlowerFeature::new(d.clone())),
            Arc::new(ScatterPlainsFlowerFeature::new(d.clone())),
            Arc::new(SwampFlowerFeature::new(d.clone())),
            Arc::new(SunflowerDoublePlantPatchFeature::new(d.clone())),
            // Grass/fern (tall/short/dry).
            Arc::new(TallGrassPatchFeature::new(d.clone())),
            Arc::new(TallGrassGenerateFeature::new(d.clone())),
            Arc::new(TallFernPatchFeature::new(d.clone())),
            Arc::new(TaigaGrassFeature::new(d.clone())),
            Arc::new(JungleGrassFeature::new(d.clone())),
            Arc::new(ScatterDryGrassFeature::new(d.clone())),
            Arc::new(BushFeature::new(d.clone())),
            // Mushroom/dead-bush/sweet-berry.
            Arc::new(ScatterBrownMushroomFeature::new(d.clone())),
            Arc::new(ScatterRedMushroomFeature::new(d.clone())),
            Arc::new(DeadBushFeature::new(d.clone())),
            Arc::new(ScatterSweetBerryBushFeature::new(d.clone())),
            // Single-point decorations.
            Arc::new(DesertCactusFeature::new(d.clone())),
            Arc::new(PumpkinGenerateFeature::new(d.clone())),
            Arc::new(WaterlilyFeature::new(d.clone())),
            Arc::new(ReedsFeature::new(d.clone())),
        ];
        stage.features.extend(decoration_features);
        stage
    }
}

impl GenerateStage for NormalChunkFeatureStage {
    /// Applies each feature in order, flushes buffered writes, and marks the chunk populated.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        // Buffer every feature's block writes into the shared root; the chunk
        // blocks/heightmap keep terrain values meanwhile so later features do
        // not stack trees onto an earlier tree's canopy.
        for feature in &self.features {
            feature.apply(ctx);
            if ctx.spillover_overflowed() {
                return;
            }
        }

        // Flush all buffered writes at once (chunk blocks + heightmap update).
        ctx.apply_root_to_chunk();

        ctx.chunk.set_state(ChunkState::Populated);
    }

    /// Stage name used for chain lookup.
    fn name(&self) -> &'static str {
        "feature"
    }
}

// ---------------------------------------------------------------------------
// Finished marker stage after population
// ---------------------------------------------------------------------------

/// Marks the chunk state as `Finished`.
///
/// There is no lighting computation here; the stage directly marks finished.
pub struct FinishedStage;

impl FinishedStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FinishedStage {
    fn default() -> Self {
        Self::new()
    }
}

impl GenerateStage for FinishedStage {
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        ctx.chunk.set_state(ChunkState::Finished);
    }

    fn name(&self) -> &'static str {
        "finished"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::chunk::{ChunkState, WorldgenChunk};
    use crate::worldgen::holder::normal::NormalObjectHolder;
    use crate::worldgen::material::MaterialBlocks;
    use crate::worldgen::random::Xoroshiro128;
    use sc_world::chunk::{BlockRuntimeId, ChunkPosition};

    fn test_ore_table() -> OreBlockTable {
        let id = |n: u32| BlockRuntimeId(n);
        OreBlockTable {
            stone: id(1),
            deepslate: id(2),
            coal_ore: id(10),
            iron_ore: id(11),
            copper_ore: id(12),
            gold_ore: id(13),
            redstone_ore: id(14),
            diamond_ore: id(15),
            lapis_ore: id(16),
            emerald_ore: id(17),
            deepslate_coal_ore: id(20),
            deepslate_iron_ore: id(21),
            deepslate_copper_ore: id(22),
            deepslate_gold_ore: id(23),
            deepslate_redstone_ore: id(24),
            deepslate_diamond_ore: id(25),
            deepslate_lapis_ore: id(26),
            deepslate_emerald_ore: id(27),
            dirt: id(30),
            gravel: id(31),
            andesite: id(32),
            diorite: id(33),
            granite: id(34),
            tuff: id(35),
            infested_stone: id(40),
            infested_deepslate: id(41),
        }
    }

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
    fn feature_stage_sets_populated() {
        let stage = NormalChunkFeatureStage::new(test_ore_table());
        let holder = NormalObjectHolder::new(Xoroshiro128::new(42), test_material_blocks());
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);
        stage.apply(&mut ctx);
        assert_eq!(ctx.chunk.state(), ChunkState::Populated);
    }

    #[test]
    fn feature_stage_name() {
        let stage = NormalChunkFeatureStage::new(test_ore_table());
        assert_eq!(stage.name(), "feature");
    }

    #[test]
    fn feature_stage_has_ore_features() {
        let stage = NormalChunkFeatureStage::new(test_ore_table());
        assert!(stage.features.len() > 20, "should have 20+ ore features");
    }

    #[test]
    fn finished_stage_sets_finished() {
        let holder = NormalObjectHolder::new(Xoroshiro128::new(42), test_material_blocks());
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        let mut ctx = ChunkGenerateContext::new(&mut wc, &holder, 42, -64, 319);
        FinishedStage.apply(&mut ctx);
        assert_eq!(ctx.chunk.state(), ChunkState::Finished);
    }
}
