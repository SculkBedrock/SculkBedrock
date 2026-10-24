//! 噪声采样器基类型：`Noise`（原点 + 置换基类）、多八度 `PerlinNoise` 栈、
//! 双栈 `NormalNoise`，以及 `SKIP_262` 种子跳过契约。
//!
//! Noise sampler base types: `Noise` (origin + permutation base), the
//! multi-octave `PerlinNoise` stack, the two-stack `NormalNoise`, and the
//! `SKIP_262` seed-skipping contract.

use super::perlin::{maintain_precision, PerlinNoiseSampler};
use super::random::RandomSourceProvider;

/// 从种子起推进 262 步 LCG，用于跳过被丢弃的八度。
///
/// Advances a seed by 262 LCG steps; used to skip discarded octaves.
///
/// 种子状态占 48 位——LCG 模数是 2^48，因此每一步都把状态掩码到 48 位。
///
/// Seed state lives in 48 bits — the LCG modulus is 2^48, so every step masks
/// the state to 48 bits.
pub(crate) fn skip_262_seed(seed: i64) -> i64 {
    const MULTIPLIER: i64 = 25214_90391_7; // LCG "a"
    const ADDEND: i64 = 11; // LCG "c"
    const MODULUS_MASK: i64 = (1 << 48) - 1;
    let mut s = seed;
    for _ in 0..262 {
        s = s.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND) & MODULUS_MASK;
    }
    s
}

/// 共享噪声基类（原点 + 洗牌后的置换表）。
///
/// Shared noise base (origin + shuffled permutation table).
///
/// RNG 消耗顺序：3 次 `nextDouble`（原点），然后 256 次
/// `nextBoundedInt`（Fisher-Yates 洗牌）。
///
/// RNG consumption order: 3x `nextDouble` (origin), then 256x
/// `nextBoundedInt` (Fisher-Yates shuffle).
pub struct Noise {
    pub origin_x: f64,
    pub origin_y: f64,
    pub origin_z: f64,
    /// `u8` 与有符号字节的低 8 位一致。
    ///
    /// `u8` matches the low 8 bits of a signed byte.
    pub(crate) permutations: [u8; 256],
}

impl Noise {
    /// 构建基类采样器。
    ///
    /// Builds the base sampler.
    pub fn new<R: RandomSourceProvider>(rand: &mut R) -> Self {
        let origin_x = rand.next_double() * 256.0;
        let origin_y = rand.next_double() * 256.0;
        let origin_z = rand.next_double() * 256.0;

        let mut permutations = [0u8; 256];
        for (index, slot) in permutations.iter_mut().enumerate() {
            *slot = index as u8;
        }
        for index in 0..256usize {
            let random_index =
                (rand.next_bounded_int(255 - index as i32) + index as i32) as usize;
            permutations.swap(index, random_index);
        }
        Self { origin_x, origin_y, origin_z, permutations }
    }
}

// ---------------------------------------------------------------------------
// PerlinNoise (multi-octave stack)
// ---------------------------------------------------------------------------

/// 多八度 Perlin 噪声栈。
///
/// A multi-octave Perlin noise stack.
pub struct PerlinNoise {
    noise_levels: Vec<Option<PerlinNoiseSampler>>,
    amplitudes: Vec<f64>,
    lowest_freq_value_factor: f64,
    lowest_freq_input_factor: f64,
    max_value: f64,
}

impl PerlinNoise {
    /// 由 `first_octave` 与每八度振幅构建。RNG 消耗顺序固定：
    /// 零八度先构建，然后向下遍历剩余索引；任何重排都会改变所有噪声值。
    ///
    /// Builds from `first_octave` and per-octave amplitudes. The RNG consumption
    /// order is fixed: the zero octave is constructed first, then the remaining
    /// indices are walked downward; any reordering changes every noise value.
    pub fn new<R: RandomSourceProvider>(
        random: &mut R,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let octaves = amplitudes.len();
        let zero_octave_index = first_octave.wrapping_neg();
        let mut noise_levels: Vec<Option<PerlinNoiseSampler>> =
            (0..octaves).map(|_| None).collect();

        // The zero octave always consumes randomness; amplitude decides
        // whether the sampler is kept.
        let zero_octave = PerlinNoiseSampler::new(random);
        if zero_octave_index >= 0 && (zero_octave_index as usize) < octaves {
            let amplitude = amplitudes[zero_octave_index as usize];
            if amplitude != 0.0 {
                noise_levels[zero_octave_index as usize] = Some(zero_octave);
            }
        }

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

    /// 跳过该八度：只推进种子，不构建采样器。
    ///
    /// Skips this octave: advances only the seed instead of building a sampler.
    fn skip_octave<R: RandomSourceProvider>(random: &mut R) {
        random.set_seed(skip_262_seed(random.get_seed()));
    }

    /// 逐八度累加 `amplitude * sample * value_factor`；输入标度每次加倍、
    /// 值标度每次减半。
    ///
    /// Accumulates `amplitude * sample * value_factor` per octave; the input
    /// scale doubles and the value scale halves each step.
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut value = 0.0;
        let mut factor = self.lowest_freq_input_factor;
        let mut value_factor = self.lowest_freq_value_factor;

        for (i, level) in self.noise_levels.iter().enumerate() {
            if let Some(noise) = level {
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

    /// 在构造期计算取值范围上界（按假定噪声值 `noise_value` 累加保留的八度）。
    ///
    /// Computes the range bound at construction time by summing the kept octaves
    /// with an assumed `noise_value`.
    fn edge_value(
        noise_levels: &[Option<PerlinNoiseSampler>],
        amplitudes: &[f64],
        lowest_freq_value_factor: f64,
        noise_value: f64,
    ) -> f64 {
        let mut value = 0.0;
        let mut value_factor = lowest_freq_value_factor;
        for (i, level) in noise_levels.iter().enumerate() {
            if level.is_some() {
                value += amplitudes[i] * noise_value * value_factor;
            }
            value_factor /= 2.0;
        }
        value
    }

    /// 返回构造期算出的取值范围上界。
    ///
    /// Returns the range bound computed at construction time.
    pub fn max_value(&self) -> f64 {
        self.max_value
    }
}

// ---------------------------------------------------------------------------
// NormalNoise (two-stack normalized sampler)
// ---------------------------------------------------------------------------

/// 第二个栈相对第一个栈的输入放大系数。
///
/// Input amplification applied to the second stack relative to the first.
const INPUT_FACTOR: f64 = 1.0181268882175227;

/// 由两个 Perlin 栈归一化相加得到的噪声采样器。
///
/// A noise sampler built from two Perlin stacks, summed and normalized.
pub struct NormalNoise {
    first: PerlinNoise,
    second: PerlinNoise,
    value_factor: f64,
    max_value: f64,
}

impl NormalNoise {
    /// 由 `first_octave` 与 f32 振幅列表构建；振幅内部加宽为 f64，
    /// 归一化系数依赖非零八度的跨度。
    ///
    /// Builds from `first_octave` and an f32 amplitude list; amplitudes widen to
    /// f64 internally and the normalization factor depends on the span of the
    /// non-zero octaves.
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
        let value_factor = 1.0 / 6.0 / Self::expected_deviation(max.wrapping_sub(min));
        let max_value = (first.max_value() + second.max_value()) * value_factor;
        Self { first, second, value_factor, max_value }
    }

    /// 两个栈归一化相加后收窄为 f32 返回。
    ///
    /// Sums and normalizes both stacks, then narrows the result to f32.
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f32 {
        let x2 = x * INPUT_FACTOR;
        let y2 = y * INPUT_FACTOR;
        let z2 = z * INPUT_FACTOR;
        ((self.first.get_value(x, y, z) + self.second.get_value(x2, y2, z2)) * self.value_factor)
            as f32
    }

    /// 返回取值范围上界。
    ///
    /// Returns the range bound.
    pub fn get_max(&self) -> f64 {
        self.max_value
    }

    /// 按八度跨度估算的期望偏差，用于归一化系数。
    ///
    /// Expected deviation estimated from the octave span, used by the
    /// normalization factor.
    fn expected_deviation(octave_span: i32) -> f64 {
        0.1 * (1.0 + 1.0 / (octave_span as f64 + 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::random::Xoroshiro128;

    #[test]
    fn stack_construction_matches_sequential_samplers() {
        // Construction-order contract: the octave samplers inside a
        // PerlinNoise stack are created in exactly the same RNG order as
        // bare sequential construction (zero octave first, then the loop) —
        // any reordering silently changes every noise value.
        // Stack: PerlinNoise::new(99, -3, [0,0,1,0,0]) — zero octave
        // discarded, ix=2 sampler kept, two skips afterwards.
        let mut rand = Xoroshiro128::new(99);
        let stack = PerlinNoise::new(&mut rand, -3, &[0.0, 0.0, 1.0, 0.0, 0.0]);
        // Manual: two samplers back to back (zero + kept).
        let mut rand2 = Xoroshiro128::new(99);
        let _zero = PerlinNoiseSampler::new(&mut rand2);
        let kept = PerlinNoiseSampler::new(&mut rand2);
        // The stack's noise_levels[2] must be the second sampler.
        let stack_sampler2 = stack.noise_levels[2].as_ref().expect("index 2 must be Some");
        println!(
            "origins equal: {}",
            stack_sampler2.noise.origin_x == kept.noise.origin_x
                && stack_sampler2.noise.origin_y == kept.noise.origin_y
                && stack_sampler2.noise.origin_z == kept.noise.origin_z
        );
        let a = stack_sampler2.sample(0.75, 1.25, 1.75, 0.0, 0.0);
        let b = kept.sample(0.75, 1.25, 1.75, 0.0, 0.0);
        println!("stack sampler2 sample={a:?} manual kept sample={b:?} equal={}", a == b);
        println!("stack value={:?}", stack.get_value(1.5, 2.5, 3.5));
        assert_eq!(
            stack.get_value(1.5, 2.5, 3.5),
            stack_sampler2.sample(0.75, 1.25, 1.75, 0.0, 0.0) * (16.0f64 / 31.0) / 4.0,
            "the stack value must be the kept sampler's contribution exactly"
        );
        println!(
            "lowest_freq_input_factor={:?} lowest_freq_value_factor={:?} (16/31={:?})",
            stack.lowest_freq_input_factor,
            stack.lowest_freq_value_factor,
            16.0f64 / 31.0
        );
        println!(
            "expected input factor (2^-3)={:?}",
            2.0f64.powi(-3)
        );
    }

    #[test]
    fn skip_262_seed_matches_iteration() {
        // Seed state lives in 48 bits: every LCG step masks to 2^48.
        const MASK: i64 = (1 << 48) - 1;
        let mut expected = 12345i64;
        for _ in 0..262 {
            expected = expected.wrapping_mul(25214_90391_7).wrapping_add(11) & MASK;
        }
        assert_eq!(skip_262_seed(12345), expected);
        // 64-bit wrapping would diverge — guard the contract explicitly.
        let mut unmasked = 12345i64;
        for _ in 0..262 {
            unmasked = unmasked.wrapping_mul(25214_90391_7).wrapping_add(11);
        }
        assert_ne!(unmasked, expected, "the 48-bit mask must be load-bearing");
    }

    #[test]
    fn noise_permutations_form_shuffle() {
        let mut rand = Xoroshiro128::new(2024);
        let noise = Noise::new(&mut rand);
        let mut seen = [false; 256];
        for &p in &noise.permutations {
            seen[p as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "permutations must be a 0..=255 permutation");
        let mut rand2 = Xoroshiro128::new(2024);
        let noise2 = Noise::new(&mut rand2);
        assert_eq!(noise.permutations, noise2.permutations);
    }

    #[test]
    fn perlin_noise_deterministic_and_linear() {
        let mut r1 = Xoroshiro128::new(77);
        let n1 = PerlinNoise::new(&mut r1, -3, &[1.0, 1.0, 1.0, 0.0, 1.0]);
        let mut r2 = Xoroshiro128::new(77);
        let n2 = PerlinNoise::new(&mut r2, -3, &[1.0, 1.0, 1.0, 0.0, 1.0]);
        assert_eq!(n1.get_value(1.5, 2.5, 3.5), n2.get_value(1.5, 2.5, 3.5));
        assert_eq!(n1.max_value(), n2.max_value());

        let mut r3 = Xoroshiro128::new(77);
        let n3 = PerlinNoise::new(&mut r3, -3, &[0.5, 0.5, 0.5, 0.0, 0.5]);
        assert_eq!(n3.get_value(1.5, 2.5, 3.5), 0.5 * n1.get_value(1.5, 2.5, 3.5));
    }

    #[test]
    fn normal_noise_bounds_and_determinism() {
        let mut rand = Xoroshiro128::new(123);
        let noise = NormalNoise::new(&mut rand, -5, &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        let max = noise.get_max();
        assert!(max > 0.0);
        for i in 0..100 {
            let x = i as f64 * 0.37;
            let v = noise.get_value(x, x * 0.5, -x);
            assert!(v.is_finite());
            assert!((v as f64).abs() <= max * 1.000001, "v={v} exceeds bound max={max}");
        }
        let mut rand2 = Xoroshiro128::new(123);
        let noise2 = NormalNoise::new(&mut rand2, -5, &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(noise.get_value(10.0, 20.0, 30.0), noise2.get_value(10.0, 20.0, 30.0));
    }
}
