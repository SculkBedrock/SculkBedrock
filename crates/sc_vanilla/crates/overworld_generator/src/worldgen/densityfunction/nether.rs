//! Nether final density with vertical clamps and blending.

use crate::worldgen::densityfunction::common::{
    add, blend_density, constant, interpolated, mul, squeeze, y_clamped_gradient,
};
use crate::worldgen::densityfunction::function::DensityFunction;
use std::sync::Arc;

/// Builds the nether final density from base 3D noise.
pub fn final_density(base3d_noise: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    let density = add(constant(-0.9375), base3d_noise);
    let density = mul(y_clamped_gradient(104, 128, 1.0, 0.0), density);
    let density = add(constant(0.9375), density);
    let density = add(constant(-2.5), density);
    let density = mul(y_clamped_gradient(-8, 24, 0.0, 1.0), density);
    let density = add(constant(2.5), density);
    let density = blend_density(density);
    let density = interpolated(density);
    let density = mul(constant(0.64), density);
    squeeze(density)
}
