//! Noise sampler base types.
//!
//! - `Noise` is the shared base for Perlin/Simplex samplers.
//! - `NoiseSampler` is the sampler trait plus the [`SKIP_262`] constant.
//! - `PerlinNoise` is the multi-octave Perlin stack.
//! - `NormalNoise` is the two-stack normalized sampler.

use super::perlin::PerlinNoiseSampler;
use crate::worldgen::math::{maintain_precision, Lcg};
use crate::worldgen::random::RandomSourceProvider;

// ---------------------------------------------------------------------------
// NoiseSampler (interface as trait)
// ---------------------------------------------------------------------------

/// Skips 262 LCG steps (`combine(long)` is [`Lcg::combine_steps`]).
///
/// Interface constant in the source; hoisted to a module-level constant
/// for direct use.
pub const SKIP_262: Lcg = Lcg::JAVA.combine_steps(262);

/// Sampler interface.
pub trait NoiseSampler {
    /// Samples the noise value.
    fn sample(&self, x: f64, y: f64, y_amplification: f64, min_y: f64) -> f64;
}

// ---------------------------------------------------------------------------
// Noise (shared base via composition)
// ---------------------------------------------------------------------------

/// Shared noise base.
///
/// Base of the Perlin/Simplex samplers; expressed via composition here:
/// subclasses hold a `noise: Noise` field, accessed as
/// `sampler.noise.origin_y`.
pub struct Noise {
    /// Origin X.
    pub origin_x: f64,
    /// Origin Y.
    pub origin_y: f64,
    /// Origin Z.
    pub origin_z: f64,
    /// Shuffle permutation table.
    ///
    /// `u8` matches the low 8 bits of a signed byte (uses are unsigned
    /// after `& 0xFF` / `& 15`).
    pub(crate) permutations: [u8; 256],
}

impl Noise {
    /// Builds the base sampler.
    ///
    /// RNG consumption order: 3x `nextDouble` (origin), then 256x
    /// `nextBoundedInt` (shuffle).
    pub fn new<R: RandomSourceProvider>(rand: &mut R) -> Self {
        let origin_x = rand.next_double() * 256.0;
        let origin_y = rand.next_double() * 256.0;
        let origin_z = rand.next_double() * 256.0;

        let mut permutations = [0u8; 256];
        for j in 0..256 {
            permutations[j] = j as u8;
        }
        for index in 0..256usize {
            // Random index in `[index, 255]` (Fisher-Yates shuffle).
            let random_index = (rand.next_bounded_int(255 - index as i32) + index as i32) as usize;
            permutations.swap(index, random_index);
        }
        Self {
            origin_x,
            origin_y,
            origin_z,
            permutations,
        }
    }

    /// Looks up a permutation entry by hash.
    ///
    /// Caller is `SimplexNoiseSampler` ([`super::simplex`]).
    pub(crate) fn lookup(&self, hash: i32) -> i32 {
        self.permutations[(hash & 0xFF) as usize] as i32
    }
}

// ---------------------------------------------------------------------------
// PerlinNoise
// ---------------------------------------------------------------------------

/// Java: `PerlinNoise`.
pub struct PerlinNoise {
    /// Octave sampler array (entries may be null).
    noise_levels: Vec<Option<PerlinNoiseSampler>>,
    /// Java: `private final double[] amplitudes`.
    amplitudes: Vec<f64>,
    /// Java: `private final double lowestFreqValueFactor`.
    lowest_freq_value_factor: f64,
    /// Java: `private final double lowestFreqInputFactor`.
    lowest_freq_input_factor: f64,
    /// Java: `private final double maxValue`.
    max_value: f64,
}

impl PerlinNoise {
    /// Java: `PerlinNoise(RandomSourceProvider, int, double[])`(L14-43).
    pub fn new<R: RandomSourceProvider>(
        random: &mut R,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let octaves = amplitudes.len();
        let zero_octave_index = first_octave.wrapping_neg();
        let mut noise_levels: Vec<Option<PerlinNoiseSampler>> =
            (0..octaves).map(|_| None).collect();

        // The zero octave always constructs (consuming randomness), stored conditionally by amplitude.
        let zero_octave = PerlinNoiseSampler::new(random);
        if zero_octave_index >= 0 && (zero_octave_index as usize) < octaves {
            let amplitude = amplitudes[zero_octave_index as usize];
            if amplitude != 0.0 {
                noise_levels[zero_octave_index as usize] = Some(zero_octave);
            }
        }

        // Octaves below the zero index run downward to 0.
        for ix in (0..zero_octave_index).rev() {
            if (ix as usize) < octaves {
                if amplitudes[ix as usize] != 0.0 {
                    noise_levels[ix as usize] = Some(PerlinNoiseSampler::new(random));
                } else {
                    Self::skip_octave(random);
                }
            } else {
                Self::skip_octave(random);
            }
        }

        // Java L40-42.
        let lowest_freq_input_factor = 2.0f64.powi(-zero_octave_index);
        let lowest_freq_value_factor =
            2.0f64.powi(octaves as i32 - 1) / (2.0f64.powi(octaves as i32) - 1.0);
        let max_value = Self::edge_value(&noise_levels, amplitudes, lowest_freq_value_factor, 2.0);

        Self {
            noise_levels,
            amplitudes: amplitudes.to_vec(),
            lowest_freq_value_factor,
            lowest_freq_input_factor,
            max_value,
        }
    }

    /// Java: `private static void skipOctave(RandomSourceProvider)`(L45-47).
    fn skip_octave<R: RandomSourceProvider>(random: &mut R) {
        random.set_seed(SKIP_262.next_seed(random.get_seed()));
    }

    /// Java: `getValue(double, double, double)`(L49-72).
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut value = 0.0;
        let mut factor = self.lowest_freq_input_factor;
        let mut value_factor = self.lowest_freq_value_factor;

        for i in 0..self.noise_levels.len() {
            if let Some(noise) = &self.noise_levels[i] {
                let noise_value = noise.sample(
                    maintain_precision(x * factor),
                    maintain_precision(y * factor),
                    maintain_precision(z * factor),
                    0.0,
                    0.0,
                );
                value += self.amplitudes[i] * noise_value * value_factor;
            }

            factor *= 2.0;
            value_factor /= 2.0;
        }

        value
    }

    /// Java: `private double edgeValue(double noiseValue)`(L74-87).
    ///
    /// Rust passes equivalent state as parameters,
    /// called only during construction.
    fn edge_value(
        noise_levels: &[Option<PerlinNoiseSampler>],
        amplitudes: &[f64],
        lowest_freq_value_factor: f64,
        noise_value: f64,
    ) -> f64 {
        let mut value = 0.0;
        let mut value_factor = lowest_freq_value_factor;

        for i in 0..noise_levels.len() {
            if noise_levels[i].is_some() {
                value += amplitudes[i] * noise_value * value_factor;
            }
            value_factor /= 2.0;
        }

        value
    }

    /// Java: `maxValue()`(L89-91).
    pub fn max_value(&self) -> f64 {
        self.max_value
    }
}

// ---------------------------------------------------------------------------
// NormalNoise
// ---------------------------------------------------------------------------

/// Java: `NormalNoise.INPUT_FACTOR`(L7).
const INPUT_FACTOR: f64 = 1.0181268882175227;

/// Java: `NormalNoise`.
pub struct NormalNoise {
    /// Java: `private final PerlinNoise first`.
    first: PerlinNoise,
    /// Java: `private final PerlinNoise second`.
    second: PerlinNoise,
    /// Java: `private final double valueFactor`.
    value_factor: f64,
    /// Java: `private final double maxValue`.
    max_value: f64,
}

impl NormalNoise {
    /// Java: `NormalNoise(RandomSourceProvider, int, float[])`(L13-31).
    ///
    /// Parameters arrive as f32 and widen to f64 internally.
    pub fn new<R: RandomSourceProvider>(
        random: &mut R,
        first_octave: i32,
        amplitudes: &[f32],
    ) -> Self {
        let mut octave_amplitudes: Vec<f64> = Vec::with_capacity(amplitudes.len());
        let mut min = i32::MAX;
        let mut max = i32::MIN;

        for (i, &a) in amplitudes.iter().enumerate() {
            let amplitude = a as f64;
            octave_amplitudes.push(amplitude);
            if amplitude != 0.0 {
                min = min.min(i as i32);
                max = max.max(i as i32);
            }
        }

        let first = PerlinNoise::new(random, first_octave, &octave_amplitudes);
        let second = PerlinNoise::new(random, first_octave, &octave_amplitudes);
        // 1.0/6.0/expectedDeviation (left-associative).
        let value_factor = 1.0 / 6.0 / Self::expected_deviation(max.wrapping_sub(min));
        let max_value = (first.max_value() + second.max_value()) * value_factor;
        Self {
            first,
            second,
            value_factor,
            max_value,
        }
    }

    /// Java: `getValue(double, double, double)`(L33-38).
    ///
    /// First samples raw coordinates, second uses INPUT_FACTOR-scaled coordinates;
    /// the return narrows to f32 (round-to-nearest).
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f32 {
        let x2 = x * INPUT_FACTOR;
        let y2 = y * INPUT_FACTOR;
        let z2 = z * INPUT_FACTOR;
        ((self.first.get_value(x, y, z) + self.second.get_value(x2, y2, z2)) * self.value_factor)
            as f32
    }

    /// Java: `getMax()`(L40-42).
    pub fn get_max(&self) -> f64 {
        self.max_value
    }

    /// Java: `private static double expectedDeviation(int)`(L44-46).
    fn expected_deviation(octave_span: i32) -> f64 {
        0.1 * (1.0 + 1.0 / (octave_span as f64 + 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::random::Xoroshiro128;

    #[test]
    fn skip_262_equivalence() {
        // SKIP_262.nextSeed(s) must equal 262 LCG iterations from s.
        let mut expected = 12345i64;
        for _ in 0..262 {
            expected = Lcg::JAVA.next_seed(expected);
        }
        assert_eq!(SKIP_262.next_seed(12345), expected);
        // Const and runtime evaluation agree.
        let runtime = Lcg::JAVA.combine_steps(262);
        assert_eq!(SKIP_262.multiplier, runtime.multiplier);
        assert_eq!(SKIP_262.addend, runtime.addend);
    }

    #[test]
    fn noise_permutations_form_shuffle() {
        let mut rand = Xoroshiro128::new(2024);
        let noise = Noise::new(&mut rand);
        let mut seen = [false; 256];
        for &p in &noise.permutations {
            seen[p as usize] = true;
        }
        assert!(
            seen.iter().all(|&s| s),
            "permutations must be a 0..=255 permutation"
        );
        // Same-seed determinism.
        let mut rand2 = Xoroshiro128::new(2024);
        let noise2 = Noise::new(&mut rand2);
        assert_eq!(noise.permutations, noise2.permutations);
        assert_eq!(noise.origin_x, noise2.origin_x);
        assert_eq!(noise.origin_y, noise2.origin_y);
        assert_eq!(noise.origin_z, noise2.origin_z);
        // Lookup uses unsigned byte semantics.
        assert!((0..256).all(|h| (0..=255).contains(&noise.lookup(h))));
    }

    #[test]
    fn perlin_noise_deterministic_and_linear() {
        let mut r1 = Xoroshiro128::new(77);
        let n1 = PerlinNoise::new(&mut r1, -3, &[1.0, 1.0, 1.0, 0.0, 1.0]);
        let mut r2 = Xoroshiro128::new(77);
        let n2 = PerlinNoise::new(&mut r2, -3, &[1.0, 1.0, 1.0, 0.0, 1.0]);
        assert_eq!(n1.get_value(1.5, 2.5, 3.5), n2.get_value(1.5, 2.5, 3.5));
        assert_eq!(n1.max_value(), n2.max_value());

        // Halving the amplitude halves the output exactly.
        // (Zero/non-zero amplitude patterns share the construction sequence.)
        let mut r3 = Xoroshiro128::new(77);
        let n3 = PerlinNoise::new(&mut r3, -3, &[0.5, 0.5, 0.5, 0.0, 0.5]);
        assert_eq!(
            n3.get_value(1.5, 2.5, 3.5),
            0.5 * n1.get_value(1.5, 2.5, 3.5)
        );
        assert_eq!(n3.max_value(), 0.5 * n1.max_value());
    }

    #[test]
    fn normal_noise_bounds() {
        let mut rand = Xoroshiro128::new(123);
        let noise = NormalNoise::new(&mut rand, -5, &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        let max = noise.get_max();
        assert!(max > 0.0);
        // Samples stay within the theoretical bound.
        for i in 0..100 {
            let x = i as f64 * 0.37;
            let v = noise.get_value(x, x * 0.5, -x);
            assert!(v.is_finite());
            assert!(
                (v as f64).abs() <= max * 1.000001,
                "v={v} exceeds bound max={max}"
            );
        }
        // Determinism.
        let mut rand2 = Xoroshiro128::new(123);
        let noise2 = NormalNoise::new(&mut rand2, -5, &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(
            noise.get_value(10.0, 20.0, 30.0),
            noise2.get_value(10.0, 20.0, 30.0)
        );
    }
}
