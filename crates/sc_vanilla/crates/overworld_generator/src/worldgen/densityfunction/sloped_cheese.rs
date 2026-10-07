//! Sloped-cheese density combining depth, jaggedness, factor, and base noise.

use crate::worldgen::densityfunction::common::{
    add, constant, flat_cache, half_negative, mul, noise_scaled_xy, quarter_negative,
};
use crate::worldgen::densityfunction::function::DensityFunction;
use crate::worldgen::noise::noise::NormalNoise;
use std::sync::Arc;

/// Horizontal sampling scale for jaggedness.
const JAGGED_XZ_SCALE: f64 = 1500.0;
/// Vertical sampling scale for jaggedness.
const JAGGED_Y_SCALE: f64 = 0.0;

/// Builds the sloped-cheese density from depth, jaggedness, factor, and base noise.
pub fn overworld_sloped_cheese(
    depth: Arc<dyn DensityFunction>,
    jaggedness: Arc<dyn DensityFunction>,
    factor: Arc<dyn DensityFunction>,
    base3d_noise: Arc<dyn DensityFunction>,
    jagged_noise: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    let jagged_sample = half_negative(noise_scaled_xy(
        jagged_noise,
        JAGGED_XZ_SCALE,
        JAGGED_Y_SCALE,
    ));
    let combined_depth = add(depth, flat_cache(mul(jaggedness, jagged_sample)));

    add(
        mul(constant(4.0), quarter_negative(mul(combined_depth, factor))),
        base3d_noise,
    )
}
