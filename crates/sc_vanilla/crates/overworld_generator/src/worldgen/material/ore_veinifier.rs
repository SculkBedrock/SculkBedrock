//! Ore vein material rule placing copper/iron veins with noise gates.
//!
//! Block table is injected via MaterialBlocks.
//! Block table is injected via MaterialBlocks. //

use crate::worldgen::densityfunction::function::{DensityFunction, FunctionContext};
use crate::worldgen::material::filler::MaterialFiller;
use crate::worldgen::material::MaterialBlocks;
use crate::worldgen::math::clamp_f64;
use crate::worldgen::random::{MtRandom, RandomSourceProvider};
use std::cell::RefCell;
use std::sync::Arc;
use sc_world::chunk::BlockRuntimeId;

/// Java: `private static final float VEININESS_THRESHOLD = 0.4F`(L17).
const VEININESS_THRESHOLD: f32 = 0.4;
/// Java: `private static final int EDGE_ROUNDOFF_BEGIN = 20`(L18).
const EDGE_ROUNDOFF_BEGIN: i32 = 20;
/// Java: `private static final double MAX_EDGE_ROUNDOFF = 0.2`(L19).
const MAX_EDGE_ROUNDOFF: f64 = 0.2;
/// Java: `private static final float VEIN_SOLIDNESS = 0.7F`(L20).
const VEIN_SOLIDNESS: f32 = 0.7;
/// Java: `private static final float MIN_RICHNESS = 0.1F`(L21).
const MIN_RICHNESS: f32 = 0.1;
/// Java: `private static final float MAX_RICHNESS = 0.3F`(L22).
const MAX_RICHNESS: f32 = 0.3;
/// Java: `private static final float MAX_RICHNESS_THRESHOLD = 0.6F`(L23).
const MAX_RICHNESS_THRESHOLD: f32 = 0.6;
/// Java: `private static final float CHANCE_OF_RAW_ORE_BLOCK = 0.02F`(L24).
const CHANCE_OF_RAW_ORE_BLOCK: f32 = 0.02;
/// Java: `private static final float SKIP_ORE_IF_GAP_NOISE_IS_BELOW = -0.3F`(L25).
const SKIP_ORE_IF_GAP_NOISE_IS_BELOW: f32 = -0.3;
/// Java: `public static final int MIN_VEIN_Y = VeinType.IRON.minY`(L26).
pub const MIN_VEIN_Y: i32 = -60;
/// Java: `public static final int MAX_VEIN_Y = VeinType.COPPER.maxY`(L27).
pub const MAX_VEIN_Y: i32 = 50;

/// Java: `private enum VeinType`(L112-141).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VeinType {
    Copper,
    Iron,
}

impl VeinType {
    /// Returns the ore block for this vein type.
    fn ore(self, blocks: &MaterialBlocks) -> BlockRuntimeId {
        match self {
            VeinType::Copper => blocks.copper_ore,
            VeinType::Iron => blocks.deepslate_iron_ore,
        }
    }

    fn raw_ore_block(self, blocks: &MaterialBlocks) -> BlockRuntimeId {
        match self {
            VeinType::Copper => blocks.raw_copper_block,
            VeinType::Iron => blocks.raw_iron_block,
        }
    }

    fn filler(self, blocks: &MaterialBlocks) -> BlockRuntimeId {
        match self {
            VeinType::Copper => blocks.granite,
            VeinType::Iron => blocks.tuff,
        }
    }

    /// Java: COPPER minY=0(L117),IRON minY=-60(L124).
    fn min_y(self) -> i32 {
        match self {
            VeinType::Copper => 0,
            VeinType::Iron => -60,
        }
    }

    /// Java: COPPER maxY=50(L118),IRON maxY=-8(L125).
    fn max_y(self) -> i32 {
        match self {
            VeinType::Copper => 50,
            VeinType::Iron => -8,
        }
    }
}

/// Java: `final class OreVeinifier`(L16-141).
pub struct OreVeinifier {
    vein_toggle: Arc<dyn DensityFunction>,
    vein_ridged: Arc<dyn DensityFunction>,
    vein_gap: Arc<dyn DensityFunction>,
    ore_vein_seed: i64,
    /// Reused random source (reset per position; initial seed does not matter).
    /// Reused random source (reset per position; initial seed does not matter). //
    random: RefCell<MtRandom>,
    blocks: MaterialBlocks,
}

impl OreVeinifier {
    /// Builds the veinifier with injected blocks.
    pub fn new(
        vein_toggle: Arc<dyn DensityFunction>,
        vein_ridged: Arc<dyn DensityFunction>,
        vein_gap: Arc<dyn DensityFunction>,
        ore_vein_seed: i64,
        blocks: MaterialBlocks,
    ) -> Self {
        Self {
            vein_toggle,
            vein_ridged,
            vein_gap,
            ore_vein_seed,
            random: RefCell::new(MtRandom::new(ore_vein_seed)),
            blocks,
        }
    }

    /// Java: `@Nullable BlockState calculate(FunctionContext)`(L48-91).
    pub fn calculate(&self, context: &dyn FunctionContext) -> Option<BlockRuntimeId> {
        let y = context.block_y();
        // Outside both copper and iron vein bands, no vein material can ever be placed.
        if !(MIN_VEIN_Y..=MAX_VEIN_Y).contains(&y) {
            return None;
        }

        let ore_veininess_noise_value = self.vein_toggle.compute(context);
        let vein_type = if ore_veininess_noise_value > 0.0 {
            VeinType::Copper
        } else {
            VeinType::Iron
        };
        let veininess_ridged = ore_veininess_noise_value.abs();
        let distance_from_top = vein_type.max_y() - y;
        let distance_from_bottom = y - vein_type.min_y();
        if distance_from_bottom < 0 || distance_from_top < 0 {
            return None;
        }

        let distance_from_edge = distance_from_top.min(distance_from_bottom);
        let edge_roundoff = clamped_map(
            distance_from_edge as f64,
            0.0,
            EDGE_ROUNDOFF_BEGIN as f64,
            -MAX_EDGE_ROUNDOFF,
            0.0,
        );
        if veininess_ridged + edge_roundoff < VEININESS_THRESHOLD as f64 {
            return None;
        }

        if self.vein_ridged.compute(context) >= 0.0 {
            return None;
        }

        let mut positional_random = self.random.borrow_mut();
        positional_random.set_seed(mix_seed(
            self.ore_vein_seed,
            context.block_x(),
            y,
            context.block_z(),
        ));
        if positional_random.next_float() > VEIN_SOLIDNESS {
            return None;
        }

        let richness = clamped_map(
            veininess_ridged,
            VEININESS_THRESHOLD as f64,
            MAX_RICHNESS_THRESHOLD as f64,
            MIN_RICHNESS as f64,
            MAX_RICHNESS as f64,
        );
        if positional_random.next_float() < richness as f32
            && self.vein_gap.compute(context) > SKIP_ORE_IF_GAP_NOISE_IS_BELOW as f64
        {
            return Some(
                if positional_random.next_float() < CHANCE_OF_RAW_ORE_BLOCK {
                    vein_type.raw_ore_block(&self.blocks)
                } else {
                    vein_type.ore(&self.blocks)
                },
            );
        }
        Some(vein_type.filler(&self.blocks))
    }
}

/// Delegates to the inherent calculate method.
impl MaterialFiller for OreVeinifier {
    fn calculate(&self, context: &dyn FunctionContext) -> Option<BlockRuntimeId> {
        self.calculate(context)
    }
}

/// Java: `private static double clampedMap(double, ..., double, ..., double, ..., double)`(L93-97).
fn clamped_map(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    let mut t = (value - in_min) / (in_max - in_min);
    t = clamp_f64(t, 0.0, 1.0);
    out_min + (out_max - out_min) * t
}

/// Java: `private static long mixSeed(long, int, int, int)`(L99-110).
fn mix_seed(seed: i64, x: i32, y: i32, z: i32) -> i64 {
    let mut mixed = seed;
    mixed ^= (x as i64).wrapping_mul(341873128712);
    mixed ^= (y as i64).wrapping_mul(132897987541);
    mixed ^= (z as i64).wrapping_mul(42317861);
    mixed ^= ((mixed as u64) >> 33) as i64;
    mixed = mixed.wrapping_mul(0xff51afd7ed558ccd_u64 as i64);
    mixed ^= ((mixed as u64) >> 33) as i64;
    mixed = mixed.wrapping_mul(0xc4ceb9fe1a85ec53_u64 as i64);
    mixed ^= ((mixed as u64) >> 33) as i64;
    mixed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::densityfunction::common::constant;
    use crate::worldgen::densityfunction::function::SinglePointContext;

    fn test_blocks() -> MaterialBlocks {
        MaterialBlocks {
            air: BlockRuntimeId(0),
            water: BlockRuntimeId(1),
            lava: BlockRuntimeId(2),
            stone: BlockRuntimeId(3),
            granite: BlockRuntimeId(4),
            tuff: BlockRuntimeId(5),
            copper_ore: BlockRuntimeId(6),
            deepslate_iron_ore: BlockRuntimeId(7),
            raw_copper_block: BlockRuntimeId(8),
            raw_iron_block: BlockRuntimeId(9),
        }
    }

    #[test]
    fn mix_seed_deterministic_and_position_sensitive() {
        let a = mix_seed(12345, 10, -20, 30);
        assert_eq!(a, mix_seed(12345, 10, -20, 30));
        assert_ne!(a, mix_seed(12345, 11, -20, 30));
        assert_ne!(a, mix_seed(12346, 10, -20, 30));
    }

    #[test]
    fn calculate_outside_vein_band_returns_none() {
        let veinifier = OreVeinifier::new(
            constant(0.5),
            constant(-1.0),
            constant(0.0),
            42,
            test_blocks(),
        );
        // y > MAX_VEIN_Y(50)
        assert!(veinifier
            .calculate(&SinglePointContext {
                block_x: 0,
                block_y: 51,
                block_z: 0,
            })
            .is_none());
        // y < MIN_VEIN_Y(-60)
        assert!(veinifier
            .calculate(&SinglePointContext {
                block_x: 0,
                block_y: -61,
                block_z: 0,
            })
            .is_none());
    }

    #[test]
    fn calculate_outside_vein_type_band_returns_none() {
        // veinToggle = 0.5 > 0 → Copper(minY=0, maxY=50)
        let veinifier = OreVeinifier::new(
            constant(0.5),
            constant(-1.0),
            constant(0.0),
            42,
            test_blocks(),
        );
        // Outside the copper band but inside the global vein band.
        assert!(veinifier
            .calculate(&SinglePointContext {
                block_x: 0,
                block_y: -10,
                block_z: 0,
            })
            .is_none());
    }

    #[test]
    fn calculate_deterministic_within_band() {
        let blocks = test_blocks();
        let v1 = OreVeinifier::new(
            constant(0.5),
            constant(-1.0),
            constant(0.0),
            42,
            test_blocks(),
        );
        let v2 = OreVeinifier::new(
            constant(0.5),
            constant(-1.0),
            constant(0.0),
            42,
            test_blocks(),
        );
        let ctx = SinglePointContext {
            block_x: 100,
            block_y: 25,
            block_z: -100,
        };
        let r1 = v1.calculate(&ctx);
        let r2 = v2.calculate(&ctx);
        assert_eq!(r1, r2);
        // Above threshold with negative ridged value: output is a copper-band block.
        // One of ore/raw/filler (None depends only on random rolls). Deterministic.
        if let Some(id) = r1 {
            assert!(
                id == blocks.copper_ore || id == blocks.raw_copper_block || id == blocks.granite
            );
        }
    }

    #[test]
    fn vein_ridged_nonnegative_returns_none() {
        let veinifier = OreVeinifier::new(
            constant(0.5),
            constant(0.0),
            constant(0.0),
            42,
            test_blocks(),
        );
        assert!(veinifier
            .calculate(&SinglePointContext {
                block_x: 0,
                block_y: 25,
                block_z: 0,
            })
            .is_none());
    }
}
