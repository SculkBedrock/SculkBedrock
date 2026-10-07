//! Continents density from continentalness noise with shift.

use crate::worldgen::densityfunction::common::{flat_cache, shift_a, shift_b, zero, ShiftedNoise};
use crate::worldgen::densityfunction::function::{DensityFunction, NoiseHolder};
use crate::worldgen::noise::noise::NormalNoise;
use std::sync::Arc;

/// Java: `private static final double XZ_SCALE = 0.25`(L12).
const XZ_SCALE: f64 = 0.25;
/// Java: `private static final double Y_SCALE = 0.0`(L13).
const Y_SCALE: f64 = 0.0;

/// Java: `overworldContinents(NormalNoise, DensityFunction, DensityFunction)`(L18-33).
pub fn overworld_continents(
    continentalness: Arc<NormalNoise>,
    shift_x: Arc<dyn DensityFunction>,
    shift_z: Arc<dyn DensityFunction>,
) -> Arc<dyn DensityFunction> {
    flat_cache(Arc::new(ShiftedNoise {
        shift_x,
        shift_y: zero(),
        shift_z,
        xz_scale: XZ_SCALE,
        y_scale: Y_SCALE,
        noise: NoiseHolder::from_normal_noise(continentalness),
    }))
}

/// Java: `overworldContinents(NormalNoise, NormalNoise)`(L35-44).
///
/// Shift-noise convenience overload.
pub fn overworld_continents_with_shift_noise(
    continentalness: Arc<NormalNoise>,
    shift_noise: Arc<NormalNoise>,
) -> Arc<dyn DensityFunction> {
    overworld_continents(
        continentalness,
        shift_a(Arc::clone(&shift_noise)),
        shift_b(shift_noise),
    )
}
