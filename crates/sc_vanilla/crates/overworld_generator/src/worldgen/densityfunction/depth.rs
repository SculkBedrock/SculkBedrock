//! Vertical depth gradient plus offset.

use crate::worldgen::densityfunction::common::{add, y_clamped_gradient};
use crate::worldgen::densityfunction::function::DensityFunction;
use std::sync::Arc;

/// Lower Y bound of the depth gradient.
const FROM_Y: i32 = -64;
/// Upper Y bound of the depth gradient.
const TO_Y: i32 = 320;
/// Density value at the lower bound.
const FROM_VALUE: f64 = 1.5;
/// Density value at the upper bound.
const TO_VALUE: f64 = -1.5;

/// Builds the depth function by adding the vertical gradient to the offset.
pub fn overworld_depth(offset: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    add(
        y_clamped_gradient(FROM_Y, TO_Y, FROM_VALUE, TO_VALUE),
        offset,
    )
}
