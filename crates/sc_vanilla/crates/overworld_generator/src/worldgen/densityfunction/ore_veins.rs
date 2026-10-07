//! Ore-vein toggle, ridged, and gap densities.
//!
//! Vein Y bounds shared with the ore veinifier:
//! minimum -60 (iron), maximum 50 (copper).
//! Vein Y bounds shared with the ore veinifier.

use crate::worldgen::densityfunction::common::{
    abs, add, constant, interpolated, max, noise_scaled_xy, range_choice, y, zero,
};
use crate::worldgen::densityfunction::function::DensityFunction;
use crate::worldgen::noise::noise::NormalNoise;
use std::sync::Arc;

/// Java: `private static final double MIN_VEIN_Y = OreVeinifier.MIN_VEIN_Y`(L7).
const MIN_VEIN_Y: f64 = -60.0;
/// Java: `private static final double MAX_VEIN_Y_EXCLUSIVE = OreVeinifier.MAX_VEIN_Y + 1.0`(L8).
const MAX_VEIN_Y_EXCLUSIVE: f64 = 51.0;

/// Builds the vein-toggle density from veininess noise.
pub fn overworld_vein_toggle(ore_veininess: Arc<NormalNoise>) -> Arc<dyn DensityFunction> {
    interpolated(range_choice(
        y(),
        MIN_VEIN_Y,
        MAX_VEIN_Y_EXCLUSIVE,
        noise_scaled_xy(ore_veininess, 1.5, 1.5),
        zero(),
    ))
}

/// Builds the ridged vein density from the two vein noises.
pub fn overworld_vein_ridged(
    ore_vein_a: Arc<NormalNoise>,
    ore_vein_b: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    let vein_a = interpolated(range_choice(
        y(),
        MIN_VEIN_Y,
        MAX_VEIN_Y_EXCLUSIVE,
        noise_scaled_xy(ore_vein_a, 4.0, 4.0),
        zero(),
    ));
    let vein_b = interpolated(range_choice(
        y(),
        MIN_VEIN_Y,
        MAX_VEIN_Y_EXCLUSIVE,
        noise_scaled_xy(ore_vein_b, 4.0, 4.0),
        zero(),
    ));
    add(constant(-0.08), max(abs(vein_a), abs(vein_b)))
}

/// Builds the vein-gap density from gap noise.
pub fn overworld_vein_gap(ore_gap: Arc<NormalNoise>) -> Arc<dyn DensityFunction> {
    noise_scaled_xy(ore_gap, 1.0, 1.0)
}
