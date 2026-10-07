//! Base 3D blended noise density.

use crate::worldgen::densityfunction::function::{
    ContextProvider, DensityFunction, FunctionContext,
};
use crate::worldgen::math::clamp_f64;
use crate::worldgen::noise::perlin::OctavePerlinNoiseSampler;
use crate::worldgen::random::RandomSourceProvider;
use std::any::Any;
use std::sync::Arc;

/// Java: `private static final double MAIN_NOISE_DIVISOR = 10.0`(L18).
const MAIN_NOISE_DIVISOR: f64 = 10.0;
/// Java: `private static final double LIMIT_NOISE_DIVISOR = 512.0`(L19).
const LIMIT_NOISE_DIVISOR: f64 = 512.0;
/// Java: `private static final double RESULT_DIVISOR = 128.0`(L20).
const RESULT_DIVISOR: f64 = 128.0;
/// Java: `private static final double BASE_SCALE = 684.412`(L21).
const BASE_SCALE: f64 = 684.412;

/// Java: `private static final List<Integer> LIMIT_OCTAVES = descendingOctaves(-15, 0)`(L22).
///
/// Descending octave list from -15 to 0.
const LIMIT_OCTAVES: [i32; 16] = [
    0, -1, -2, -3, -4, -5, -6, -7, -8, -9, -10, -11, -12, -13, -14, -15,
];
/// Java: `private static final List<Integer> MAIN_OCTAVES = descendingOctaves(-7, 0)`(L23).
const MAIN_OCTAVES: [i32; 8] = [0, -1, -2, -3, -4, -5, -6, -7];

/// Java: `oldBlendedNoise(RandomSourceProvider, double, double, double, double, double)`(L28-46).
///
/// Forks the random source for min-limit, max-limit, then main noise in order.
pub fn old_blended_noise<R: RandomSourceProvider>(
    random: &mut R,
    xz_scale: f64,
    y_scale: f64,
    xz_factor: f64,
    y_factor: f64,
    smear_scale_multiplier: f64,
) -> Arc<dyn DensityFunction> {
    let mut min_limit_fork = random.fork();
    let min_limit_noise = OctavePerlinNoiseSampler::new_old(&mut min_limit_fork, &LIMIT_OCTAVES);
    let mut max_limit_fork = random.fork();
    let max_limit_noise = OctavePerlinNoiseSampler::new_old(&mut max_limit_fork, &LIMIT_OCTAVES);
    let mut main_fork = random.fork();
    let main_noise = OctavePerlinNoiseSampler::new_old(&mut main_fork, &MAIN_OCTAVES);
    Arc::new(OldBlendedNoise {
        min_limit_noise,
        max_limit_noise,
        main_noise,
        xz_scale,
        y_scale,
        xz_factor,
        y_factor,
        smear_scale_multiplier,
    })
}

/// Java: `overworld(RandomSourceProvider)`(L48-50).
pub fn overworld<R: RandomSourceProvider>(random: &mut R) -> Arc<dyn DensityFunction> {
    old_blended_noise(random, 0.25, 0.125, 80.0, 160.0, 8.0)
}

/// Java: `nether(RandomSourceProvider)`(L52-54).
pub fn nether<R: RandomSourceProvider>(random: &mut R) -> Arc<dyn DensityFunction> {
    old_blended_noise(random, 0.25, 0.375, 80.0, 60.0, 8.0)
}

/// Java: `private record OldBlendedNoise(...) implements DensityFunction`(L63-156).
pub struct OldBlendedNoise {
    pub min_limit_noise: OctavePerlinNoiseSampler,
    pub max_limit_noise: OctavePerlinNoiseSampler,
    pub main_noise: OctavePerlinNoiseSampler,
    pub xz_scale: f64,
    pub y_scale: f64,
    pub xz_factor: f64,
    pub y_factor: f64,
    pub smear_scale_multiplier: f64,
}

impl DensityFunction for OldBlendedNoise {
    /// Java: `compute(FunctionContext)`(L74-136).
    fn compute(&self, context: &dyn FunctionContext) -> f64 {
        let scaled_xz = BASE_SCALE * self.xz_scale;
        let scaled_y = BASE_SCALE * self.y_scale;
        let main_scaled_xz = scaled_xz / self.xz_factor;
        let main_scaled_y = scaled_y / self.y_factor;
        let smear_scale = scaled_y * self.smear_scale_multiplier;

        let x = context.block_x() as f64;
        let y = context.block_y() as f64;
        let z = context.block_z() as f64;

        let mut main_value = 0.0;
        let mut frequency = 1.0;
        for octave in 0..self.main_noise.get_count() {
            if let Some(sampler) = self.main_noise.get_octave(octave) {
                main_value += sampler.sample(
                    x * main_scaled_xz * frequency,
                    y * main_scaled_y * frequency,
                    z * main_scaled_xz * frequency,
                    smear_scale * frequency,
                    y * main_scaled_y * frequency,
                ) / frequency;
            }
            frequency /= 2.0;
        }

        // Clamps the blended main value to [0, 2] scaled by 0.5.
        let blend = clamp_f64(main_value / MAIN_NOISE_DIVISOR + 1.0, 0.0, 2.0) * 0.5;
        let use_only_max = blend >= 1.0;
        let use_only_min = blend <= 0.0;

        let mut min_value = 0.0;
        let mut max_value = 0.0;
        let mut frequency = 1.0;

        let octave_count = self
            .min_limit_noise
            .get_count()
            .min(self.max_limit_noise.get_count());
        for octave in 0..octave_count {
            let sample_x = x * scaled_xz * frequency;
            let sample_y = y * scaled_y * frequency;
            let sample_z = z * scaled_xz * frequency;
            let sample_smear = smear_scale * frequency;

            if !use_only_max {
                if let Some(sampler) = self.min_limit_noise.get_octave(octave) {
                    min_value +=
                        sampler.sample(sample_x, sample_y, sample_z, sample_smear, sample_y)
                            / frequency;
                }
            }

            if !use_only_min {
                if let Some(sampler) = self.max_limit_noise.get_octave(octave) {
                    max_value +=
                        sampler.sample(sample_x, sample_y, sample_z, sample_smear, sample_y)
                            / frequency;
                }
            }

            frequency /= 2.0;
        }

        let lower = min_value / LIMIT_NOISE_DIVISOR;
        let upper = max_value / LIMIT_NOISE_DIVISOR;
        lerp(lower, upper, blend) / RESULT_DIVISOR
    }

    /// Java: `fillArray`(L139-141).
    fn fill_array(&self, output: &mut [f64], context_provider: &dyn ContextProvider) {
        context_provider.fill_all_directly(output, self);
    }

    /// Java: `minValue()`(L144-146).
    fn min_value(&self) -> f64 {
        -1.5
    }

    /// Java: `maxValue()`(L148-150).
    fn max_value(&self) -> f64 {
        1.5
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Java: `private static double lerp(double, double, double)`(L153-155).
fn lerp(first: f64, second: f64, alpha: f64) -> f64 {
    first + alpha * (second - first)
}
