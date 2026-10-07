//! Port of the vanilla perlin noise package.
//!
//! - `PerlinNoiseSampler.java` → [`PerlinNoiseSampler`]
//! - `OctavePerlinNoiseSampler.java` → [`OctavePerlinNoiseSampler`]
//!
//! Reference sources live under `.fetch/`.
//!
//! Constructor overloads map to Rust names:
//! - `new(rand, octave_count)` ← `(RandomSourceProvider, int)`
//! - `new_from_octaves_iter(rand, octaves)` ← `(RandomSourceProvider, IntStream)`
//! - `new_with_first_octave(rand, first_octave, amplitudes)` ← `(RandomSourceProvider, int, List<Double>)`
//! - `new_with_octave_params(rand, octave_params)` ← `(RandomSourceProvider, Pair<Integer, List<Double>>)`
//! - `new_old(rand, octaves)` ← `(RandomSourceProvider, List<Integer>)` (old method)

use super::noise::{Noise, NoiseSampler, SKIP_262};
use crate::worldgen::math::{maintain_precision, Pair, Quad};
use crate::worldgen::random::RandomSourceProvider;

// ---------------------------------------------------------------------------
// PerlinNoiseSampler
// ---------------------------------------------------------------------------

/// Java: `PerlinNoiseSampler.FLAT_SIMPLEX_GRAD`(L8-25).
///
/// 16 gradient sets of 4 components (int literals widen to double,
/// 4th component always 0; sampling reads the first 3).
const FLAT_SIMPLEX_GRAD: [f64; 64] = [
    1.0, 1.0, 0.0, 0.0, //
    -1.0, 1.0, 0.0, 0.0, //
    1.0, -1.0, 0.0, 0.0, //
    -1.0, -1.0, 0.0, 0.0, //
    1.0, 0.0, 1.0, 0.0, //
    -1.0, 0.0, 1.0, 0.0, //
    1.0, 0.0, -1.0, 0.0, //
    -1.0, 0.0, -1.0, 0.0, //
    0.0, 1.0, 1.0, 0.0, //
    0.0, -1.0, 1.0, 0.0, //
    0.0, 1.0, -1.0, 0.0, //
    0.0, -1.0, -1.0, 0.0, //
    1.0, 1.0, 0.0, 0.0, //
    0.0, -1.0, 1.0, 0.0, //
    -1.0, 1.0, 0.0, 0.0, //
    0.0, -1.0, -1.0, 0.0, //
];

/// Java: `PerlinNoiseSampler extends Noise`.
///
/// Inherited sampler state (`originX/Y/Z`, `permutations`) via [`Noise`] composition.
pub struct PerlinNoiseSampler {
    /// Base sampler state (`originX/originY/originZ`, `permutations`).
    pub noise: Noise,
}

impl PerlinNoiseSampler {
    /// Java: `PerlinNoiseSampler(RandomSourceProvider rand)`(L27-29)——`super(rand)`.
    pub fn new<R: RandomSourceProvider>(rand: &mut R) -> Self {
        Self {
            noise: Noise::new(rand),
        }
    }

    /// Java: `sample(double, double, double, double, double)`(L31-48).
    pub fn sample(&self, x: f64, y: f64, z: f64, y_amplification: f64, min_y: f64) -> f64 {
        let offset_x = x + self.noise.origin_x;
        let offset_y = y + self.noise.origin_y;
        let offset_z = z + self.noise.origin_z;
        let floor_x = offset_x.floor();
        let floor_y = offset_y.floor();
        let floor_z = offset_z.floor();
        let local_x = offset_x - floor_x;
        let local_y = offset_y - floor_y;
        let local_z = offset_z - floor_z;
        let mut y_offset = 0.0;
        if y_amplification != 0.0 {
            // Java L43:minY >= 0 && minY < localY ? minY : localY
            let y_clamp = if min_y >= 0.0 && min_y < local_y {
                min_y
            } else {
                local_y
            };
            y_offset = (y_clamp / y_amplification + 1.0E-7).floor() * y_amplification;
        }

        // Saturating float-to-int conversion, matching `as i32`.
        self.sample_impl(
            floor_x as i32,
            floor_y as i32,
            floor_z as i32,
            local_x,
            local_y - y_offset,
            local_z,
            local_y,
        )
    }

    /// Java: `private double sample(int, int, int, double, double, double, double)`
    /// Local names `var0`..`var27` mirror the upstream decompilation.
    #[allow(clippy::too_many_arguments)]
    fn sample_impl(
        &self,
        section_x: i32,
        section_y: i32,
        section_z: i32,
        local_x: f64,
        local_y: f64,
        local_z: f64,
        fade_local_y: f64,
    ) -> f64 {
        let permutation = &self.noise.permutations;
        let var0 = section_x & 0xFF;
        let var1 = section_x.wrapping_add(1) & 0xFF;
        // `permutation[i] & 0xFF` reads the unsigned byte value;
        // u8 as i32 is naturally 0..=255.
        let var2 = permutation[var0 as usize] as i32;
        let var3 = permutation[var1 as usize] as i32;
        let var4 = var2.wrapping_add(section_y) & 0xFF;
        let var5 = var3.wrapping_add(section_y) & 0xFF;
        let var6 = var2.wrapping_add(section_y).wrapping_add(1) & 0xFF;
        let var7 = var3.wrapping_add(section_y).wrapping_add(1) & 0xFF;
        let var8 = permutation[var4 as usize] as i32;
        let var9 = permutation[var5 as usize] as i32;
        let var10 = permutation[var6 as usize] as i32;
        let var11 = permutation[var7 as usize] as i32;

        let var12 = var8.wrapping_add(section_z) & 0xFF;
        let var13 = var9.wrapping_add(section_z) & 0xFF;
        let var14 = var10.wrapping_add(section_z) & 0xFF;
        let var15 = var11.wrapping_add(section_z) & 0xFF;
        let var16 = var8.wrapping_add(section_z).wrapping_add(1) & 0xFF;
        let var17 = var9.wrapping_add(section_z).wrapping_add(1) & 0xFF;
        let var18 = var10.wrapping_add(section_z).wrapping_add(1) & 0xFF;
        let var19 = var11.wrapping_add(section_z).wrapping_add(1) & 0xFF;
        // `(permutation[i] & 15) << 2` indexes the gradient table.
        let var20 = ((permutation[var12 as usize] & 15) as usize) << 2;
        let var21 = ((permutation[var13 as usize] & 15) as usize) << 2;
        let var22 = ((permutation[var14 as usize] & 15) as usize) << 2;
        let var23 = ((permutation[var15 as usize] & 15) as usize) << 2;
        let var24 = ((permutation[var16 as usize] & 15) as usize) << 2;
        let var25 = ((permutation[var17 as usize] & 15) as usize) << 2;
        let var26 = ((permutation[var18 as usize] & 15) as usize) << 2;
        let var27 = ((permutation[var19 as usize] & 15) as usize) << 2;

        let x_minus_one = local_x - 1.0;
        let y_minus_one = local_y - 1.0;
        let z_minus_one = local_z - 1.0;
        let grad000 = FLAT_SIMPLEX_GRAD[var20] * local_x
            + FLAT_SIMPLEX_GRAD[var20 | 1] * local_y
            + FLAT_SIMPLEX_GRAD[var20 | 2] * local_z;
        let grad100 = FLAT_SIMPLEX_GRAD[var21] * x_minus_one
            + FLAT_SIMPLEX_GRAD[var21 | 1] * local_y
            + FLAT_SIMPLEX_GRAD[var21 | 2] * local_z;
        let grad010 = FLAT_SIMPLEX_GRAD[var22] * local_x
            + FLAT_SIMPLEX_GRAD[var22 | 1] * y_minus_one
            + FLAT_SIMPLEX_GRAD[var22 | 2] * local_z;
        let grad110 = FLAT_SIMPLEX_GRAD[var23] * x_minus_one
            + FLAT_SIMPLEX_GRAD[var23 | 1] * y_minus_one
            + FLAT_SIMPLEX_GRAD[var23 | 2] * local_z;
        let grad001 = FLAT_SIMPLEX_GRAD[var24] * local_x
            + FLAT_SIMPLEX_GRAD[var24 | 1] * local_y
            + FLAT_SIMPLEX_GRAD[var24 | 2] * z_minus_one;
        let grad101 = FLAT_SIMPLEX_GRAD[var25] * x_minus_one
            + FLAT_SIMPLEX_GRAD[var25 | 1] * local_y
            + FLAT_SIMPLEX_GRAD[var25 | 2] * z_minus_one;
        let grad011 = FLAT_SIMPLEX_GRAD[var26] * local_x
            + FLAT_SIMPLEX_GRAD[var26 | 1] * y_minus_one
            + FLAT_SIMPLEX_GRAD[var26 | 2] * z_minus_one;
        let grad111 = FLAT_SIMPLEX_GRAD[var27] * x_minus_one
            + FLAT_SIMPLEX_GRAD[var27 | 1] * y_minus_one
            + FLAT_SIMPLEX_GRAD[var27 | 2] * z_minus_one;

        let fade_x = local_x * local_x * local_x * (local_x * (local_x * 6.0 - 15.0) + 10.0);
        let fade_y = fade_local_y
            * fade_local_y
            * fade_local_y
            * (fade_local_y * (fade_local_y * 6.0 - 15.0) + 10.0);
        let fade_z = local_z * local_z * local_z * (local_z * (local_z * 6.0 - 15.0) + 10.0);

        let x00 = grad000 + fade_x * (grad100 - grad000);
        let x10 = grad010 + fade_x * (grad110 - grad010);
        let x01 = grad001 + fade_x * (grad101 - grad001);
        let x11 = grad011 + fade_x * (grad111 - grad011);
        let y0 = x00 + fade_y * (x10 - x00);
        let y1 = x01 + fade_y * (x11 - x01);
        y0 + fade_z * (y1 - y0)
    }
}

// ---------------------------------------------------------------------------
// OctavePerlinNoiseSampler
// ---------------------------------------------------------------------------

/// Java: `OctavePerlinNoiseSampler implements NoiseSampler`.
pub struct OctavePerlinNoiseSampler {
    /// Java: `public final double lacunarity`.
    pub lacunarity: f64,
    /// Java: `public final double persistence`.
    pub persistence: f64,
    /// Octave sampler array (entries may be null).
    octave_samplers: Vec<Option<PerlinNoiseSampler>>,
    /// Amplitude list (unread after construction, kept for parity).
    #[allow(dead_code)]
    amplitudes: Option<Vec<f64>>,
    /// Java: `private final double[] amplitudesArray`.
    amplitudes_array: Option<Vec<f64>>,
    /// Java: `private final int octaveSamplersCount`.
    octave_samplers_count: usize,
}

impl OctavePerlinNoiseSampler {
    /// Java: `OctavePerlinNoiseSampler(RandomSourceProvider, int)`(L22-32).
    pub fn new<R: RandomSourceProvider>(random: &mut R, octave_count: usize) -> Self {
        let mut octave_samplers: Vec<Option<PerlinNoiseSampler>> = Vec::with_capacity(octave_count);
        for _ in 0..octave_count {
            octave_samplers.push(Some(PerlinNoiseSampler::new(random)));
        }
        Self {
            lacunarity: 1.0,
            persistence: 1.0,
            octave_samplers_count: octave_samplers.len(),
            octave_samplers,
            amplitudes: None,
            amplitudes_array: None,
        }
    }

    /// Java: `getCount()`(L34-36).
    pub fn get_count(&self) -> usize {
        self.octave_samplers.len()
    }

    /// Java: `OctavePerlinNoiseSampler(RandomSourceProvider, IntStream)`(L38-40)
    /// Delegates to the `(RandomSourceProvider, List<Integer>)` constructor (old method).
    pub fn new_from_octaves_iter<R, I>(rand: &mut R, octaves: I) -> Self
    where
        R: RandomSourceProvider,
        I: IntoIterator<Item = i32>,
    {
        let octaves: Vec<i32> = octaves.into_iter().collect();
        Self::new_old(rand, &octaves)
    }

    /// Java: `makeAmplitudes(List<Integer>)`(L42-53).
    pub fn make_amplitudes(octaves: &[i32]) -> Pair<i32, Vec<f64>> {
        let processed = Self::process_octaves(octaves);
        let start = *processed.get_first();
        let length = *processed.get_third();
        let mut octave_places: Vec<f64> = Vec::with_capacity(length as usize);
        for _ in 0..length {
            octave_places.push(0.0);
        }
        for &octave in processed.get_fourth() {
            octave_places[octave.wrapping_add(start) as usize] = 1.0;
        }
        Pair::new(start, octave_places)
    }

    /// Java: `OctavePerlinNoiseSampler(RandomSourceProvider, int, List<Double>)`
    /// Delegates to the Pair constructor.
    pub fn new_with_first_octave<R: RandomSourceProvider>(
        rand: &mut R,
        first_octave: i32,
        amplitudes: Vec<f64>,
    ) -> Self {
        Self::new_with_octave_params(rand, Pair::new(first_octave, amplitudes))
    }

    /// Java: `OctavePerlinNoiseSampler(RandomSourceProvider, Pair<Integer, List<Double>>)`
    /// Nether variant constructor.
    pub fn new_with_octave_params<R: RandomSourceProvider>(
        rand: &mut R,
        octave_params: Pair<i32, Vec<f64>>,
    ) -> Self {
        // Field init order has no side effects here.
        let start = *octave_params.get_first();
        let amplitudes = octave_params.into_second();
        let perlin = PerlinNoiseSampler::new(rand);
        // Sampling is a pure function of construction-time state,
        // so precomputing it matches a later call bit for bit.
        let perlin_zero_sample = perlin.sample(0.0, 0.0, 0.0, 0.0, 0.0);

        let length = amplitudes.len();
        let mut octave_samplers: Vec<Option<PerlinNoiseSampler>> =
            (0..length).map(|_| None).collect();

        // Java L66-71
        if start >= 0 && (start as usize) < length {
            let d0 = amplitudes[start as usize];
            if d0 != 0.0 {
                octave_samplers[start as usize] = Some(perlin);
            }
        }

        // Indices run from start-1 down to 0 (empty when start <= 0).
        for idx in (0..start).rev() {
            if (idx as usize) < length {
                let d1 = amplitudes[idx as usize];
                if d1 != 0.0 {
                    octave_samplers[idx as usize] = Some(PerlinNoiseSampler::new(rand));
                } else {
                    rand.set_seed(SKIP_262.next_seed(rand.get_seed()));
                }
            } else {
                rand.set_seed(SKIP_262.next_seed(rand.get_seed()));
            }
        }

        // Java L86-102
        if start < length as i32 - 1 {
            // Java L87:`(long)(perlin.sample(...) * (double)9.223372E18F)`
            // (Note 9.223372E18F is a float literal, not exactly 2^63 as double.)
            let noise_seed = (perlin_zero_sample * 9.223372E18f32 as f64) as i64;
            rand.set_seed(noise_seed);

            for l in start.wrapping_add(1)..length as i32 {
                if l >= 0 {
                    let d2 = amplitudes[l as usize];
                    if d2 != 0.0 {
                        octave_samplers[l as usize] = Some(PerlinNoiseSampler::new(rand));
                    } else {
                        rand.set_seed(SKIP_262.next_seed(rand.get_seed()));
                    }
                } else {
                    rand.set_seed(SKIP_262.next_seed(rand.get_seed()));
                }
            }
        }

        // Java L104-107
        let persistence = 2.0f64.powi(-start);
        let lacunarity = 2.0f64.powi(length as i32 - 1) / (2.0f64.powi(length as i32) - 1.0);
        Self {
            lacunarity,
            persistence,
            octave_samplers,
            amplitudes: Some(amplitudes.clone()),
            amplitudes_array: Some(amplitudes),
            octave_samplers_count: length,
        }
    }

    /// Java: `processOctaves(List<Integer>)`(L110-124).
    fn process_octaves(octaves: &[i32]) -> Quad<i32, i32, i32, Vec<i32>> {
        // Works on a sorted copy (membership is order-independent).
        let mut sorted = octaves.to_vec();
        sorted.sort();

        if sorted.is_empty() {
            panic!("Need some octaves!");
        }

        let start = sorted[0].wrapping_neg();
        let end = sorted[sorted.len() - 1];
        let length = start.wrapping_add(end).wrapping_add(1);
        if length < 1 {
            panic!("Total number of octaves needs to be >= 1");
        }
        Quad::new(start, end, length, sorted)
    }

    /// Java: `OctavePerlinNoiseSampler(RandomSourceProvider, List<Integer>)`
    ///(L126-166,old method).
    pub fn new_old<R: RandomSourceProvider>(rand: &mut R, octaves: &[i32]) -> Self {
        let processed = Self::process_octaves(octaves);
        let end = *processed.get_second();
        let length = *processed.get_third();

        let perlin = PerlinNoiseSampler::new(rand);
        // Pure-function sampling, equivalent to precomputing (see above).
        let perlin_zero_sample = perlin.sample(0.0, 0.0, 0.0, 0.0, 0.0);

        let mut octave_samplers: Vec<Option<PerlinNoiseSampler>> =
            (0..length as usize).map(|_| None).collect();

        // Membership checks use the original octaves parameter.
        if end >= 0 && end < length && octaves.contains(&0) {
            octave_samplers[end as usize] = Some(perlin);
        }

        // Java L142-148
        for idx in end.wrapping_add(1)..length {
            if idx >= 0 && octaves.contains(&(end - idx)) {
                octave_samplers[idx as usize] = Some(PerlinNoiseSampler::new(rand));
            } else {
                rand.set_seed(SKIP_262.next_seed(rand.get_seed()));
            }
        }

        // Java L150-160
        if end > 0 {
            // Double literal ~= 2^63.
            let noise_seed = (perlin_zero_sample * 9.223372036854776E18f64) as i64;
            rand.set_seed(noise_seed);
            for index in (0..end).rev() {
                if index < length && octaves.contains(&(end - index)) {
                    octave_samplers[index as usize] = Some(PerlinNoiseSampler::new(rand));
                } else {
                    rand.set_seed(SKIP_262.next_seed(rand.get_seed()));
                }
            }
        }

        // Java L162-165
        let persistence = 2.0f64.powi(end);
        let lacunarity = 1.0 / (2.0f64.powi(length) - 1.0);
        let octave_samplers_count = octave_samplers.len();
        Self {
            lacunarity,
            persistence,
            octave_samplers,
            amplitudes: None,
            amplitudes_array: None,
            octave_samplers_count,
        }
    }

    /// Java: `sample(double, double, double)`(L168-170).
    pub fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        self.sample_full(x, y, z, 0.0, 0.0, false)
    }

    /// Java: `sample(double, double, double, double, double, boolean)`(L172-194).
    pub fn sample_full(
        &self,
        x: f64,
        y: f64,
        z: f64,
        y_amplification: f64,
        min_y: f64,
        use_default_y: bool,
    ) -> f64 {
        let mut noise = 0.0;
        // Each octave contributes to the final noise, decaying by a factor of 2.
        let mut persistence = self.persistence;
        // Octave spacing doubles per octave.
        let mut lacunarity = self.lacunarity;

        let amplitudes = &self.amplitudes_array;
        for idx in 0..self.octave_samplers_count {
            if let Some(sampler) = &self.octave_samplers[idx] {
                let sample = sampler.sample(
                    maintain_precision(x * persistence),
                    // Java L184:useDefaultY ? -sampler.originY : maintainPrecision(y * persistence)
                    if use_default_y {
                        -sampler.noise.origin_y
                    } else {
                        maintain_precision(y * persistence)
                    },
                    maintain_precision(z * persistence),
                    y_amplification * persistence,
                    min_y * persistence,
                ) * lacunarity;
                // Java L188:amplitudes != null ? amplitudes[idx] : 1.0
                noise += amplitudes.as_ref().map(|a| a[idx]).unwrap_or(1.0) * sample;
            }
            persistence /= 2.0;
            lacunarity *= 2.0;
        }
        noise
    }

    /// Octave lookup by index (out-of-range panics).
    pub fn get_octave(&self, octave: usize) -> Option<&PerlinNoiseSampler> {
        self.octave_samplers[octave].as_ref()
    }
}

impl NoiseSampler for OctavePerlinNoiseSampler {
    /// Java: `@Override sample(double, double, double, double)`(L212-215)
    /// Delegates to the full sample form with z=0.0.
    fn sample(&self, x: f64, y: f64, y_amplification: f64, min_y: f64) -> f64 {
        self.sample_full(x, y, 0.0, y_amplification, min_y, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::random::Xoroshiro128;

    #[test]
    fn perlin_sampler_period_256() {
        // Permutation indices wrap at 256 per axis. Exact in real arithmetic,
        // but float rounding differs across periods,
        // so tests use relative tolerance, not bitwise equality.
        let mut rand = Xoroshiro128::new(12345);
        let sampler = PerlinNoiseSampler::new(&mut rand);
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-10 * a.abs().max(1.0);
        for (x, y, z) in [(0.3, 1.7, 2.9), (-5.2, 0.0, 100.1), (63.9, -17.3, 0.5)] {
            let a = sampler.sample(x, y, z, 0.0, 0.0);
            assert!(
                close(a, sampler.sample(x + 256.0, y, z, 0.0, 0.0)),
                "x-axis period"
            );
            assert!(
                close(a, sampler.sample(x, y + 256.0, z, 0.0, 0.0)),
                "y-axis period"
            );
            assert!(
                close(a, sampler.sample(x, y, z + 256.0, 0.0, 0.0)),
                "z-axis period"
            );
        }
    }

    #[test]
    fn perlin_sampler_deterministic() {
        let mut rand1 = Xoroshiro128::new(42);
        let s1 = PerlinNoiseSampler::new(&mut rand1);
        let mut rand2 = Xoroshiro128::new(42);
        let s2 = PerlinNoiseSampler::new(&mut rand2);
        assert_eq!(s1.noise.origin_x, s2.noise.origin_x);
        assert_eq!(s1.noise.permutations, s2.noise.permutations);
        assert_eq!(
            s1.sample(1.25, 2.5, 3.75, 0.0, 0.0),
            s2.sample(1.25, 2.5, 3.75, 0.0, 0.0)
        );
        // yAmplification branch.
        let a = s1.sample(1.0, 2.0, 3.0, 0.5, 0.0);
        let b = s2.sample(1.0, 2.0, 3.0, 0.5, 0.0);
        assert_eq!(a, b);
        assert!(a.is_finite());
    }

    #[test]
    fn octave_make_amplitudes() {
        // sorted [-2, 0, 3]:start = 2,length = 2+3+1 = 6,
        // octave+start = 0/2/5 takes branch 1.
        let p = OctavePerlinNoiseSampler::make_amplitudes(&[-2, 0, 3]);
        assert_eq!(*p.get_first(), 2);
        assert_eq!(p.get_second(), &vec![1.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn octave_new_simple() {
        let mut rand = Xoroshiro128::new(3);
        let octave = OctavePerlinNoiseSampler::new(&mut rand, 4);
        assert_eq!(octave.get_count(), 4);
        assert_eq!(octave.persistence, 1.0);
        assert_eq!(octave.lacunarity, 1.0);
        for i in 0..4 {
            assert!(octave.get_octave(i).is_some());
        }
        assert!(octave.sample(1.0, 2.0, 3.0).is_finite());
    }

    #[test]
    fn octave_new_with_octave_params() {
        let amplitudes = vec![1.0, 1.0, 0.5, 0.0, 0.25];
        let mut rand = Xoroshiro128::new(99);
        let octave = OctavePerlinNoiseSampler::new_with_octave_params(
            &mut rand,
            Pair::new(2, amplitudes.clone()),
        );
        // start=2, length=5: [Some, Some, Some(shared), None(zero amplitude), Some].
        assert_eq!(octave.get_count(), 5);
        assert!(octave.get_octave(0).is_some());
        assert!(octave.get_octave(2).is_some());
        assert!(octave.get_octave(3).is_none(), "zero-amplitude octaves must be empty");
        assert!(octave.get_octave(4).is_some());
        assert_eq!(octave.persistence, 2.0f64.powi(-2));
        assert_eq!(octave.lacunarity, 2.0f64.powi(4) / (2.0f64.powi(5) - 1.0));
        let v = octave.sample(1.0, 2.0, 3.0);
        assert!(v.is_finite());
        // Same-seed determinism.
        let mut rand2 = Xoroshiro128::new(99);
        let octave2 =
            OctavePerlinNoiseSampler::new_with_octave_params(&mut rand2, Pair::new(2, amplitudes));
        assert_eq!(v, octave2.sample(1.0, 2.0, 3.0));
    }

    #[test]
    fn octave_new_with_first_octave_delegates() {
        let mut rand = Xoroshiro128::new(55);
        let a = OctavePerlinNoiseSampler::new_with_first_octave(&mut rand, 1, vec![1.0, 0.0, 1.0]);
        let mut rand2 = Xoroshiro128::new(55);
        let b = OctavePerlinNoiseSampler::new_with_octave_params(
            &mut rand2,
            Pair::new(1, vec![1.0, 0.0, 1.0]),
        );
        assert_eq!(a.persistence, b.persistence);
        assert_eq!(a.sample(2.0, 3.0, 4.0), b.sample(2.0, 3.0, 4.0));
    }

    #[test]
    fn octave_old_method() {
        // sorted [-3, 1, 2]:start=3, end=2, length=6
        // - contains(0)? No: the shared perlin is discarded (randomness consumed).
        // - idx 3..5: contains(2-idx) misses -> skip x2.
        // - end>0: noiseSeed -> indices 1..0 hit -> 2 samplers.
        let mut rand = Xoroshiro128::new(5);
        let octave = OctavePerlinNoiseSampler::new_old(&mut rand, &[-3, 1, 2]);
        assert_eq!(octave.get_count(), 6);
        assert!(octave.get_octave(0).is_some());
        assert!(octave.get_octave(1).is_some());
        assert!(octave.get_octave(2).is_none());
        assert_eq!(octave.persistence, 2.0f64.powi(2));
        assert_eq!(octave.lacunarity, 1.0 / (2.0f64.powi(6) - 1.0));
        // Same-seed determinism.
        let mut rand2 = Xoroshiro128::new(5);
        let octave2 = OctavePerlinNoiseSampler::new_old(&mut rand2, &[-3, 1, 2]);
        assert_eq!(
            octave.sample(1.5, -2.5, 3.5),
            octave2.sample(1.5, -2.5, 3.5)
        );
    }

    #[test]
    fn octave_new_from_octaves_iter_delegates() {
        let mut rand = Xoroshiro128::new(8);
        let a = OctavePerlinNoiseSampler::new_from_octaves_iter(&mut rand, [1, -2, 4]);
        let mut rand2 = Xoroshiro128::new(8);
        let b = OctavePerlinNoiseSampler::new_old(&mut rand2, &[1, -2, 4]);
        assert_eq!(a.get_count(), b.get_count());
        assert_eq!(a.sample(0.7, 1.3, 2.1), b.sample(0.7, 1.3, 2.1));
    }

    #[test]
    fn octave_trait_sample_matches_full() {
        let mut rand = Xoroshiro128::new(11);
        let octave = OctavePerlinNoiseSampler::new(&mut rand, 3);
        // NoiseSampler::sample(x, y, yAmp, minY) == sample_full(x, y, 0, yAmp, minY, false)
        let v = NoiseSampler::sample(&octave, 1.0, 2.0, 0.25, -64.0);
        assert_eq!(v, octave.sample_full(1.0, 2.0, 0.0, 0.25, -64.0, false));
        // useDefaultY branch works.
        let w = octave.sample_full(1.0, 2.0, 0.0, 0.0, 0.0, true);
        assert!(w.is_finite());
    }
}
