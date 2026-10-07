//! Port of the vanilla simplex noise package.
//!
//! - `SimplexNoiseSampler.java` → [`SimplexNoiseSampler`]
//! - `OctaveSimplexNoiseSampler.java` → [`OctaveSimplexNoiseSampler`]
//! - `SimplexNoise.java` → [`SimplexNoise`]
//!
//! Reference sources live under `.fetch/`.
//!
//! Constructor overloads map to Rust names (OctaveSimplexNoiseSampler):
//! - `new(rand, octave_count)` ← `(RandomSourceProvider, int)`
//! - `new_from_octaves_iter(rand, octaves)` ← `(RandomSourceProvider, IntStream)`
//! - `new_with_octaves(rand, octaves)` ← `(RandomSourceProvider, List<Integer>)`

use super::noise::{Noise, NoiseSampler, SKIP_262};
use crate::worldgen::math::{floor, grad};
use crate::worldgen::random::{MtRandom, RandomSourceProvider};

// ---------------------------------------------------------------------------
// SimplexNoiseSampler
// ---------------------------------------------------------------------------

/// Correctly-rounded `sqrt(3.0)` value.
///
/// (Matches the std constant bit for bit.)
const SQRT_3: f64 = 1.7320508075688772;
/// Java: `SKEW_FACTOR_2D`(L12,also known as F2).
const SKEW_FACTOR_2D: f64 = 0.5 * (SQRT_3 - 1.0);
/// Java: `UNSKEW_FACTOR_2D`(L13,also known as G2).
const UNSKEW_FACTOR_2D: f64 = (3.0 - SQRT_3) / 6.0;
/// Java: `F3`(L14).
const F3: f64 = 0.3333333333333333;
/// Java: `G3`(L15).
const G3: f64 = 0.16666666666666666;

/// Java: `SimplexNoiseSampler extends Noise`.
///
/// Inherited sampler state via [`Noise`] composition.
pub struct SimplexNoiseSampler {
    /// Base sampler state.
    pub noise: Noise,
}

impl SimplexNoiseSampler {
    /// Java: `SimplexNoiseSampler(RandomSourceProvider rand)`(L17-19)——`super(rand)`.
    pub fn new<R: RandomSourceProvider>(rand: &mut R) -> Self {
        Self {
            noise: Noise::new(rand),
        }
    }

    /// Java: `sample2D(double, double)`(L21-54).
    pub fn sample_2d(&self, x: f64, y: f64) -> f64 {
        let hairy_factor = (x + y) * SKEW_FACTOR_2D;
        // in minecraft those are the temperatures
        let hairy_x = floor(x + hairy_factor);
        let hairy_z = floor(y + hairy_factor);
        let mixed_hairy_xz = (hairy_x + hairy_z) as f64 * UNSKEW_FACTOR_2D;
        let diff_x_to_xz = hairy_x as f64 - mixed_hairy_xz;
        let diff_z_to_xz = hairy_z as f64 - mixed_hairy_xz;
        let x0 = x - diff_x_to_xz;
        let y0 = y - diff_z_to_xz;
        // Java byte(0/1),Rust i32.
        let (offset_second_corner_x, offset_second_corner_z): (i32, i32) = if x0 > y0 {
            // lower triangle, XY order: (0,0)->(1,0)->(1,1)
            (1, 0)
        } else {
            // upper triangle, YX order: (0,0)->(0,1)->(1,1)
            (0, 1)
        };

        let x1 = x0 - offset_second_corner_x as f64 + UNSKEW_FACTOR_2D;
        let y1 = y0 - offset_second_corner_z as f64 + UNSKEW_FACTOR_2D;
        let x3 = x0 - 1.0 + 2.0 * UNSKEW_FACTOR_2D;
        let y3 = y0 - 1.0 + 2.0 * UNSKEW_FACTOR_2D;
        let ii = hairy_x & 255;
        let jj = hairy_z & 255;
        // Lookup and lattice indices stay non-negative, so `% 12` matches.
        let gi0 = self.noise.lookup(ii + self.noise.lookup(jj)) % 12;
        let gi1 = self
            .noise
            .lookup(ii + offset_second_corner_x + self.noise.lookup(jj + offset_second_corner_z))
            % 12;
        let gi2 = self.noise.lookup(ii + 1 + self.noise.lookup(jj + 1)) % 12;
        let t0 = self.corner_noise_3d(gi0, x0, y0, 0.0, 0.5);
        let t1 = self.corner_noise_3d(gi1, x1, y1, 0.0, 0.5);
        let t2 = self.corner_noise_3d(gi2, x3, y3, 0.0, 0.5);
        70.0 * (t0 + t1 + t2)
    }

    /// Java: `sample3D(double, double, double)`(L56-138).
    pub fn sample_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        let skew_factor = (x + y + z) * F3; // F3 is 1/3
                                            // Skew the input space to determine which simplex cell we're in
        let i = floor(x + skew_factor);
        let j = floor(y + skew_factor);
        let k = floor(z + skew_factor);
        let unskew_factor = (i + j + k) as f64 * G3; // G3 is 1/6
        let x0 = i as f64 - unskew_factor;
        let y0 = j as f64 - unskew_factor;
        let z0 = k as f64 - unskew_factor;
        let x0 = x - x0;
        let y0 = y - y0;
        let z0 = z - z0;
        // Java byte(0/1),Rust i32.
        let (i1, j1, k1, i2, j2, k2): (i32, i32, i32, i32, i32, i32) = if x0 >= y0 {
            if y0 >= z0 {
                // X Y Z order
                (1, 0, 0, 1, 1, 0)
            } else if x0 >= z0 {
                // X Z Y order
                (1, 0, 0, 1, 0, 1)
            } else {
                // Z X Y order
                (0, 0, 1, 1, 0, 1)
            }
        } else if y0 < z0 {
            // Z Y X order
            (0, 0, 1, 0, 1, 1)
        } else if x0 < z0 {
            // Y Z X order
            (0, 1, 0, 0, 1, 1)
        } else {
            // Y X Z order
            (0, 1, 0, 1, 1, 0)
        };

        let x1 = x0 - i1 as f64 + G3;
        let y1 = y0 - j1 as f64 + G3;
        let z1 = z0 - k1 as f64 + G3;
        let x2 = x0 - i2 as f64 + F3;
        let y2 = y0 - j2 as f64 + F3;
        let z2 = z0 - k2 as f64 + F3;
        let x3 = x0 - 1.0 + 0.5;
        let y3 = y0 - 1.0 + 0.5;
        let z3 = z0 - 1.0 + 0.5;
        let ii = i & 255;
        let jj = j & 255;
        let kk = k & 255;
        let gi0 = self
            .noise
            .lookup(ii + self.noise.lookup(jj + self.noise.lookup(kk)))
            % 12;
        let gi1 = self
            .noise
            .lookup(ii + i1 + self.noise.lookup(jj + j1 + self.noise.lookup(kk + k1)))
            % 12;
        let gi2 = self
            .noise
            .lookup(ii + i2 + self.noise.lookup(jj + j2 + self.noise.lookup(kk + k2)))
            % 12;
        let gi3 = self
            .noise
            .lookup(ii + 1 + self.noise.lookup(jj + 1 + self.noise.lookup(kk + 1)))
            % 12;
        let t0 = self.corner_noise_3d(gi0, x0, y0, z0, 0.6);
        let t1 = self.corner_noise_3d(gi1, x1, y1, z1, 0.6);
        let t2 = self.corner_noise_3d(gi2, x2, y2, z2, 0.6);
        let t3 = self.corner_noise_3d(gi3, x3, y3, z3, 0.6);
        32.0 * (t0 + t1 + t2 + t3)
    }

    /// Java: `private double cornerNoise3d(int, double, double, double, double)`
    ///(L140-151).
    fn corner_noise_3d(&self, hash: i32, x: f64, y: f64, z: f64, max: f64) -> f64 {
        let mut contribution = max - x * x - y * y - z * z;
        if contribution < 0.0 {
            0.0
        } else {
            contribution *= contribution;
            contribution * contribution * grad(hash, x, y, z)
        }
    }
}

// ---------------------------------------------------------------------------
// OctaveSimplexNoiseSampler
// ---------------------------------------------------------------------------

/// Java: `OctaveSimplexNoiseSampler implements NoiseSampler`.
pub struct OctaveSimplexNoiseSampler {
    /// Java: `public final double lacunarity`.
    pub lacunarity: f64,
    /// Java: `public final double persistence`.
    pub persistence: f64,
    /// Octave sampler array (entries may be null).
    octave_samplers: Vec<Option<SimplexNoiseSampler>>,
}

impl OctaveSimplexNoiseSampler {
    /// Java: `OctaveSimplexNoiseSampler(RandomSourceProvider, int)`(L16-23).
    pub fn new<R: RandomSourceProvider>(random: &mut R, octave_count: usize) -> Self {
        let mut octave_samplers: Vec<Option<SimplexNoiseSampler>> =
            Vec::with_capacity(octave_count);
        for _ in 0..octave_count {
            octave_samplers.push(Some(SimplexNoiseSampler::new(random)));
        }
        Self {
            lacunarity: 1.0,
            persistence: 1.0,
            octave_samplers,
        }
    }

    /// Java: `OctaveSimplexNoiseSampler(RandomSourceProvider, IntStream)`(L25-27)
    /// Delegates to the `(RandomSourceProvider, List<Integer>)` constructor.
    pub fn new_from_octaves_iter<R, I>(rand: &mut R, octaves: I) -> Self
    where
        R: RandomSourceProvider,
        I: IntoIterator<Item = i32>,
    {
        let octaves: Vec<i32> = octaves.into_iter().collect();
        Self::new_with_octaves(rand, &octaves)
    }

    /// Java: `OctaveSimplexNoiseSampler(RandomSourceProvider, List<Integer>)`
    ///(L29-73).
    pub fn new_with_octaves<R: RandomSourceProvider>(rand: &mut R, octaves: &[i32]) -> Self {
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

        let simplex = SimplexNoiseSampler::new(rand);
        // Sampling is a pure function of construction-time state,
        // so precomputing it matches a later call bit for bit.
        // Sampling is a pure function of construction-time state.
        let simplex_origin_sample = simplex.sample_3d(
            simplex.noise.origin_x,
            simplex.noise.origin_y,
            simplex.noise.origin_z,
        );

        let mut octave_samplers: Vec<Option<SimplexNoiseSampler>> =
            (0..length as usize).map(|_| None).collect();

        // Java L47-49
        if end >= 0 && end < length && octaves.contains(&0) {
            octave_samplers[end as usize] = Some(simplex);
        }

        // Indices may go negative (end < -1); negative indices only skip seeds.
        for idx in end.wrapping_add(1)..length {
            if idx >= 0 && octaves.contains(&(end.wrapping_sub(idx))) {
                octave_samplers[idx as usize] = Some(SimplexNoiseSampler::new(rand));
            } else {
                rand.set_seed(SKIP_262.next_seed(rand.get_seed()));
            }
        }

        // Java L59-69
        if end > 0 {
            // Double literal ~= 2^63.
            let noise_seed = (simplex_origin_sample * 9.223372036854776E18f64) as i64;
            rand.set_seed(noise_seed);
            for index in (0..end).rev() {
                if index < length && octaves.contains(&(end.wrapping_sub(index))) {
                    octave_samplers[index as usize] = Some(SimplexNoiseSampler::new(rand));
                } else {
                    rand.set_seed(SKIP_262.next_seed(rand.get_seed()));
                }
            }
        }

        // Java L71-72
        let persistence = 2.0f64.powi(end);
        let lacunarity = 1.0 / (2.0f64.powi(length) - 1.0);
        Self {
            lacunarity,
            persistence,
            octave_samplers,
        }
    }

    /// Java: `sample(double, double)`(L75-77).
    pub fn sample(&self, x: f64, y: f64) -> f64 {
        self.sample_with_offset(x, y, false)
    }

    /// Java: `sample(double, double, boolean)`(L79-96).
    pub fn sample_with_offset(&self, x: f64, y: f64, use_random_offset: bool) -> f64 {
        let mut noise = 0.0;
        // Each octave contributes to the final noise, decaying by a factor of 2.
        let mut persistence = self.persistence;
        let mut lacunarity = self.lacunarity;
        for sampler in &self.octave_samplers {
            if let Some(sampler) = sampler {
                noise += sampler.sample_2d(
                    x * persistence
                        + if use_random_offset {
                            sampler.noise.origin_x
                        } else {
                            0.0
                        },
                    y * persistence
                        + if use_random_offset {
                            sampler.noise.origin_y
                        } else {
                            0.0
                        },
                ) * lacunarity;
            }
            persistence /= 2.0;
            lacunarity *= 2.0;
        }
        noise
    }
}

impl NoiseSampler for OctaveSimplexNoiseSampler {
    /// Java: `@Override sample(double, double, double, double)`(L98-101)
    /// ——`sample(x, y, true) * 0.55D`.
    fn sample(&self, x: f64, y: f64, _not_used: f64, _not_used2: f64) -> f64 {
        self.sample_with_offset(x, y, true) * 0.55
    }
}

// ---------------------------------------------------------------------------
// SimplexNoise
// ---------------------------------------------------------------------------

/// `String.hashCode()` of `"octave_" + int` strings.
///
/// `h = 31*h + c` over UTF-16 units with i32 wrapping; all strings here are ASCII,
/// so `chars()` matches unit by unit.
fn java_string_hash(s: &str) -> i32 {
    let mut h: i32 = 0;
    for c in s.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as i32);
    }
    h
}

/// Java: `SimplexNoise`.
pub struct SimplexNoise {
    /// Java: `private final float[] amplitudes`.
    amplitudes: Vec<f32>,
    /// Octave sampler array (entries may be null).
    noise_levels: Vec<Option<SimplexNoiseSampler>>,
    /// Level count (unread after construction, kept for parity).
    #[allow(dead_code)]
    levels: usize,
    /// Java: `private final float lowestFreqValueFactor`.
    lowest_freq_value_factor: f32,
    /// Java: `private final float lowestFreqInputFactor`.
    lowest_freq_input_factor: f32,
    /// Java: `private final float maxValue`.
    max_value: f32,
}

impl SimplexNoise {
    /// Port of the `(RandomSourceProvider, int, float[])` constructor.
    ///
    /// Note: each non-zero-amplitude octave consumes one `random.nextLong()`, seeding
    /// an `MtRandom` child source with `nextLong + hashCode`.
    pub fn new<R: RandomSourceProvider>(
        random: &mut R,
        first_octave: i32,
        amplitudes: &[f32],
    ) -> Self {
        let levels = amplitudes.len();
        let mut noise_levels: Vec<Option<SimplexNoiseSampler>> =
            (0..levels).map(|_| None).collect();
        // Java L21:`(float) Math.pow(2, firstOctave)`.
        let lowest_freq_input_factor = 2.0f64.powi(first_octave) as f32;
        // Java L22:`(float)(Math.pow(2, levels-1) / (Math.pow(2, levels) - 1))`.
        let lowest_freq_value_factor =
            (2.0f64.powi(levels as i32 - 1) / (2.0f64.powi(levels as i32) - 1.0)) as f32;

        // Java L23-27
        for i in 0..levels {
            if amplitudes[i] != 0.0 {
                // Java:`random.nextLong() + ("octave_" + (firstOctave + i)).hashCode()`
                // (hashCode returns int, sign-extended to long; i32 addition wraps).
                let seed = random.next_long().wrapping_add(java_string_hash(&format!(
                    "octave_{}",
                    first_octave.wrapping_add(i as i32)
                )) as i64);
                let mut octave_rand = MtRandom::new(seed);
                noise_levels[i] = Some(SimplexNoiseSampler::new(&mut octave_rand));
            }
        }

        let max_value = Self::edge_value(&noise_levels, amplitudes, lowest_freq_value_factor, 2.0);

        Self {
            amplitudes: amplitudes.to_vec(),
            noise_levels,
            levels,
            lowest_freq_value_factor,
            lowest_freq_input_factor,
            max_value,
        }
    }

    /// Java: `getValue(double, double, double)`(L31-53).
    ///
    /// Accumulators use f32 arithmetic; `x * d1` in `wrap` widens
    /// double-times-float to double.
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut d0 = 0.0f32;
        let mut d1 = self.lowest_freq_input_factor;
        let mut d2 = self.lowest_freq_value_factor;

        for i in 0..self.noise_levels.len() {
            if let Some(noise) = &self.noise_levels[i] {
                let d3 = noise.sample_3d(
                    Self::wrap(x * d1 as f64) as f64,
                    Self::wrap(y * d1 as f64) as f64,
                    Self::wrap(z * d1 as f64) as f64,
                ) as f32;
                d0 += self.amplitudes[i] * d3 * d2;
            }

            d1 *= 2.0;
            d2 /= 2.0;
        }

        d0
    }

    /// Java: `public static float wrap(double)`(L56-58).
    pub fn wrap(v: f64) -> f32 {
        (v - (v / 3.3554432E7 + 0.5).floor() * 3.3554432E7) as f32
    }

    /// Java: `private float edgeValue(double)`(L60-72).
    ///
    /// `cumulativeSum += amplitudes[i] * input * factor` narrows the double
    /// right-hand side back to float.
    fn edge_value(
        noise_levels: &[Option<SimplexNoiseSampler>],
        amplitudes: &[f32],
        lowest_freq_value_factor: f32,
        input: f64,
    ) -> f32 {
        let mut cumulative_sum = 0.0f32;
        let mut octave_contribution_factor = lowest_freq_value_factor;

        for i in 0..noise_levels.len() {
            if noise_levels[i].is_some() {
                let term = amplitudes[i] as f64 * input * octave_contribution_factor as f64;
                cumulative_sum = (cumulative_sum as f64 + term) as f32;
            }
            octave_contribution_factor /= 2.0;
        }
        cumulative_sum
    }

    /// Java: `getMax()`(L74-76).
    pub fn get_max(&self) -> f32 {
        self.max_value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::random::Xoroshiro128;

    // "hello".hashCode() == 99162322 (known value, validates the base algorithm).
    #[test]
    fn java_string_hash_known_values() {
        assert_eq!(java_string_hash(""), 0);
        assert_eq!(java_string_hash("hello"), 99162322);
        // Covers the i32 overflow wrapping path.
        assert_eq!(java_string_hash("octave_0"), 1261148513);
    }

    #[test]
    fn simplex_sampler_deterministic() {
        let mut rand1 = Xoroshiro128::new(42);
        let s1 = SimplexNoiseSampler::new(&mut rand1);
        let mut rand2 = Xoroshiro128::new(42);
        let s2 = SimplexNoiseSampler::new(&mut rand2);
        assert_eq!(s1.noise.origin_x, s2.noise.origin_x);
        assert_eq!(s1.noise.permutations, s2.noise.permutations);
        assert_eq!(s1.sample_2d(1.25, 2.5), s2.sample_2d(1.25, 2.5));
        assert_eq!(s1.sample_3d(1.25, 2.5, 3.75), s2.sample_3d(1.25, 2.5, 3.75));
    }

    #[test]
    fn simplex_sampler_bounds_and_smoothness() {
        let mut rand = Xoroshiro128::new(7);
        let s = SimplexNoiseSampler::new(&mut rand);
        let mut max2 = 0.0f64;
        let mut max3 = 0.0f64;
        for i in 0..200 {
            let x = i as f64 * 0.137 - 10.0;
            let y = i as f64 * 0.271 - 5.0;
            let z = i as f64 * 0.419 + 3.0;
            let v2 = s.sample_2d(x, y);
            let v3 = s.sample_3d(x, y, z);
            assert!(v2.is_finite() && v3.is_finite());
            max2 = max2.max(v2.abs());
            max3 = max3.max(v3.abs());
        }
        // Simplex noise stays within |v| <= ~1 (loose regression bound).
        assert!(max2 <= 1.0, "2D amplitude out of range: {max2}");
        assert!(max3 <= 1.0, "3D amplitude out of range: {max3}");
        // Smoothness: small perturbations change little.
        let a = s.sample_3d(3.7, -1.2, 0.8);
        let b = s.sample_3d(3.7001, -1.2, 0.8);
        assert!((a - b).abs() < 1e-3);
        // Non-constant.
        assert!(s.sample_2d(0.3, 0.9) != s.sample_2d(5.1, -2.2));
    }

    #[test]
    fn octave_simplex_new_simple() {
        let mut rand = Xoroshiro128::new(3);
        let octave = OctaveSimplexNoiseSampler::new(&mut rand, 4);
        assert_eq!(octave.persistence, 1.0);
        assert_eq!(octave.lacunarity, 1.0);
        let v = octave.sample(1.0, 2.0);
        assert!(v.is_finite());
        // Same-seed determinism.
        let mut rand2 = Xoroshiro128::new(3);
        let octave2 = OctaveSimplexNoiseSampler::new(&mut rand2, 4);
        assert_eq!(v, octave2.sample(1.0, 2.0));
    }

    #[test]
    fn octave_simplex_with_octaves() {
        // sorted [-3, 1, 2]:start=3, end=2, length=6
        // - contains(0)? No: the shared simplex is discarded (randomness consumed).
        // - idx 3..5: contains(2-idx) misses -> skip x3.
        // - end>0: noiseSeed -> indices 1..0 hit -> 2 samplers.
        let mut rand = Xoroshiro128::new(5);
        let octave = OctaveSimplexNoiseSampler::new_with_octaves(&mut rand, &[-3, 1, 2]);
        assert_eq!(octave.persistence, 2.0f64.powi(2));
        assert_eq!(octave.lacunarity, 1.0 / (2.0f64.powi(6) - 1.0));
        let v = octave.sample(1.5, -2.5);
        assert!(v.is_finite());
        // Same-seed determinism.
        let mut rand2 = Xoroshiro128::new(5);
        let octave2 = OctaveSimplexNoiseSampler::new_with_octaves(&mut rand2, &[-3, 1, 2]);
        assert_eq!(v, octave2.sample(1.5, -2.5));
        // IntStream delegation equivalence.
        let mut rand3 = Xoroshiro128::new(5);
        let octave3 = OctaveSimplexNoiseSampler::new_from_octaves_iter(&mut rand3, [1, -3, 2]);
        assert_eq!(v, octave3.sample(1.5, -2.5));
        // Trait form equals sample(x, y, true) * 0.55.
        let trait_v = NoiseSampler::sample(&octave, 1.5, -2.5, 0.0, 0.0);
        assert_eq!(trait_v, octave.sample_with_offset(1.5, -2.5, true) * 0.55);
    }

    #[test]
    fn octave_simplex_negative_end() {
        // All-negative octaves: end=-1 < 0, no shared slot or end>0 branch.
        // sorted [-5, -1]:start=5, length=5-1+1=5,idx 0..4:
        // contains(-1-idx): idx=0 hits contains(-1); idx=4 hits contains(-5).
        let mut rand = Xoroshiro128::new(9);
        let octave = OctaveSimplexNoiseSampler::new_with_octaves(&mut rand, &[-5, -1]);
        assert_eq!(octave.persistence, 2.0f64.powi(-1));
        assert!(octave.sample(0.5, 0.5).is_finite());
    }

    #[test]
    fn octave_simplex_empty_panics() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut r = Xoroshiro128::new(1);
            let _ = OctaveSimplexNoiseSampler::new_with_octaves(&mut r, &[]);
        }));
        assert!(result.is_err(), "empty octaves must panic");
    }

    #[test]
    fn simplex_noise_zero_amplitudes() {
        // All-zero amplitudes build no samplers: getValue == 0, getMax == 0.
        let mut rand = Xoroshiro128::new(21);
        let noise = SimplexNoise::new(&mut rand, -2, &[0.0, 0.0, 0.0]);
        assert_eq!(noise.get_value(1.0, 2.0, 3.0), 0.0);
        assert_eq!(noise.get_max(), 0.0);
    }

    #[test]
    fn simplex_noise_deterministic_and_wrap() {
        let mut r1 = Xoroshiro128::new(77);
        let n1 = SimplexNoise::new(&mut r1, -3, &[1.0, 0.5, 0.0, 0.25]);
        let mut r2 = Xoroshiro128::new(77);
        let n2 = SimplexNoise::new(&mut r2, -3, &[1.0, 0.5, 0.0, 0.25]);
        assert_eq!(n1.get_value(1.5, 2.5, 3.5), n2.get_value(1.5, 2.5, 3.5));
        assert_eq!(n1.get_max(), n2.get_max());
        assert!(n1.get_max() > 0.0);
        let v = n1.get_value(10.0, -20.0, 30.0);
        assert!(v.is_finite());

        // wrap period is 3.3554432E7.
        assert_eq!(SimplexNoise::wrap(0.0), 0.0);
        assert_eq!(SimplexNoise::wrap(3.3554432E7), 0.0);
        assert_eq!(SimplexNoise::wrap(-3.3554432E7), 0.0);
        assert_eq!(SimplexNoise::wrap(-0.25), -0.25f32);
        assert_eq!(SimplexNoise::wrap(1.5), 1.5f32);
        // Half-period rounding wraps back to the start.
        // (f32 spacing near 2^24 is 1.)
        let half = 3.3554432E7 / 2.0 + 0.25;
        assert_eq!(
            SimplexNoise::wrap(half) as f64,
            (half - 3.3554432E7) as f32 as f64
        );
    }
}
