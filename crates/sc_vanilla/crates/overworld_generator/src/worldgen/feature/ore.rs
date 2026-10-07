//! Port of `feature/ore/OreGeneratorFeature.java` plus all concrete ore features.
//!
//! Port notes:
//! - The ~30 `XxxOreGenerationFeature` subclasses collapse into one [`OreFeature`]
//!   struct plus an [`OreSpec`] parameter table (data-driven, matching the getter
//!   overrides).
//! - The `protected BlockState STONE/DEEPSLATE/NETHERRACK` constants are injected
//!   via [`OreBlockTable`] instead (no global dictionary dependency).
//! - The two-layer `BlockManager` buffer (per-cluster `object` plus per-chunk
//!   `manager`) becomes a per-apply `BlockManager`, flushed by `apply_to_chunk`.
//! - Cross-chunk `level.getBlockStateAt(x, y, z)` reads become current-chunk-only
//!   (`WorldgenChunk::block_state`); cross-chunk cluster edges clip (full
//!   cross-chunk routing needs GenSession).
//! - The per-use random source constructs per apply (see
//!   [`crate::worldgen::feature::GenerateFeature::make_random`]).
//! - Operator-precedence trap: `chunkHash ^ level.getSeed() + name().hashCode()`
//!   parses as `chunkHash ^ (levelSeed + nameHash)` (`+` binds tighter than `^`).

use crate::blocks_table::OreBlockTable;
use crate::worldgen::context::{BlockManager, ChunkGenerateContext};
use crate::worldgen::feature::GenerateFeature;
use crate::worldgen::math::{floor, random_range_triangle};
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};
use crate::worldgen::stages::chunk_hash;
use sc_world::chunk::BlockRuntimeId;

// ---------------------------------------------------------------------------
// ConcentrationType enum.
// ---------------------------------------------------------------------------

/// Java: `OreGeneratorFeature.ConcentrationType { UNIFORM, TRIANGLE }`(L148-151).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConcentrationType {
    /// UNIFORM: even distribution across the Y range.
    Uniform,
    /// TRIANGLE: triangular distribution via `random_range_triangle`.
    Triangle,
}

// ---------------------------------------------------------------------------
// OreState (two getState strategies).
// ---------------------------------------------------------------------------

/// Ore block state resolution strategy.
///
/// Two modes:
/// 1. [`Self::StoneDeepslate`]: pick by the original block (stone goes ore,
///    deepslate goes deepslate_ore).
/// 2. [`Self::Single`]: always one block.
///    Used for dirt/gravel/andesite/diorite/granite/tuff.
#[derive(Clone, Copy, Debug)]
pub enum OreState {
    /// Java: `switch(original.getIdentifier()) { case STONE -> TYPE_STONE; case DEEPSLATE -> TYPE_DEEPSLATE; }`
    StoneDeepslate {
        stone: BlockRuntimeId,
        deepslate: BlockRuntimeId,
    },
    /// Always returns the single state.
    Single(BlockRuntimeId),
}

// ---------------------------------------------------------------------------
// OreReplaceTarget (two canBeReplaced strategies).
// ---------------------------------------------------------------------------

/// Ore replacement targets.
///
/// Two modes:
/// 1. [`Self::StoneAndDeepslate`]: the default.
/// 2. [`Self::StoneOnly`]: upper ores, replacing stone only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OreReplaceTarget {
    /// Java: `state == STONE || state == DEEPSLATE || state == NETHERRACK`
    /// (No netherrack in the overworld: stone/deepslate only.)
    StoneAndDeepslate,
    /// Java: `AbstractOreUpperGeneratorFeature.canBeReplaced` → `state == STONE`.
    StoneOnly,
}

// ---------------------------------------------------------------------------
// OreSpec (one ore feature parameter table).
// ---------------------------------------------------------------------------

/// Complete parameter table for one ore feature.
///
/// Matches each upstream ore feature getter overrides.
/// `BlockRuntimeId`s resolve from [`OreBlockTable`] at construction
/// (deferred to `OreFeature::new`).
#[derive(Clone, Debug)]
pub struct OreSpec {
    pub name: &'static str,
    pub state: OreState,
    pub cluster_count: i32,
    pub cluster_size: i32,
    pub min_height: i32,
    pub max_height: i32,
    pub concentration: ConcentrationType,
    pub skip_air: f32,
    pub is_rare: bool,
    pub replace_target: OreReplaceTarget,
}

// ---------------------------------------------------------------------------
// OreFeature (one struct for all ore generator subclasses).
// ---------------------------------------------------------------------------

/// Unified implementation of the ore generator plus all subclasses.
///
/// Holds [`OreBlockTable`] (block id injection) plus [`OreSpec`];
/// `apply` implements the apply plus spawn logic directly.
pub struct OreFeature {
    table: OreBlockTable,
    spec: OreSpec,
}

impl OreFeature {
    pub fn new(table: OreBlockTable, spec: OreSpec) -> Self {
        Self { table, spec }
    }

    /// Java: `BlockState getState(BlockState original)`(L23).
    fn get_state(&self, original: BlockRuntimeId) -> BlockRuntimeId {
        match self.spec.state {
            OreState::StoneDeepslate { stone, deepslate } => {
                if original == self.table.stone {
                    stone
                } else if original == self.table.deepslate {
                    deepslate
                } else {
                    original
                }
            }
            OreState::Single(state) => state,
        }
    }

    /// Java: `boolean canBeReplaced(BlockState state)`(L41-43 / AbstractOreUpper L8-10).
    fn can_be_replaced(&self, state: BlockRuntimeId) -> bool {
        match self.spec.replace_target {
            OreReplaceTarget::StoneAndDeepslate => {
                state == self.table.stone || state == self.table.deepslate
            }
            OreReplaceTarget::StoneOnly => state == self.table.stone,
        }
    }

    /// Java: `protected void spawn(BlockManager, RandomSourceProvider, int x, int y, int z)`(L97-146).
    ///
    /// Ellipsoid cluster generation.
    fn spawn(&self, level: &mut BlockManager, rand: &mut Xoroshiro128, x: i32, y: i32, z: i32) {
        let cluster_size = self.spec.cluster_size;
        let pi_scaled = rand.next_float() * std::f32::consts::PI;
        let scale_max_x = (x as f64 + 8.0) + (pi_scaled.sin() as f64 * cluster_size as f64 / 8.0);
        let scale_min_x = (x as f64 + 8.0) - (pi_scaled.sin() as f64 * cluster_size as f64 / 8.0);
        let scale_max_z = (z as f64 + 8.0) + (pi_scaled.cos() as f64 * cluster_size as f64 / 8.0);
        let scale_min_z = (z as f64 + 8.0) - (pi_scaled.cos() as f64 * cluster_size as f64 / 8.0);
        let scale_max_y = y as f64 + rand.next_bounded_int(3) as f64 - 2.0;
        let scale_min_y = y as f64 + rand.next_bounded_int(3) as f64 - 2.0;

        for i in 0..cluster_size {
            let size_incr = i as f32 / cluster_size as f32;
            let scale_x = scale_max_x + (scale_min_x - scale_max_x) * size_incr as f64;
            let scale_y = scale_max_y + (scale_min_y - scale_max_y) * size_incr as f64;
            let scale_z = scale_max_z + (scale_min_z - scale_max_z) * size_incr as f64;
            let rand_size_offset = rand.next_double() * cluster_size as f64 / 16.0;
            let rand_vec1 =
                (std::f32::consts::PI * size_incr).sin() as f64 * rand_size_offset + 1.0;
            let rand_vec2 =
                (std::f32::consts::PI * size_incr).sin() as f64 * rand_size_offset + 1.0;
            let min_x = floor(scale_x - rand_vec1 / 2.0);
            let min_y = floor(scale_y - rand_vec2 / 2.0);
            let min_z = floor(scale_z - rand_vec1 / 2.0);
            let max_x = floor(scale_x + rand_vec1 / 2.0);
            let max_y = floor(scale_y + rand_vec2 / 2.0);
            let max_z = floor(scale_z + rand_vec1 / 2.0);

            for x_seg in min_x..=max_x {
                let x_val = (x_seg as f64 + 0.5 - scale_x) / (rand_vec1 / 2.0);
                if x_val * x_val < 1.0 {
                    for y_seg in min_y..=max_y {
                        // Java L127: if(ySeg < -64) continue;
                        if y_seg < -64 {
                            continue;
                        }
                        let y_val = (y_seg as f64 + 0.5 - scale_y) / (rand_vec2 / 2.0);
                        if x_val * x_val + y_val * y_val < 1.0 {
                            for z_seg in min_z..=max_z {
                                let z_val = (z_seg as f64 + 0.5 - scale_z) / (rand_vec1 / 2.0);
                                if x_val * x_val + y_val * y_val + z_val * z_val < 1.0 {
                                    let original = level.get_block_at(x_seg, y_seg, z_seg);
                                    if self.can_be_replaced(original) {
                                        level.set_block_state_at(
                                            x_seg,
                                            y_seg,
                                            z_seg,
                                            0,
                                            self.get_state(original),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl GenerateFeature for OreFeature {
    /// Java: `final void apply(ChunkGenerateContext context)`(L46-95).
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let level_seed = ctx.level_seed;
        let min_y = ctx.min_y;
        let max_y = ctx.max_y;
        let sx = chunk_x << 4;
        let sz = chunk_z << 4;

        let mut random = self.make_random(level_seed, chunk_x, chunk_z);

        // Java L55: isRare ? (random.nextInt(getClusterCount()) == 0 ? 1 : 0) : getClusterCount()
        let loop_count = if self.spec.is_rare {
            if random.next_int_max(self.spec.cluster_count) == 0 {
                1
            } else {
                0
            }
        } else {
            self.spec.cluster_count
        };

        // Per-chunk accumulating BlockManager.
        let mut manager = BlockManager::new();

        for _ in 0..loop_count {
            // Per-cluster BlockManager.
            let mut object = BlockManager::new();

            let effective_max_y = self.spec.max_height.min(max_y);
            let effective_min_y = self.spec.min_height.max(min_y);

            let x = sx + random.next_int_max(15);
            let z = sz + random.next_int_max(15);
            let y = match self.spec.concentration {
                ConcentrationType::Triangle => {
                    random_range_triangle(&mut random, effective_min_y, effective_max_y)
                }
                ConcentrationType::Uniform => {
                    effective_min_y
                        + random.next_bounded_int((effective_max_y - effective_min_y) + 1)
                }
            };

            // Reads land in the current chunk (cross-chunk reads miss).
            let local_x = (x & 0xF) as u8;
            let local_z = (z & 0xF) as u8;
            let original = if (x >> 4) == chunk_x && (z >> 4) == chunk_z {
                ctx.chunk.block_state(local_x, y, local_z, 0)
            } else {
                // Cross-chunk: stone placeholder (clipped to this chunk on write).
                self.table.stone
            };

            if !self.can_be_replaced(original) {
                continue;
            }

            if self.spec.cluster_size == 1 {
                object.set_block_state_at(x, y, z, 0, self.get_state(original));
            } else {
                // Java L73: spawn(object, random.setSeed(...), x, y, z)
                let spawn_seed = level_seed
                    ^ chunk_hash(chunk_x, chunk_z)
                    ^ (x as i64).wrapping_add(y as i64 + z as i64);
                let mut spawn_rand = Xoroshiro128::new(spawn_seed);
                self.spawn(&mut object, &mut spawn_rand, x, y, z);
                // Java L74: this.random.setSeed(level.getSeed() ^ Level.chunkHash(chunkX, chunkZ))
                random = Xoroshiro128::new(level_seed ^ chunk_hash(chunk_x, chunk_z));
            }

            // skipAir logic.
            let mut skip = false;
            if self.spec.skip_air != 0.0 {
                let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
                let has_air = object.places().values().any(|e| e.block == air);
                if has_air {
                    skip = random.next_float() < self.spec.skip_air;
                }
            }

            if !skip {
                // Merge object blocks into the manager.
                manager.merge(object);
            }
        }

        // Java L94: queueObject(chunk, manager) → root.merge(manager)——
        // Buffer until stage end; the submit phase clips cross-chunk blocks.
        let places = manager.into_places();
        ctx.queue_object(places);
    }

    fn name(&self) -> &'static str {
        self.spec.name
    }
}

// ---------------------------------------------------------------------------
// Ore spec builders (resolve BlockRuntimeIds from OreBlockTable).
// ---------------------------------------------------------------------------

/// Build every overworld ore spec from [`OreBlockTable`].
///
/// Matches the upstream feature-registry ore set, hardcoded for the
/// overworld underground ores.
pub fn build_overworld_ore_specs(table: &OreBlockTable) -> Vec<OreSpec> {
    use ConcentrationType::*;
    use OreReplaceTarget::*;
    use OreState::*;

    let sd = |stone: BlockRuntimeId, deepslate: BlockRuntimeId| StoneDeepslate { stone, deepslate };
    let single = |b: BlockRuntimeId| Single(b);

    // Parameters mirror the upstream ore subclass getters.
    vec![
        // --- Coal ---
        OreSpec {
            name: "minecraft:overworld_underground_coal_ore_upper_feature",
            state: sd(table.coal_ore, table.deepslate_coal_ore),
            cluster_count: 30,
            cluster_size: 17,
            min_height: 136,
            max_height: 320,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_coal_ore_lower_feature",
            state: sd(table.coal_ore, table.deepslate_coal_ore),
            cluster_count: 20,
            cluster_size: 17, // 继承 upper
            min_height: 0,
            max_height: 192,
            concentration: Triangle,
            skip_air: 0.5,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:mountains_underground_coal_ore_feature",
            state: sd(table.coal_ore, table.deepslate_coal_ore),
            cluster_count: 20,
            cluster_size: 17,
            min_height: 128,
            max_height: 156,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        // --- Iron ---
        OreSpec {
            name: "minecraft:overworld_underground_iron_ore_upper_feature",
            state: sd(table.iron_ore, table.deepslate_iron_ore),
            cluster_count: 90,
            cluster_size: 10,
            min_height: 80,
            max_height: 384,
            concentration: Triangle,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_iron_ore_middle_feature",
            state: sd(table.iron_ore, table.deepslate_iron_ore),
            cluster_count: 10,
            cluster_size: 10,
            min_height: -24,
            max_height: 56,
            concentration: Triangle,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_iron_ore_small_feature",
            state: sd(table.iron_ore, table.deepslate_iron_ore),
            cluster_count: 10, // 继承 middle
            cluster_size: 4,
            min_height: -64,
            max_height: 72,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        // --- Copper ---
        OreSpec {
            name: "minecraft:overworld_underground_copper_ore_feature",
            state: sd(table.copper_ore, table.deepslate_copper_ore),
            cluster_count: 16,
            cluster_size: 10,
            min_height: -16,
            max_height: 112,
            concentration: Triangle,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:dripstone_caves_copper_ore_feature",
            state: sd(table.copper_ore, table.deepslate_copper_ore),
            cluster_count: 16, // 继承
            cluster_size: 20,
            min_height: -16,
            max_height: 112,
            concentration: Triangle,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        // --- Gold ---
        OreSpec {
            name: "minecraft:mesa_underground_gold_ore_feature",
            state: sd(table.gold_ore, table.deepslate_gold_ore),
            cluster_count: 50,
            cluster_size: 9,
            min_height: 32,
            max_height: 256,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_gold_ore_feature",
            state: sd(table.gold_ore, table.deepslate_gold_ore),
            cluster_count: 4,
            cluster_size: 9, // 继承 mesa
            min_height: -64,
            max_height: 32,
            concentration: Triangle,
            skip_air: 0.5,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_gold_ore_lower_feature",
            state: sd(table.gold_ore, table.deepslate_gold_ore),
            cluster_count: 2,
            cluster_size: 9, // 继承 mesa
            min_height: -64,
            max_height: -48,
            concentration: Uniform,
            skip_air: 0.5,
            is_rare: true,
            replace_target: StoneAndDeepslate,
        },
        // --- Redstone ---
        OreSpec {
            name: "minecraft:overworld_underground_redstone_ore_feature",
            state: sd(table.redstone_ore, table.deepslate_redstone_ore),
            cluster_count: 4,
            cluster_size: 8,
            min_height: -64,
            max_height: 15,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_redstone_ore_lower_feature",
            state: sd(table.redstone_ore, table.deepslate_redstone_ore),
            cluster_count: 8,
            cluster_size: 8, // 继承
            min_height: -64, // 继承
            max_height: -32,
            concentration: Triangle,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        // --- Diamond ---
        OreSpec {
            name: "minecraft:overworld_underground_diamond_ore_feature_square",
            state: sd(table.diamond_ore, table.deepslate_diamond_ore),
            cluster_count: 8,
            cluster_size: 2,
            min_height: -64,
            max_height: -4,
            concentration: Uniform,
            skip_air: 0.5,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_diamond_ore_feature",
            state: sd(table.diamond_ore, table.deepslate_diamond_ore),
            cluster_count: 4,
            cluster_size: 7,
            min_height: -64, // 继承 square
            max_height: 16,
            concentration: Triangle,
            skip_air: 0.5, // 继承 square
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_diamond_ore_large_feature",
            state: sd(table.diamond_ore, table.deepslate_diamond_ore),
            cluster_count: 9,
            cluster_size: 12,
            min_height: -64,         // 继承
            max_height: 16,          // 继承
            concentration: Triangle, // 继承
            skip_air: 0.7,
            is_rare: true,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_diamond_ore_buried_feature",
            state: sd(table.diamond_ore, table.deepslate_diamond_ore),
            cluster_count: 8,
            cluster_size: 4,
            min_height: -64,         // 继承
            max_height: 16,          // 继承
            concentration: Triangle, // 继承
            skip_air: 1.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        // --- Lapis ---
        OreSpec {
            name: "minecraft:overworld_underground_lapis_ore_buried_feature",
            state: sd(table.lapis_ore, table.deepslate_lapis_ore),
            cluster_count: 4,
            cluster_size: 7,
            min_height: -64,
            max_height: 64,
            concentration: Uniform,
            skip_air: 1.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        OreSpec {
            name: "minecraft:overworld_underground_lapis_ore_feature",
            state: sd(table.lapis_ore, table.deepslate_lapis_ore),
            cluster_count: 2,
            cluster_size: 7, // 继承 buried
            min_height: -32,
            max_height: 32,
            concentration: Triangle,
            skip_air: 1.0, // 继承 buried
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        // --- Emerald ---
        OreSpec {
            name: "minecraft:overworld_underground_emerald_ore_feature",
            state: sd(table.emerald_ore, table.deepslate_emerald_ore),
            cluster_count: 100,
            cluster_size: 3,
            min_height: -16,
            max_height: 420,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        // --- Infested blocks ---
        OreSpec {
            name: "minecraft:extreme_hills_after_surface_silverfish_feature",
            state: sd(table.infested_stone, table.infested_deepslate),
            cluster_count: 100,
            cluster_size: 3,
            min_height: -16,
            max_height: 420,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        // --- Dirt ---
        OreSpec {
            name: "minecraft:overworld_underground_dirt_feature",
            state: single(table.dirt),
            cluster_count: 7,
            cluster_size: 33,
            min_height: 0,
            max_height: 160,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
        // --- Gravel ---
        OreSpec {
            name: "minecraft:overworld_underground_gravel_ore_feature",
            state: single(table.gravel),
            cluster_count: 14,
            cluster_size: 33,
            min_height: -64,
            max_height: 320,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneOnly, // AbstractOreUpperGeneratorFeature
        },
        // --- Granite (upper) ---
        OreSpec {
            name: "minecraft:overworld_underground_granite_upper_feature",
            state: single(table.granite),
            cluster_count: 6,
            cluster_size: 64,
            min_height: 64,
            max_height: 128,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: true,
            replace_target: StoneOnly, // AbstractOreUpperGeneratorFeature
        },
        // --- Granite (lower) ---
        OreSpec {
            name: "minecraft:overworld_underground_granite_lower_feature",
            state: single(table.granite),
            cluster_count: 2,
            cluster_size: 64,
            min_height: 0,
            max_height: 60,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneOnly,
        },
        // --- Diorite (upper) ---
        OreSpec {
            name: "minecraft:overworld_underground_diorite_upper_feature",
            state: single(table.diorite),
            cluster_count: 6, // 继承 granite_upper
            cluster_size: 64,
            min_height: 64,
            max_height: 128,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: true,
            replace_target: StoneOnly,
        },
        // --- Diorite (lower) ---
        OreSpec {
            name: "minecraft:overworld_underground_diorite_lower_feature",
            state: single(table.diorite),
            cluster_count: 2, // 继承 granite_lower
            cluster_size: 64,
            min_height: 0,
            max_height: 60,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneOnly,
        },
        // --- Andesite (upper) ---
        OreSpec {
            name: "minecraft:overworld_underground_andesite_upper_feature",
            state: single(table.andesite),
            cluster_count: 6, // 继承 granite_upper
            cluster_size: 64,
            min_height: 64,
            max_height: 128,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: true,
            replace_target: StoneOnly,
        },
        // --- Andesite (lower) ---
        OreSpec {
            name: "minecraft:overworld_underground_andesite_lower_feature",
            state: single(table.andesite),
            cluster_count: 2, // 继承 granite_lower
            cluster_size: 64,
            min_height: 0,
            max_height: 60,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneOnly,
        },
        // --- Tuff ---
        OreSpec {
            name: "minecraft:overworld_underground_tuff_feature",
            state: single(table.tuff),
            cluster_count: 2,
            cluster_size: 64,
            min_height: -64,
            max_height: 0,
            concentration: Uniform,
            skip_air: 0.0,
            is_rare: false,
            replace_target: StoneAndDeepslate,
        },
    ]
}

// ---------------------------------------------------------------------------
// Emerald surface ore (CountGenerateFeature subclass).
// ---------------------------------------------------------------------------

/// Emerald ore on surface stone (not underground clusters).
///
/// Emerald ore on surface stone (not underground clusters).
/// count ranges [-2000, 15]; negative counts skip the loop.
pub struct EmeraldOreSurfaceFeature {
    table: OreBlockTable,
}

impl EmeraldOreSurfaceFeature {
    pub fn new(table: OreBlockTable) -> Self {
        Self { table }
    }
}

impl GenerateFeature for EmeraldOreSurfaceFeature {
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let level_seed = ctx.level_seed;
        let name_hash = crate::worldgen::math::java_string_hashcode(self.name()) as i64;
        let seed = level_seed ^ chunk_hash(chunk_x, chunk_z) ^ name_hash;
        let mut random = Xoroshiro128::new(seed);

        // Java: count = getBase() + random.nextBoundedInt(getRandom())
        let count = (-2000i32).wrapping_add(random.next_bounded_int(2015));
        for _ in 0..count {
            // Java populate:
            let x = random.next_int_max(15);
            let z = random.next_int_max(15);
            let y = ctx.chunk.height_map(x as u8, z as u8);
            let state = ctx.chunk.block_state(x as u8, y, z as u8, 0);
            if state == self.table.stone {
                ctx.chunk
                    .set_block_state(x as u8, y, z as u8, 0, self.table.emerald_ore);
            }
        }
    }

    fn name(&self) -> &'static str {
        "minecraft:extreme_hills_after_surface_emerald_ore_feature"
    }
}
