//! Structure subset that code can generate.
//!
//! | Rust item | Upstream source |
//! |---|---|
//! | [`StructureBlockTable`] | (block default-state constants) |
//! | [`StructureHelper`] | `object/structures/StructureHelper.java` |
//! | [`ObjectDesertWell`] | `object/structures/ObjectDesertWell.java` |
//! | [`ObjectSwampHut`] | `object/structures/ObjectSwampHut.java` |
//!
//! Port notes:
//! - Upstream `BlockState` constants become the runtime-id table
//!   [`StructureBlockTable`] (`from_core_palette` reads the global
//!   block dictionary, falling back to air plus warn).
//! - `StructureHelper extends BlockManager` (origin offset + fill /
//!   setBlockDownward) becomes composition: [`BlockManager`] plus origin.
//! - `canBeReplaced()` (the downward-drill check) approximates as
//!   air/water (swamp piles drill to the lakebed).
//! - Block-entity post-processing (pot contents, brewing stands, chest loot)
//!   is skipped (`addHook` is not ported).

use std::collections::{HashMap, HashSet};

use sc_log::t_log;
use sc_world::block_dictionary::BlockStateDictionary;
use sc_world::chunk::BlockRuntimeId;

use crate::worldgen::chunk::WorldgenChunk;
use crate::worldgen::context::{BlockEntry, BlockManager};
use crate::worldgen::feature::legacy_tree::biome_id_matches_tag;
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};

// ---------------------------------------------------------------------------
// StructureBlockTable.
// ---------------------------------------------------------------------------

/// Block runtime-id table for structure generation.
pub struct StructureBlockTable {
    pub air: BlockRuntimeId,
    /// `BlockSandstone.PROPERTIES.getDefaultState()`.
    pub sandstone: BlockRuntimeId,
    /// `BlockWater.PROPERTIES.getDefaultState()`(liquid_depth=0).
    pub water: BlockRuntimeId,
    /// Sandstone slab default state (bottom half).
    pub sandstone_slab: BlockRuntimeId,
    /// `BlockSprucePlanks.PROPERTIES.getDefaultState()`.
    pub spruce_planks: BlockRuntimeId,
    /// `BlockFlowerPot.PROPERTIES.getDefaultState()`.
    pub flower_pot: BlockRuntimeId,
    /// `BlockOakFence.PROPERTIES.getDefaultState()`.
    pub oak_fence: BlockRuntimeId,
    /// `BlockOakLog.PROPERTIES.getDefaultState()`(pillar_axis=y).
    pub oak_log: BlockRuntimeId,
    /// `BlockCauldron.PROPERTIES.getDefaultState()`.
    pub cauldron: BlockRuntimeId,
    /// `BlockCraftingTable.PROPERTIES.getDefaultState()`.
    pub crafting_table: BlockRuntimeId,
    /// `BlockSpruceStairs`(weirdo_direction 0-3,upside_down_bit=false).
    /// Index is the direction value (N=2 / E=1 / S=3 / W=0).
    pub spruce_stairs: [BlockRuntimeId; 4],
    /// Sand approximation: the full state sets of sand and red sand
    /// (desert-well surface check).
    pub sand_set: HashSet<BlockRuntimeId>,
    /// Liquid approximation: the full water state set
    /// (downward-drill check).
    pub water_set: HashSet<BlockRuntimeId>,
}

impl StructureBlockTable {
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
                log::warn!("{}", t_log!("console.worldgen.table_missing", table = "structure", name = format!("{name:?}")));
                air
            }
        };
        let lookup_states = |name: &str, pairs: &[(&str, &str)]| {
            // Exact states prefer the snapshot (always at hand here) over the
            // global dictionary (dlopened plugin copies may hold an empty one).
            if let Some(hash) = snapshot.and_then(|s| s.find_state_hash(name, pairs)) {
                return BlockRuntimeId(hash);
            }
            match dictionary.find_hash_by_states(name, pairs) {
                Some(hash) => BlockRuntimeId(hash),
                None => {
                    log::warn!("{}", t_log!("console.worldgen.table_state", table = "structure", name = name, detail = format!("{pairs:?}")));
                    lookup(name)
                }
            }
        };
        // Name list to full state sets (snapshot first for the same
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

        let mut spruce_stairs = [air; 4];
        for (dir, state) in spruce_stairs.iter_mut().enumerate() {
            *state = lookup_states(
                "minecraft:spruce_stairs",
                &[
                    ("weirdo_direction", &dir.to_string()),
                    ("upside_down_bit", "false"),
                ],
            );
        }

        let mut sand_set = states_of("minecraft:sand");
        sand_set.extend(states_of("minecraft:red_sand"));

        Self {
            air,
            sandstone: lookup("minecraft:sandstone"),
            water: lookup_states("minecraft:water", &[("liquid_depth", "0")]),
            sandstone_slab: lookup("minecraft:sandstone_slab"),
            spruce_planks: lookup("minecraft:spruce_planks"),
            flower_pot: lookup("minecraft:flower_pot"),
            oak_fence: lookup("minecraft:oak_fence"),
            oak_log: lookup_states("minecraft:oak_log", &[("pillar_axis", "y")]),
            cauldron: lookup("minecraft:cauldron"),
            crafting_table: lookup("minecraft:crafting_table"),
            spruce_stairs,
            sand_set,
            water_set: states_of("minecraft:water"),
        }
    }
}

// ---------------------------------------------------------------------------
// StructureHelper.
// ---------------------------------------------------------------------------

/// Relative-coordinate structure builder rooted at origin.
///
/// Composition instead of inheritance: holds [`BlockManager`] (buffer plus
/// chunk read fallback) and the origin offset; all writes apply the offset first.
/// Composition instead of inheritance: holds [`BlockManager`] (buffer plus
pub struct StructureHelper<'a> {
    manager: BlockManager<'a>,
    /// Java: `final BlockVector3 origen`.
    origin: (i32, i32, i32),
}

impl<'a> StructureHelper<'a> {
    /// Java: `StructureHelper(Level level, BlockVector3 origen)`——
    /// Chunk read fallback plus world seed.
    pub fn new(chunk: &'a WorldgenChunk, level_seed: i64, origin: (i32, i32, i32)) -> Self {
        Self {
            manager: BlockManager::with_chunk_and_seed(chunk, level_seed),
            origin,
        }
    }

    /// Java: `void fill(BlockVector3 min, BlockVector3 max, BlockState state)`(L35-38).
    pub fn fill_uniform(
        &mut self,
        min: (i32, i32, i32),
        max: (i32, i32, i32),
        state: BlockRuntimeId,
    ) {
        self.fill(min, max, state, state);
    }

    /// Java: `void fill(min, max, BlockState outer, BlockState inner)`(L47-61).
    ///
    /// Shell faces fill outer, interior fills inner.
    pub fn fill(
        &mut self,
        min: (i32, i32, i32),
        max: (i32, i32, i32),
        outer: BlockRuntimeId,
        inner: BlockRuntimeId,
    ) {
        for y in min.1..=max.1 {
            for x in min.0..=max.0 {
                for z in min.2..=max.2 {
                    let state = if x != min.0
                        && x != max.0
                        && z != min.2
                        && z != max.2
                        && y != min.1
                        && y != max.1
                    {
                        inner
                    } else {
                        outer
                    };
                    self.set_block_state_at(x, y, z, state);
                }
            }
        }
    }

    /// Java: `void setBlockStateAt(int x, int y, int z, BlockState state)`(L117-120)——
    /// Writes go through the origin offset (layer 0); structure_void filtering is unused.
    pub fn set_block_state_at(&mut self, x: i32, y: i32, z: i32, state: BlockRuntimeId) {
        self.manager.set_block_state_at(
            self.origin.0 + x,
            self.origin.1 + y,
            self.origin.2 + z,
            0,
            state,
        );
    }

    /// Java: `void setBlockDownward(BlockVector3 pos, BlockState state)`(L103-109)——
    /// Fills downward from pos.y while cells are replaceable (air/water approx).
    pub fn set_block_downward(
        &mut self,
        pos: (i32, i32, i32),
        state: BlockRuntimeId,
        table: &StructureBlockTable,
    ) {
        let (x, mut y, z) = (
            self.origin.0 + pos.0,
            self.origin.1 + pos.1,
            self.origin.2 + pos.2,
        );
        while y > 1 {
            let block = self.manager.get_block_if_cached_or_loaded(x, y, z);
            if !(block == table.air || table.water_set.contains(&block)) {
                break;
            }
            self.manager.set_block_state_at(x, y, z, 0, state);
            y -= 1;
        }
    }

    /// Consume the helper, returning buffered placements for the queue.
    pub fn into_places(self) -> HashMap<u64, BlockEntry> {
        self.manager.into_places()
    }
}

// ---------------------------------------------------------------------------
// ObjectDesertWell.
// ---------------------------------------------------------------------------

/// Desert well generator.
///
/// [`generate`] builds a static shape; [`can_generate_at`] checks
/// biome, rarity, and surface.
pub struct ObjectDesertWell;

impl ObjectDesertWell {
    /// Java: `boolean generate(BlockManager level, RandomSourceProvider rand,
    /// position is the well center (absolute coordinates).
    pub fn generate(
        &self,
        manager: &mut BlockManager<'_>,
        table: &StructureBlockTable,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        // Base plate (dy -1..0, 5x5 sandstone).
        for dy in -1..=0 {
            for dx in -2..=2 {
                for dz in -2..=2 {
                    manager.set_block_state_at(x + dx, y + dy, z + dz, 0, table.sandstone);
                }
            }
        }
        // Center water cross.
        manager.set_block_state_at(x, y, z, 0, table.water);
        manager.set_block_state_at(x - 1, y, z, 0, table.water);
        manager.set_block_state_at(x + 1, y, z, 0, table.water);
        manager.set_block_state_at(x, y, z - 1, 0, table.water);
        manager.set_block_state_at(x, y, z + 1, 0, table.water);
        // y+1 outer sandstone ring.
        for dx in -2..=2 {
            for dz in -2..=2 {
                if dx == -2 || dx == 2 || dz == -2 || dz == 2 {
                    manager.set_block_state_at(x + dx, y + 1, z + dz, 0, table.sandstone);
                }
            }
        }
        // Four edge-midpoint slabs (overwriting the ring).
        manager.set_block_state_at(x + 2, y + 1, z, 0, table.sandstone_slab);
        manager.set_block_state_at(x - 2, y + 1, z, 0, table.sandstone_slab);
        manager.set_block_state_at(x, y + 1, z + 2, 0, table.sandstone_slab);
        manager.set_block_state_at(x, y + 1, z - 2, 0, table.sandstone_slab);
        // Top 3x3 cap (sandstone center, slabs around).
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 {
                    manager.set_block_state_at(x + dx, y + 4, z + dz, 0, table.sandstone);
                } else {
                    manager.set_block_state_at(x + dx, y + 4, z + dz, 0, table.sandstone_slab);
                }
            }
        }
        // Four corner pillars (y+1..3).
        for dy in 1..=3 {
            manager.set_block_state_at(x - 1, y + dy, z - 1, 0, table.sandstone);
            manager.set_block_state_at(x - 1, y + dy, z + 1, 0, table.sandstone);
            manager.set_block_state_at(x + 1, y + dy, z - 1, 0, table.sandstone);
            manager.set_block_state_at(x + 1, y + dy, z + 1, 0, table.sandstone);
        }
        true
    }

    /// Java: `boolean canGenerateAt(Location location)`(L80-109).
    ///
    /// Check order (short-circuit): desert biome, 1/500 rarity,
    /// y <= 128, sand surface, no overhang under the 5x5.
    pub fn can_generate_at(
        &self,
        chunk: &WorldgenChunk,
        table: &StructureBlockTable,
        level_seed: i64,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        // Java L85: random.setSeed(level.getSeed() ^ (x + y + z))
        let mut random = Xoroshiro128::new(level_seed ^ (x.wrapping_add(y).wrapping_add(z)) as i64);
        let (lx, lz) = ((x & 15) as u8, (z & 15) as u8);

        // Desert tag plus 1/500 rarity.
        let biome = chunk.biome_id(lx, y, lz);
        if !biome_id_matches_tag("desert", biome) || random.next_bounded_int(500) != 0 {
            return false;
        }

        // Height cap.
        if y > 128 {
            return false;
        }

        // Sand surface (tag approximation).
        if !table.sand_set.contains(&chunk.block_state(lx, y, lz, 0)) {
            return false;
        }

        // 5x5 under-check: air at both y-1 and y-2 means overhang.
        for dx in -2..=2 {
            for dz in -2..=2 {
                let bx = ((x + dx) & 15) as u8;
                let bz = ((z + dz) & 15) as u8;
                let b1 = chunk.block_state(bx, y - 1, bz, 0);
                let b2 = chunk.block_state(bx, y - 2, bz, 0);
                if b1 == table.air && b2 == table.air {
                    return false;
                }
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// ObjectSwampHut.
// ---------------------------------------------------------------------------

/// Swamp hut generator.
///
/// 7x9x5 witch hut (relative 0-6 x 0-8 x 0-4); chest loot and pot contents
/// (`addHook`) are not ported (they need block-entity infrastructure).
pub struct ObjectSwampHut;

impl ObjectSwampHut {
    /// Java: `boolean generate(BlockManager object, RandomSourceProvider rand,
    /// position is the origin (absolute coordinates).
    pub fn generate(&self, builder: &mut StructureHelper<'_>, table: &StructureBlockTable) -> bool {
        let (planks, air, fence, log) = (
            table.spruce_planks,
            table.air,
            table.oak_fence,
            table.oak_log,
        );
        let (pot, cauldron, craft) = (table.flower_pot, table.cauldron, table.crafting_table);
        let [stairs_w, stairs_e, stairs_n, stairs_s] = table.spruce_stairs;

        // Hut body (spruce shell, air inside) plus doorstep.
        builder.fill((1, 1, 2), (5, 4, 7), planks, air); // hut body
        builder.fill((1, 1, 1), (5, 1, 1), planks, air); // hut steps
        builder.fill((2, 1, 0), (4, 1, 0), planks, air); // hut steps
                                                         // Door and windows.
        builder.fill_uniform((4, 2, 2), (4, 3, 2), air); // hut door
        builder.fill_uniform((5, 3, 4), (5, 3, 5), air); // left window
        builder.set_block_state_at(1, 3, 4, air);

        // Flower pots (content hooks not ported).
        builder.set_block_state_at(1, 3, 5, pot);

        // Fence rails.
        builder.set_block_state_at(2, 3, 2, fence);
        builder.set_block_state_at(3, 3, 7, fence);

        // Roof stairs (N=2 / E=1 / S=3 / W=0).
        builder.fill_uniform((0, 4, 1), (6, 4, 1), stairs_n); // N
        builder.fill_uniform((6, 4, 2), (6, 4, 7), stairs_e); // E
        builder.fill_uniform((0, 4, 8), (6, 4, 8), stairs_s); // S
        builder.fill_uniform((0, 4, 2), (0, 4, 7), stairs_w); // W

        // Four corner pillars (y 0-3).
        builder.fill_uniform((1, 0, 2), (1, 3, 2), log);
        builder.fill_uniform((5, 0, 2), (5, 3, 2), log);
        builder.fill_uniform((1, 0, 7), (1, 3, 7), log);
        builder.fill_uniform((5, 0, 7), (5, 3, 7), log);

        // Door rails.
        builder.set_block_state_at(1, 2, 1, fence);
        builder.set_block_state_at(5, 2, 1, fence);

        // Cauldron and workbench.
        builder.set_block_state_at(4, 2, 6, cauldron);
        builder.set_block_state_at(3, 2, 6, craft);

        // Corner piles drilling down (to the lakebed over water).
        builder.set_block_downward((1, -1, 2), log, table);
        builder.set_block_downward((5, -1, 2), log, table);
        builder.set_block_downward((1, -1, 7), log, table);
        builder.set_block_downward((5, -1, 7), log, table);
        true
    }
}
