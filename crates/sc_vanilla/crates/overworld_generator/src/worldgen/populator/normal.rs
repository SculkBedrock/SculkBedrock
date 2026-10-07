//! Code-generated structure populators (subset).
//!
//! | Struct | Contents |
//! |---|---|
//! | [`DesertWellPopulator`] | Desert-well structure populator |
//! | [`SwampHutPopulator`] | Swamp-hut structure populator |
//!
//! Not covered (they need the NBT template loader for registry structures):
//! Igloo / Fossil / DesertPyramid / JungleTemple / Mineshaft / OceanMonument /
//! OceanRuin / PillagerOutpost / Shipwreck / Stronghold / TrailRuins /
//! TrialChambers / Village / WoodlandMansion / AncientCity / RuinedPortal.

use std::sync::Arc;

use crate::worldgen::context::{BlockManager, ChunkGenerateContext};
use crate::worldgen::feature::legacy_tree::biome_id_matches_tag;
use crate::worldgen::populator::placement::{PlacementSettings, StructurePlacement};
use crate::worldgen::populator::structures::{
    ObjectDesertWell, ObjectSwampHut, StructureBlockTable, StructureHelper,
};
use crate::worldgen::populator::{should_generate_structures, Populator};
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};
use crate::worldgen::stages::chunk_hash;
use crate::worldgen::stages::terrain::SEA_LEVEL;

// ---------------------------------------------------------------------------
// DesertWellPopulator: desert-well structure populator
// ---------------------------------------------------------------------------

/// Desert-well structure populator.
pub struct DesertWellPopulator {
    table: Arc<StructureBlockTable>,
}

impl DesertWellPopulator {
    pub fn new(table: Arc<StructureBlockTable>) -> Self {
        Self { table }
    }
}

impl Populator for DesertWellPopulator {
    /// Applies the populator: picks a random well center and generates the well on success.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        // Skip when structures are disabled.
        if !should_generate_structures(ctx) {
            return;
        }

        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        // Per-chunk RNG seeded from the level seed and chunk position.
        let mut random = Xoroshiro128::new(0);
        random.set_seed(ctx.level_seed ^ chunk_hash(chunk_x, chunk_z));
        // Random well center within the chunk.
        let x = (chunk_x << 4) + random.next_bounded_int(15);
        let z = (chunk_z << 4) + random.next_bounded_int(15);
        // Surface height at the well center.
        let y = ctx.chunk.height_map((x & 15) as u8, (z & 15) as u8);

        // Placement check (biome/rarity/surface) then generate.
        let chunk_ref: &crate::worldgen::chunk::WorldgenChunk = ctx.chunk;
        if ObjectDesertWell.can_generate_at(chunk_ref, &self.table, ctx.level_seed, x, y, z) {
            let mut manager = BlockManager::new();
            ObjectDesertWell.generate(&mut manager, &self.table, x, y, z);
            // Merge the generated blocks into the root buffer.
            ctx.queue_object(manager.into_places());
        }
    }

    /// Populator name.
    fn name(&self) -> &'static str {
        "normal_desert_well"
    }
}

// ---------------------------------------------------------------------------
// SwampHutPopulator: swamp-hut structure populator
// ---------------------------------------------------------------------------

/// Swamp-hut structure populator.
pub struct SwampHutPopulator {
    table: Arc<StructureBlockTable>,
    /// Placement grid for swamp huts.
    placement: StructurePlacement,
}

impl SwampHutPopulator {
    pub fn new(table: Arc<StructureBlockTable>) -> Self {
        let placement = StructurePlacement::new(PlacementSettings {
            salt: 14357620,
            min_distance: 8,
            max_distance: 32,
            // Swamp-tagged biomes only.
            is_biome_valid: Box::new(|biome| biome_id_matches_tag("swamp", biome)),
            ..Default::default()
        });
        Self { table, placement }
    }
}

impl Populator for SwampHutPopulator {
    /// Applies the populator: placement check, then a random hut origin with surface height.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        if !should_generate_structures(ctx) {
            return;
        }

        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        // Center-column biome sample.
        let biome = ctx.chunk.biome_id(7, SEA_LEVEL, 7);
        // Region-grid placement check.
        let mut random = Xoroshiro128::new(0);
        if !self
            .placement
            .can_generate(ctx.level_seed, &mut random, chunk_x, chunk_z, biome)
        {
            return;
        }

        // Per-chunk RNG seeded from the level seed and chunk position.
        random.set_seed(ctx.level_seed ^ chunk_hash(chunk_x, chunk_z));
        // Random hut origin within the chunk.
        let x = (chunk_x << 4) + random.next_bounded_int(15);
        let z = (chunk_z << 4) + random.next_bounded_int(15);
        let y = ctx.chunk.height_map((x & 15) as u8, (z & 15) as u8);

        // The helper needs chunk reads, so borrow read-only, then queue the result.
        let chunk_ref: &crate::worldgen::chunk::WorldgenChunk = ctx.chunk;
        let mut helper = StructureHelper::new(chunk_ref, ctx.level_seed, (x, y, z));
        ObjectSwampHut.generate(&mut helper, &self.table);
        ctx.queue_object(helper.into_places());
    }

    /// Populator name.
    fn name(&self) -> &'static str {
        "normal_swamp_hut"
    }
}
