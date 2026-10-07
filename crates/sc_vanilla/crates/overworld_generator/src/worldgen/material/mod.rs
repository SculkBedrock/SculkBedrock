//! Material filler rules for stone, water, lava, and ore veins.
//!
//! | Rust module | Java source |
//! |---|---|
//! | [`filler`] | `MaterialFiller.java` + `MultiMaterial.java` |
//! | [`aquifer`] | `Aquifer.java` |
//! | [`ore_veinifier`] | `OreVeinifier.java` |

pub mod aquifer;
pub mod filler;
pub mod ore_veinifier;

use sc_block::block_json::BlockJsonSnapshot;
use sc_world::chunk::BlockRuntimeId;

/// Static blocks referenced by material rules, injected via constructor
/// (replaces static default-state references; avoids a global dictionary dependency).
#[derive(Clone, Debug)]
pub struct MaterialBlocks {
    /// Java `BlockAir.STATE` / `BlockAir.PROPERTIES.getDefaultState()`.
    pub air: BlockRuntimeId,
    /// Java `BlockWater.PROPERTIES.getDefaultState()`.
    pub water: BlockRuntimeId,
    /// Java `BlockLava.PROPERTIES.getDefaultState()`.
    pub lava: BlockRuntimeId,
    /// Java `BlockStone.PROPERTIES.getDefaultState()`.
    pub stone: BlockRuntimeId,
    /// Java `BlockGranite.PROPERTIES.getDefaultState()`(COPPER filler).
    pub granite: BlockRuntimeId,
    /// Java `BlockTuff.PROPERTIES.getDefaultState()`(IRON filler).
    pub tuff: BlockRuntimeId,
    /// Java `BlockCopperOre.PROPERTIES.getDefaultState()`.
    pub copper_ore: BlockRuntimeId,
    /// Java `BlockDeepslateIronOre.PROPERTIES.getDefaultState()`.
    pub deepslate_iron_ore: BlockRuntimeId,
    /// Java `BlockRawCopperBlock.PROPERTIES.getDefaultState()`.
    pub raw_copper_block: BlockRuntimeId,
    /// Java `BlockRawIronBlock.PROPERTIES.getDefaultState()`.
    pub raw_iron_block: BlockRuntimeId,
}

impl MaterialBlocks {
    /// Builds from the core palette.
    pub fn from_core_palette() -> Self {
        Self::build(None)
    }

    /// Builds from a block bundle snapshot.
    pub fn from_block_snapshot(snapshot: &BlockJsonSnapshot) -> Self {
        Self::build(Some(snapshot))
    }

    fn build(snapshot: Option<&BlockJsonSnapshot>) -> Self {
        let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        let lookup = |name: &str| {
            crate::blocks_table::resolve_block_default(snapshot, name)
                .map(BlockRuntimeId)
                .unwrap_or(air)
        };
        Self {
            air,
            water: lookup("minecraft:water"),
            lava: lookup("minecraft:lava"),
            stone: lookup("minecraft:stone"),
            granite: lookup("minecraft:granite"),
            tuff: lookup("minecraft:tuff"),
            copper_ore: lookup("minecraft:copper_ore"),
            deepslate_iron_ore: lookup("minecraft:deepslate_iron_ore"),
            raw_copper_block: lookup("minecraft:raw_copper_block"),
            raw_iron_block: lookup("minecraft:raw_iron_block"),
        }
    }
}
