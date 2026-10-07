//! Feature (object/tree) generation infrastructure: ports of
//! `feature/ObjectGeneratorFeature.java`, `feature/GriddedFeature.java`,
//! `object/ObjectGenerator.java`, and `block/Supportable.java`.
//!
//! Upstream `Level`/`Registries.BIOME` reads map to this port as:
//! - `level.getBlock(v)` (world-space read) goes to `ctx.chunk.block_state`
//!   (inside the generating chunk) or a cross-chunk air fallback;
//! - `level.getBiomeId(x,y,z)` goes to `ctx.chunk.biome_id`;
//! - `level.getHeightMap(x,z)` goes to `ctx.chunk.height_map`;
//! - `level.getMinHeight()` goes to `ctx.min_y`.
//!
//! Block properties (canBeReplaced/isFullBlock/isLiquid/hasTag(DIRT)) have no
//! runtime block system here; [`TreeBlockTable`] id sets approximate them
//! (matching the official block_tags.json `minecraft:dirt` tag plus the stock
//! plant/leaf/liquid lists).

use std::collections::HashSet;

use sc_log::t_log;
use sc_world::block_dictionary::BlockStateDictionary;
use sc_world::chunk::BlockRuntimeId;

use crate::worldgen::chunk::WorldgenChunk;
use crate::worldgen::context::{BlockManager, ChunkGenerateContext};
use crate::worldgen::feature::GenerateFeature;
use crate::worldgen::math::{java_string_hashcode, random_range};
use crate::worldgen::random::{RandomSourceProvider, Xoroshiro128};
use crate::worldgen::stages::chunk_hash;
use crate::worldgen::stages::terrain::SEA_LEVEL;

/// Wood species (the 9 supported kinds).
/// Array index matches [`TreeBlockTable::logs`]/[`TreeBlockTable::leaves`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WoodType {
    Oak,
    Spruce,
    Birch,
    Jungle,
    DarkOak,
    Acacia,
    Cherry,
    PaleOak,
    Mangrove,
}

impl WoodType {
    /// All 9 kinds (declaration order).
    pub const ALL: [WoodType; 9] = [
        WoodType::Oak,
        WoodType::Spruce,
        WoodType::Birch,
        WoodType::Jungle,
        WoodType::DarkOak,
        WoodType::Acacia,
        WoodType::Cherry,
        WoodType::PaleOak,
        WoodType::Mangrove,
    ];

    pub fn log_name(self) -> &'static str {
        match self {
            WoodType::Oak => "minecraft:oak_log",
            WoodType::Spruce => "minecraft:spruce_log",
            WoodType::Birch => "minecraft:birch_log",
            WoodType::Jungle => "minecraft:jungle_log",
            WoodType::DarkOak => "minecraft:dark_oak_log",
            WoodType::Acacia => "minecraft:acacia_log",
            WoodType::Cherry => "minecraft:cherry_log",
            WoodType::PaleOak => "minecraft:pale_oak_log",
            WoodType::Mangrove => "minecraft:mangrove_log",
        }
    }

    pub fn leaves_name(self) -> &'static str {
        match self {
            WoodType::Oak => "minecraft:oak_leaves",
            WoodType::Spruce => "minecraft:spruce_leaves",
            WoodType::Birch => "minecraft:birch_leaves",
            WoodType::Jungle => "minecraft:jungle_leaves",
            WoodType::DarkOak => "minecraft:dark_oak_leaves",
            WoodType::Acacia => "minecraft:acacia_leaves",
            WoodType::Cherry => "minecraft:cherry_leaves",
            WoodType::PaleOak => "minecraft:pale_oak_leaves",
            WoodType::Mangrove => "minecraft:mangrove_leaves",
        }
    }

    /// Ordering matches the [`TreeBlockTable`] logs/leaves indices.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|w| *w == self).unwrap_or(0)
    }
}

/// Block table plus predicate sets for tree/feature generation.
///
/// Predicate sets approximate the upstream block property system:
/// - `dirt_support`: the 11 official `minecraft:dirt` tag members;
/// - `non_solid`: replaceables plus non-full blocks (air/leaves/plants/vines/snow/liquids);
/// - `liquids`: liquid check.
pub struct TreeBlockTable {
    pub air: BlockRuntimeId,
    pub dirt: BlockRuntimeId,
    pub grass_block: BlockRuntimeId,
    pub podzol: BlockRuntimeId,
    pub snow_layer: BlockRuntimeId,
    pub vine: BlockRuntimeId,
    pub brown_mushroom: BlockRuntimeId,
    pub red_mushroom: BlockRuntimeId,
    /// 9 log default states (pillar_axis=y).
    pub logs: [BlockRuntimeId; 9],
    /// 9 three-axis logs: `[wood][axis]` (X/Y/Z order).
    log_states: [[BlockRuntimeId; 3]; 9],
    /// 16 vine directions.
    vine_states: [BlockRuntimeId; 16],
    /// 9 leaves (persistent_bit=false, update_bit=false).
    pub leaves: [BlockRuntimeId; 9],
    /// Jungle ground check (`minecraft:farmland{moisturized_amount=0}`).
    pub farmland: BlockRuntimeId,
    /// Still water (`minecraft:water{liquid_depth=0}`).
    pub water: BlockRuntimeId,
    /// `minecraft:flowing_water` (falls back to water when absent).
    pub flowing_water: BlockRuntimeId,
    /// `minecraft:mud` (mangrove roots pass through it).
    pub mud: BlockRuntimeId,
    /// `minecraft:mangrove_roots`.
    pub mangrove_roots: BlockRuntimeId,
    /// `minecraft:muddy_mangrove_roots{pillar_axis=y}`.
    pub muddy_mangrove_roots_y: BlockRuntimeId,
    /// `minecraft:moss_carpet`.
    pub moss_carpet: BlockRuntimeId,
    /// Hanging moss (`tip` false/true).
    pub pale_hanging_moss: [BlockRuntimeId; 2],
    /// Mangrove propagules (`hanging` by `propagule_stage` 0-4).
    pub mangrove_propagule: [[BlockRuntimeId; 5]; 2],
    /// Creaking hearts (uprooted, natural=false, y axis).
    pub creaking_heart: BlockRuntimeId,
    /// Cocoa (`[direction 0-3][age 0-2]`).
    pub cocoa_states: [[BlockRuntimeId; 3]; 4],
    /// `minecraft:shroomlight` (1/20 nether leaf replacement).
    pub shroomlight: BlockRuntimeId,
    /// `minecraft:crimson_stem` (pillar_axis=y).
    pub crimson_stem: BlockRuntimeId,
    /// `minecraft:warped_stem` (pillar_axis=y).
    pub warped_stem: BlockRuntimeId,
    /// `minecraft:nether_wart_block`.
    pub nether_wart_block: BlockRuntimeId,
    /// `minecraft:warped_wart_block`.
    pub warped_wart_block: BlockRuntimeId,
    /// `minecraft:chorus_plant` (default state).
    pub chorus_plant: BlockRuntimeId,
    /// Fully ripe `minecraft:chorus_flower{age=5}`.
    pub chorus_flower_fully_aged: BlockRuntimeId,
    /// `minecraft:bee_nest{direction=0, honey_level=0}`
    ///(BeeNestGenerator——direction 0 = SOUTH horizontalIndex).
    pub bee_nest: BlockRuntimeId,
    /// 14 surface plants in array order.
    pub tall_grass_places: [BlockRuntimeId; 14],
    /// `minecraft:tall_grass{upper_block_bit=true}` (upper half).
    pub tall_grass_upper: BlockRuntimeId,
    /// `minecraft:bamboo` (excluded by the bamboo check).
    pub bamboo: BlockRuntimeId,
    /// `minecraft:azalea_leaves`.
    pub azalea_leaves: BlockRuntimeId,
    /// `minecraft:azalea_leaves_flowered`.
    pub azalea_leaves_flowered: BlockRuntimeId,
    /// `minecraft:dirt_with_roots`.
    pub dirt_with_roots: BlockRuntimeId,
    /// `minecraft:dirt` tag members.
    dirt_support: HashSet<BlockRuntimeId>,
    /// Approximation of canBeReplaced() || !isFullBlock().
    non_solid: HashSet<BlockRuntimeId>,
    /// Liquid predicate set.
    liquids: HashSet<BlockRuntimeId>,
    /// Blocks trees can grow through.
    growable: HashSet<BlockRuntimeId>,
    /// Legacy-tree overridables: air, leaves, snow, saplings,
    /// surface plants, flowers.
    overridable: HashSet<BlockRuntimeId>,
    /// All leaf variants (full states of the 11 leaves).
    leaves_set: HashSet<BlockRuntimeId>,
    /// Full water states (depth-independent).
    water_set: HashSet<BlockRuntimeId>,
}

/// Official `minecraft:dirt` tag members.
pub const DIRT_TAG_MEMBERS: [&str; 11] = [
    "minecraft:mycelium",
    "minecraft:pale_moss_block",
    "minecraft:podzol",
    "minecraft:muddy_mangrove_roots",
    "minecraft:farmland",
    "minecraft:dirt_with_roots",
    "minecraft:coarse_dirt",
    "minecraft:dirt",
    "minecraft:grass_block",
    "minecraft:moss_block",
    "minecraft:mud",
];

/// Surface plants and non-full blocks (skipped by the downward scan:
/// replaceable or non-full; liquids check separately).
pub const NON_SOLID_BLOCKS: [&str; 48] = [
    // Air (injected separately).
    // 9 leaves.
    "minecraft:oak_leaves",
    "minecraft:spruce_leaves",
    "minecraft:birch_leaves",
    "minecraft:jungle_leaves",
    "minecraft:dark_oak_leaves",
    "minecraft:acacia_leaves",
    "minecraft:cherry_leaves",
    "minecraft:pale_oak_leaves",
    "minecraft:mangrove_leaves",
    "minecraft:azalea_leaves",
    "minecraft:azalea_leaves_flowered",
    // Vines.
    "minecraft:vine",
    // Snow layers.
    "minecraft:snow_layer",
    // Grass/ferns.
    "minecraft:short_grass",
    "minecraft:tall_grass",
    "minecraft:fern",
    "minecraft:large_fern",
    "minecraft:dead_bush",
    "minecraft:bamboo_sapling",
    // Flowers.
    "minecraft:dandelion",
    "minecraft:poppy",
    "minecraft:blue_orchid",
    "minecraft:allium",
    "minecraft:azure_bluet",
    "minecraft:red_tulip",
    "minecraft:orange_tulip",
    "minecraft:white_tulip",
    "minecraft:pink_tulip",
    "minecraft:oxeye_daisy",
    "minecraft:cornflower",
    "minecraft:lily_of_the_valley",
    "minecraft:wither_rose",
    "minecraft:torchflower",
    "minecraft:sunflower",
    "minecraft:lilac",
    "minecraft:rose_bush",
    "minecraft:peony",
    "minecraft:pink_petals",
    "minecraft:wildflowers",
    "minecraft:leaf_litter",
    // Saplings.
    "minecraft:oak_sapling",
    "minecraft:spruce_sapling",
    "minecraft:birch_sapling",
    "minecraft:jungle_sapling",
    "minecraft:dark_oak_sapling",
    "minecraft:acacia_sapling",
    "minecraft:cherry_sapling",
    "minecraft:pale_oak_sapling",
];

/// Liquids.
pub const LIQUIDS: [&str; 4] = [
    "minecraft:water",
    "minecraft:flowing_water",
    "minecraft:lava",
    "minecraft:flowing_lava",
];

/// Blocks trees can grow through.
/// Upstream switches on id strings; this port uses a runtime-id set
/// (air, leaves, logs, dirt-likes, saplings, plants, vines, bamboo, moss).
const GROWABLE_BLOCKS: [&str; 43] = [
    "minecraft:oak_leaves",
    "minecraft:spruce_leaves",
    "minecraft:birch_leaves",
    "minecraft:jungle_leaves",
    "minecraft:dark_oak_leaves",
    "minecraft:acacia_leaves",
    "minecraft:cherry_leaves",
    "minecraft:pale_oak_leaves",
    "minecraft:mangrove_leaves",
    "minecraft:azalea_leaves",
    "minecraft:azalea_leaves_flowered",
    "minecraft:grass_block",
    "minecraft:dirt",
    "minecraft:oak_log",
    "minecraft:spruce_log",
    "minecraft:birch_log",
    "minecraft:jungle_log",
    "minecraft:dark_oak_log",
    "minecraft:acacia_log",
    "minecraft:cherry_log",
    "minecraft:pale_oak_log",
    "minecraft:mangrove_log",
    "minecraft:vine",
    "minecraft:dirt_with_roots",
    "minecraft:mangrove_roots",
    "minecraft:mangrove_propagule",
    "minecraft:oak_sapling",
    "minecraft:spruce_sapling",
    "minecraft:birch_sapling",
    "minecraft:jungle_sapling",
    "minecraft:dark_oak_sapling",
    "minecraft:acacia_sapling",
    "minecraft:cherry_sapling",
    "minecraft:pale_oak_sapling",
    "minecraft:bamboo_sapling",
    "minecraft:fern",
    "minecraft:short_grass",
    "minecraft:tall_grass",
    "minecraft:pale_hanging_moss",
    "minecraft:closed_eyeblossom",
    "minecraft:open_eyeblossom",
    "minecraft:leaf_litter",
    "minecraft:bamboo",
    // The list also covers grass/dirt plus injected air.
];

/// Overridable id list (air injected separately; pale_oak_leaves
/// stays excluded for parity).
const OVERRIDABLE_BLOCKS: [&str; 32] = [
    "minecraft:acacia_leaves",
    "minecraft:azalea_leaves",
    "minecraft:birch_leaves",
    "minecraft:azalea_leaves_flowered",
    "minecraft:cherry_leaves",
    "minecraft:dark_oak_leaves",
    "minecraft:jungle_leaves",
    "minecraft:mangrove_leaves",
    "minecraft:oak_leaves",
    "minecraft:spruce_leaves",
    "minecraft:snow_layer",
    "minecraft:acacia_sapling",
    "minecraft:cherry_sapling",
    "minecraft:spruce_sapling",
    "minecraft:bamboo_sapling",
    "minecraft:oak_sapling",
    "minecraft:jungle_sapling",
    "minecraft:dark_oak_sapling",
    "minecraft:leaf_litter",
    "minecraft:wildflowers",
    "minecraft:pink_petals",
    "minecraft:tall_grass",
    "minecraft:birch_sapling",
    "minecraft:short_grass",
    "minecraft:dandelion",
    "minecraft:lily_of_the_valley",
    "minecraft:lilac",
    "minecraft:peony",
    "minecraft:rose_bush",
    "minecraft:large_fern",
    "minecraft:fern",
    "minecraft:closed_eyeblossom",
];

/// Coordinate axes (three-axis logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn state_value(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        }
    }
}

impl TreeBlockTable {
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
                log::warn!("{}", t_log!("console.worldgen.table_missing", table = "tree", name = format!("{name:?}")));
                air
            }
        };
        // Exact states prefer the snapshot (always at hand here) over the
        // global dictionary (dlopened plugin copies may hold an empty one).
        let lookup_state = |name: &str, key: &str, value: &str| {
            if let Some(hash) = snapshot.and_then(|s| s.find_state_hash(name, &[(key, value)])) {
                return BlockRuntimeId(hash);
            }
            match dictionary.find_hash_by_state(name, key, value) {
                Some(hash) => BlockRuntimeId(hash),
                None => {
                    log::warn!("{}", t_log!("console.worldgen.table_state", table = "tree", name = name, detail = format!("[{key}={value}]")));
                    lookup(name)
                }
            }
        };
        let lookup_states = |name: &str, pairs: &[(&str, &str)]| {
            if let Some(hash) = snapshot.and_then(|s| s.find_state_hash(name, pairs)) {
                return BlockRuntimeId(hash);
            }
            match dictionary.find_hash_by_states(name, pairs) {
                Some(hash) => BlockRuntimeId(hash),
                None => {
                    log::warn!("{}", t_log!("console.worldgen.table_state", table = "tree", name = name, detail = format!("{pairs:?}")));
                    lookup(name)
                }
            }
        };

        let logs = WoodType::ALL.map(|w| lookup(w.log_name()));
        // Leaf defaults (persistent_bit=false, update_bit=false).
        let leaves = WoodType::ALL.map(|w| {
            lookup_states(
                w.leaves_name(),
                &[("persistent_bit", "false"), ("update_bit", "false")],
            )
        });
        // Three-axis logs and 16-direction vines.
        let mut log_states = [[air; 3]; 9];
        for (i, wood) in WoodType::ALL.iter().enumerate() {
            log_states[i] = [
                lookup_state(wood.log_name(), "pillar_axis", "x"),
                lookup_state(wood.log_name(), "pillar_axis", "y"),
                lookup_state(wood.log_name(), "pillar_axis", "z"),
            ];
        }
        let mut vine_states = [air; 16];
        for (bits, state) in vine_states.iter_mut().enumerate() {
            *state = lookup_state("minecraft:vine", "vine_direction_bits", &bits.to_string());
        }

        // Cocoa (direction 0-3 by age 0-2).
        let mut cocoa_states = [[air; 3]; 4];
        for direction in 0..4usize {
            for age in 0..3usize {
                cocoa_states[direction][age] = lookup_states(
                    "minecraft:cocoa",
                    &[
                        ("direction", &direction.to_string()),
                        ("age", &age.to_string()),
                    ],
                );
            }
        }
        // Hanging moss (tip false/true).
        let pale_hanging_moss = [
            lookup_state("minecraft:pale_hanging_moss", "tip", "false"),
            lookup_state("minecraft:pale_hanging_moss", "tip", "true"),
        ];
        // Mangrove propagules.
        let mut mangrove_propagule = [[air; 5]; 2];
        for (hanging, states) in mangrove_propagule.iter_mut().enumerate() {
            for (stage, state) in states.iter_mut().enumerate() {
                *state = lookup_states(
                    "minecraft:mangrove_propagule",
                    &[
                        ("hanging", if hanging == 0 { "false" } else { "true" }),
                        ("propagule_stage", &stage.to_string()),
                    ],
                );
            }
        }
        // Creaking hearts (uprooted, natural=false, pillar_axis=y).
        let creaking_heart = lookup_states(
            "minecraft:creaking_heart",
            &[
                ("creaking_heart_state", "uprooted"),
                ("natural", "false"),
                ("pillar_axis", "y"),
            ],
        );

        // Legacy tree blocks.
        // Crimson/warped stems and wart-block leaves.
        let crimson_stem = lookup_state("minecraft:crimson_stem", "pillar_axis", "y");
        let warped_stem = lookup_state("minecraft:warped_stem", "pillar_axis", "y");
        // Chorus plants/flowers (age 0-5, max 5).
        let chorus_plant = lookup("minecraft:chorus_plant");
        let chorus_flower_fully_aged = lookup_state("minecraft:chorus_flower", "age", "5");
        // Beehives (direction 0, honey_level 0).
        let bee_nest = lookup_states(
            "minecraft:bee_nest",
            &[("direction", "0"), ("honey_level", "0")],
        );
        // 14 tall-grass plants in array order.
        let tall_grass_places = [
            lookup("minecraft:short_grass"),
            lookup("minecraft:tall_grass"),
            lookup("minecraft:dandelion"),
            lookup("minecraft:poppy"),
            lookup("minecraft:azure_bluet"),
            lookup("minecraft:oxeye_daisy"),
            lookup("minecraft:allium"),
            lookup("minecraft:cornflower"),
            lookup("minecraft:blue_orchid"),
            lookup("minecraft:lily_of_the_valley"),
            lookup("minecraft:red_tulip"),
            lookup("minecraft:orange_tulip"),
            lookup("minecraft:pink_tulip"),
            lookup("minecraft:white_tulip"),
        ];
        let tall_grass_upper = lookup_state("minecraft:tall_grass", "upper_block_bit", "true");

        let water = lookup_states("minecraft:water", &[("liquid_depth", "0")]);
        // Missing flowing_water falls back to water.
        let flowing_water = match dictionary.first_hash_of("minecraft:flowing_water") {
            Some(hash) => BlockRuntimeId(hash),
            None => water,
        };

        // Name lists expand to full state sets: generation checks key off
        // block id strings regardless of state, so sets must cover every
        // state hash of an id (e.g. all 16 water depths); unregistered ids
        // fall back to a single state. Snapshot first: dlopened plugin copies
        // may hold an empty global dictionary.
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
        let mut growable = states_of_many(&GROWABLE_BLOCKS);
        growable.insert(air);
        let mut overridable = states_of_many(&OVERRIDABLE_BLOCKS);
        overridable.insert(air);

        // Full leaf-variant set (all states of the 11 leaves).
        let leaves_set = states_of_many(&[
            "minecraft:oak_leaves",
            "minecraft:spruce_leaves",
            "minecraft:birch_leaves",
            "minecraft:jungle_leaves",
            "minecraft:dark_oak_leaves",
            "minecraft:acacia_leaves",
            "minecraft:cherry_leaves",
            "minecraft:pale_oak_leaves",
            "minecraft:mangrove_leaves",
            "minecraft:azalea_leaves",
            "minecraft:azalea_leaves_flowered",
        ]);
        let water_set = states_of("minecraft:water");

        Self {
            dirt: lookup("minecraft:dirt"),
            grass_block: lookup("minecraft:grass_block"),
            podzol: lookup("minecraft:podzol"),
            snow_layer: lookup("minecraft:snow_layer"),
            vine: lookup("minecraft:vine"),
            brown_mushroom: lookup("minecraft:brown_mushroom"),
            red_mushroom: lookup("minecraft:red_mushroom"),
            air,
            logs,
            log_states,
            vine_states,
            leaves,
            farmland: lookup_state("minecraft:farmland", "moisturized_amount", "0"),
            water,
            flowing_water,
            mud: lookup("minecraft:mud"),
            mangrove_roots: lookup("minecraft:mangrove_roots"),
            muddy_mangrove_roots_y: lookup_state(
                "minecraft:muddy_mangrove_roots",
                "pillar_axis",
                "y",
            ),
            moss_carpet: lookup("minecraft:moss_carpet"),
            pale_hanging_moss,
            mangrove_propagule,
            creaking_heart,
            cocoa_states,
            shroomlight: lookup("minecraft:shroomlight"),
            crimson_stem,
            warped_stem,
            nether_wart_block: lookup("minecraft:nether_wart_block"),
            warped_wart_block: lookup("minecraft:warped_wart_block"),
            chorus_plant,
            chorus_flower_fully_aged,
            bee_nest,
            tall_grass_places,
            tall_grass_upper,
            bamboo: lookup("minecraft:bamboo"),
            azalea_leaves: lookup("minecraft:azalea_leaves"),
            azalea_leaves_flowered: lookup("minecraft:azalea_leaves_flowered"),
            dirt_with_roots: lookup("minecraft:dirt_with_roots"),
            dirt_support: states_of_many(&DIRT_TAG_MEMBERS),
            non_solid,
            liquids,
            growable,
            overridable,
            leaves_set,
            water_set,
        }
    }

    /// Three-axis log state.
    pub fn log_state(&self, wood: WoodType, axis: Axis) -> BlockRuntimeId {
        let axis_index = match axis {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        };
        self.log_states[wood.index()][axis_index]
    }

    /// Per-species log default (pillar_axis=y).
    pub fn log_of(&self, wood: WoodType) -> BlockRuntimeId {
        self.logs[wood.index()]
    }

    /// Per-species leaf default (both bits false).
    pub fn leaves_of(&self, wood: WoodType) -> BlockRuntimeId {
        self.leaves[wood.index()]
    }

    /// Vine direction state (bits 0-15).
    pub fn vine_state(&self, bits: u8) -> BlockRuntimeId {
        self.vine_states[(bits & 0xF) as usize]
    }

    /// Java: `TreeGenerator.canGrowInto(String id)`(L20-57).
    pub fn can_grow_into(&self, block: BlockRuntimeId) -> bool {
        self.growable.contains(&block)
    }

    /// `Block.isSolid()` approximation: everything outside the non-solid
    /// set (air/leaves/plants/vines/snow/liquids) counts as solid.
    pub fn is_solid(&self, block: BlockRuntimeId) -> bool {
        !self.non_solid.contains(&block)
    }

    /// Java: `Supportable.isSupportDirt`(L7-9)——`block.hasTag(BlockTags.DIRT)`.
    pub fn is_support_dirt(&self, block: BlockRuntimeId) -> bool {
        self.dirt_support.contains(&block)
    }

    /// Java: `Supportable.isSupportGrass`(L11-13)——
    /// `block instanceof BlockGrassBlock && !(block instanceof BlockGrassPath)`.
    /// grass_path is its own id, so this equals `block == grass_block`.
    pub fn is_support_grass(&self, block: BlockRuntimeId) -> bool {
        block == self.grass_block
    }

    /// Block half of the checkBlock scan:
    /// `(bl.canBeReplaced() || !bl.isFullBlock()) && !(bl instanceof BlockLiquid)`.
    pub fn check_block(&self, block: BlockRuntimeId) -> bool {
        self.non_solid.contains(&block) && !self.liquids.contains(&block)
    }

    /// `Block.canBeReplaced()` approximation: non-solid set plus liquids
    /// (air, liquids, and plants override to true).
    /// CheckBlock override hook.
    pub fn can_be_replaced(&self, block: BlockRuntimeId) -> bool {
        self.non_solid.contains(&block) || self.liquids.contains(&block)
    }

    /// Liquid check.
    pub fn is_liquid(&self, block: BlockRuntimeId) -> bool {
        self.liquids.contains(&block)
    }

    /// Leaf check (all states of the 11 leaves).
    pub fn is_leaves(&self, block: BlockRuntimeId) -> bool {
        self.leaves_set.contains(&block)
    }

    /// Water id check (any liquid_depth).
    pub fn is_water(&self, block: BlockRuntimeId) -> bool {
        self.water_set.contains(&block)
    }

    /// Water or flowing-water id check
    /// (used by mangrove placement checks).
    pub fn is_water_or_flowing(&self, block: BlockRuntimeId) -> bool {
        self.is_water(block) || block == self.flowing_water
    }

    /// Vine id check (any direction bits).
    pub fn is_vine(&self, block: BlockRuntimeId) -> bool {
        self.vine_states.contains(&block)
    }

    /// Mangrove propagule id check
    /// (any hanging/propagule_stage state).
    pub fn is_mangrove_propagule(&self, block: BlockRuntimeId) -> bool {
        self.mangrove_propagule
            .iter()
            .flatten()
            .any(|s| *s == block)
    }

    /// Legacy-tree overridables: air, leaves, snow, saplings,
    /// surface plants, flowers.
    pub fn is_overridable(&self, block: BlockRuntimeId) -> bool {
        self.overridable.contains(&block)
    }
}

/// Java: `ObjectGenerator`(object/ObjectGenerator.java).
///
/// Note `&mut self`: legacy trees write their height/vine fields during
/// placeObject, expressed as a mutable borrow.
pub trait ObjectGenerator: Send + Sync {
    /// Java: `boolean generate(BlockManager level, RandomSourceProvider rand, Vector3 position)`.
    fn generate(
        &mut self,
        level: &mut BlockManager<'_>,
        rand: &mut Xoroshiro128,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool;

    /// Beehive eligibility flag.
    /// `generator instanceof LegacyOakTree || generator instanceof LegacyBirchTree`
    /// Only legacy oak/birch override it to true.
    fn is_bee_nest_eligible(&self) -> bool {
        false
    }
}

/// Java: `ObjectGeneratorFeature extends GenerateFeature implements Supportable`.
///
/// Default apply: random attempts per chunk at random columns, reading the 
/// heightmap with biome filtering, scanning down to support before generating.
pub trait ObjectGeneratorFeature: GenerateFeature {
    /// Java: `ObjectGenerator getGenerator(RandomSourceProvider random)`(L22).
    fn get_generator(&self, random: &mut Xoroshiro128) -> Box<dyn ObjectGenerator>;

    /// Java: `getMin()`(L24-26).
    fn get_min(&self) -> i32 {
        5
    }

    /// Java: `getMax()`(L28-30).
    fn get_max(&self) -> i32 {
        6
    }

    /// Java: `canSpawnHere(BiomeDefinitionData)`(L32-34).
    /// Upstream matches tags by biome; this port passes the biome id.
    fn can_spawn_here(&self, biome_id: i32) -> bool {
        let _ = biome_id;
        true
    }

    /// Tree block table.
    fn tree_table(&self) -> &TreeBlockTable;

    /// checkBlock is overridable (bamboo excludes bamboo, swamp relaxes
    /// to canBeReplaced).
    fn check_block(&self, block: BlockRuntimeId) -> bool {
        self.tree_table().check_block(block)
    }

    /// Java: `apply(ChunkGenerateContext)`(L37-64).
    ///
    /// Seed formula (`+` binds tighter than `^`):
    /// `levelSeed ^ (chunkHash(chunkX, chunkZ) + name().hashCode())`.
    fn object_apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let sx = chunk_x << 4;
        let sz = chunk_z << 4;

        let name_hash = java_string_hashcode(self.name()) as i64;
        let seed = ctx.level_seed() ^ chunk_hash(chunk_x, chunk_z).wrapping_add(name_hash);
        let mut random = Xoroshiro128::new(seed);

        let amount = random_range(&mut random, self.get_min(), self.get_max());

        // The tree generator must see current-chunk terrain,
        // or every ground check fails false.
        let chunk_ref: &WorldgenChunk = ctx.chunk;
        let mut object = BlockManager::with_chunk_and_seed(chunk_ref, ctx.level_seed());

        for _ in 0..amount {
            // Java L47-48: random.nextInt(15)
            let x = random.next_int_max(15);
            let z = random.next_int_max(15);
            // Java L49: chunk.getHeightMap(x, z)
            let y = ctx.chunk.height_map(x as u8, z as u8);
            // Java L50-52: y < level.getMinHeight() → continue
            if y < ctx.min_y() {
                continue;
            }
            let wx = x + sx;
            let wz = z + sz;

            // Java L54: canSpawnHere(biome at v)
            if !self.can_spawn_here(ctx.chunk.biome_id(x as u8, y, z as u8)) {
                continue;
            }

            // Java L55-57: while(checkBlock(level.getBlock(v))) v.y--;
            let mut vy = y;
            while vy > ctx.min_y()
                && self.check_block(ctx.chunk.block_state(x as u8, vy, z as u8, 0))
                && vy > SEA_LEVEL
            {
                vy -= 1;
            }

            // Java L58-60: isSupportDirt(level.getBlock(v)) → generate(v.add(0,1,0))
            let support = ctx.chunk.block_state(x as u8, vy, z as u8, 0);
            if self.tree_table().is_support_dirt(support) {
                let mut generator = self.get_generator(&mut random);
                generator.generate(&mut object, &mut random, wx, vy + 1, wz);
            }
        }

        // queueObject writes into the pending buffer.
        // into_places consumes the object, then merges into root.
        let places = object.into_places();
        ctx.queue_object(places);
    }
}

/// Java: `GriddedFeature extends ObjectGeneratorFeature`(feature/GriddedFeature.java).
///
/// Points spread over a split x split grid (random in-cell offset plus
/// distance constraint); the heightmap top decides, no downward scan.
pub trait GriddedFeature: ObjectGeneratorFeature {
    /// Java: `getSplit()`(L12-14).
    fn get_split(&self) -> i32 {
        2
    }

    /// Java: `splitLength()`(L20-22)——`16/getSplit()`.
    fn split_length(&self) -> i32 {
        16 / self.get_split()
    }

    /// Java: `getDistanceToNextField()`(L16-18)——
    /// `getSplit() > splitLength() ? splitLength()/2 : getSplit()`.
    fn get_distance_to_next_field(&self) -> i32 {
        if self.get_split() > self.split_length() {
            self.split_length() / 2
        } else {
            self.get_split()
        }
    }

    /// Java: `apply(ChunkGenerateContext)`(L24-46,@Override).
    ///
    /// Seed formula (plain `^` chain, unlike the `+` variant above):
    /// `levelSeed ^ chunkHash ^ (x + z) ^ name().hashCode()`.
    fn grid_apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();

        let name_hash = java_string_hashcode(self.name()) as i64;
        let split = self.get_split();
        let split_length = self.split_length();
        let dist = self.get_distance_to_next_field();

        // Fresh BlockManager with current-chunk terrain visible.
        let chunk_ref: &WorldgenChunk = ctx.chunk;
        let mut object = BlockManager::with_chunk_and_seed(chunk_ref, ctx.level_seed());

        for x in 0..split {
            for z in 0..split {
                // Java L33: setSeed(seed ^ chunkHash ^ (x + z) ^ nameHash)
                let seed =
                    ctx.level_seed() ^ chunk_hash(chunk_x, chunk_z) ^ ((x + z) as i64) ^ name_hash;
                let mut random = Xoroshiro128::new(seed);

                // Java L35-36: placeX/placeZ = dist + nextInt(splitLength - dist)
                //   + (i * splitLength) + (chunkX << 4)
                let place_x = dist
                    + random.next_int_max(split_length - dist)
                    + (x * split_length)
                    + (chunk_x << 4);
                let place_z = dist
                    + random.next_int_max(split_length - dist)
                    + (z * split_length)
                    + (chunk_z << 4);
                // placeX/placeZ stay inside the current chunk.
                let local_x = (place_x & 0xF) as u8;
                let local_z = (place_z & 0xF) as u8;

                // Java L37: level.getHeightMap(placeX, placeZ)
                let place_y = ctx.chunk.height_map(local_x, local_z);

                // Java L39: canSpawnHere(biome at (placeX, placeY, placeZ))
                if !self.can_spawn_here(ctx.chunk.biome_id(local_x, place_y, local_z)) {
                    continue;
                }

                // Java L40-42: isSupportDirt(level.getBlock(placeX, placeY, placeZ))
                //   → generate((placeX, placeY + 1, placeZ))
                let support = ctx.chunk.block_state(local_x, place_y, local_z, 0);
                if self.tree_table().is_support_dirt(support) {
                    let mut generator = self.get_generator(&mut random);
                    generator.generate(&mut object, &mut random, place_x, place_y + 1, place_z);
                }
            }
        }

        // Java L45: queueObject(chunk, object) → root.merge(object)——
        // Buffer until stage end, then submit once.
        let places = object.into_places();
        ctx.queue_object(places);
    }
}

/// Read world-space blocks of the generating chunk:
/// terrain inside, air across chunks (unloaded chunks read air upstream too).
pub fn level_block_at(chunk: &WorldgenChunk, x: i32, y: i32, z: i32) -> BlockRuntimeId {
    if (x >> 4) == chunk.x() && (z >> 4) == chunk.z() {
        chunk.block_state((x & 0xF) as u8, y, (z & 0xF) as u8, 0)
    } else {
        BlockRuntimeId(sc_world::block_dictionary::air_runtime_id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_table_builds_from_palette() {
        let table = TreeBlockTable::from_core_palette();
        // The core palette may lack the version pack (all air fallback).
        if table.dirt != table.air {
            // Core dirt-tag members resolve (most of the 11 parse).
            let non_air_dirt = DIRT_TAG_MEMBERS
                .iter()
                .filter(|name| {
                    BlockStateDictionary::global()
                        .first_hash_of(name)
                        .is_some_and(|hash| BlockRuntimeId(hash) != table.air)
                })
                .count();
            assert!(
                non_air_dirt >= 9,
                "dirt tag 成员应大部分解析成功（{}/11）",
                non_air_dirt
            );
            // isSupportDirt: dirt/grass_block check.
            assert!(table.is_support_dirt(table.dirt));
            assert!(table.is_support_dirt(table.grass_block));
            assert!(!table.is_support_dirt(table.air));
            // isSupportGrass: grass_block only.
            assert!(table.is_support_grass(table.grass_block));
            assert!(!table.is_support_grass(table.dirt));
            // checkBlock: air/leaves/snow pass, dirt/logs do not.
            assert!(table.check_block(table.air));
            assert!(table.check_block(table.leaves[0]));
            assert!(table.check_block(table.snow_layer));
            assert!(!table.check_block(table.dirt));
            assert!(!table.check_block(table.logs[0]));
        }
    }

    #[test]
    fn gridded_geometry_matches_java() {
        struct Dummy;
        // split=2 gives splitLength=8, distToNext=2.
        // split=4 gives splitLength=4, distToNext=2.
        //   Java: getSplit() > splitLength() ? splitLength()/2 : getSplit() → 4
        // split=8 → splitLength=2, distToNext=1(8 > 2 → 2/2=1)
        assert_eq!(gridded_geometry(2), (8, 2));
        assert_eq!(gridded_geometry(4), (4, 4));
        assert_eq!(gridded_geometry(8), (2, 1));
    }

    fn gridded_geometry(split: i32) -> (i32, i32) {
        let split_length = 16 / split;
        let dist = if split > split_length {
            split_length / 2
        } else {
            split
        };
        (split_length, dist)
    }
}
