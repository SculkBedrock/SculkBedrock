//! Folded-ridges transform applied to the ridges density.

use crate::worldgen::densityfunction::common::{abs, add, constant, mul};
use crate::worldgen::densityfunction::function::DensityFunction;
use std::sync::Arc;

/// Builds the folded-ridges function from the ridges input.
pub fn overworld_ridges_folded(ridges: Arc<dyn DensityFunction>) -> Arc<dyn DensityFunction> {
    mul(
        constant(-3.0),
        add(
            constant(-0.3333333333333333),
            abs(add(constant(-0.6666666666666666), abs(ridges))),
        ),
    )
}
