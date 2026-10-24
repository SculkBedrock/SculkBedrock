//! 统计归一化噪声（`normal26`）：从 JSON 噪声参数构建双侧噪声栈。
//!
//! Statistical normalized noise: builds a two-sided noise stack from JSON
//! noise parameters.
//!
//! `build_octaves` 从 `base_amplitude` 和归一化项（归一化时为
//! `2^(n-1)/(2^n-1)`）派生每个八度的振幅，丢弃修饰符为零的八度；
//! `NormalNoise26::new` 构建双侧栈，其中**每个八度变成两层 Perlin 噪声**
//! （频率 `f` 与 `f * 1.0181268882175227`），每层按
//! `(normalizationFactor * amplitude)` 收窄为 f32 缩放。
//!
//! `build_octaves` derives each octave's amplitude from `base_amplitude` and a
//! normalization term (`2^(n-1)/(2^n-1)` when normalizing), drops
//! zero-modifier octaves, and `NormalNoise26::new` builds a two-sided stack
//! where **every octave becomes two Perlin noise layers** (frequencies `f` and
//! `f * 1.0181268882175227`), each scaled by `(normalizationFactor * amplitude)`
//! narrowed to f32.
//!
//! 构造过程消费按资源的随机源：
//! `first = random.fork_positional()`，`second = random.fork_positional()`，
//! 每个八度的两个 Perlin 采样器来自
//! `first/second.from_hash_of("octave_" + octave_index)`。分叉顺序是确定性
//! 合同的一部分。
//!
//! Construction consumes the per-resource random:
//! `first = random.fork_positional()`, `second = random.fork_positional()`, and
//! each octave's two Perlin samplers come from
//! `first/second.from_hash_of("octave_" + octave_index)`. The fork order is part
//! of the determinism contract.

use super::positional::{PositionalFactory, RngSource};
use super::synth::NoiseStack;

/// 归一化模式（JSON `normalize`: bool | "legacy"）。
///
/// Normalization mode (JSON `normalize`: bool | "legacy").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Normalize26 {
    Disabled,
    Enabled,
    Legacy,
}

/// JSON 噪声参数（`normalize` 默认为 ENABLED，bundle 中每个文件都不含该字段）。
///
/// JSON noise parameters (`normalize` defaults to ENABLED and is absent from
/// every file in the bundle).
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseParams26 {
    pub base_amplitude: f64,
    pub base_octave: i32,
    pub octave_count: usize,
    pub normalize: Normalize26,
    pub amplitude_modifiers: Vec<f64>,
}

/// 单个八度的频率与振幅。
///
/// One octave's frequency and amplitude.
struct OctaveInfo {
    octave_index: i32,
    frequency: f64,
    amplitude: f64,
}

impl OctaveInfo {
    fn seed(&self) -> String {
        format!("octave_{}", self.octave_index)
    }
}

/// 单层 Perlin 噪声的固定标准差。
///
/// The fixed standard deviation of a single Perlin noise layer.
const PERLIN_STANDARD_DEVIATION: f64 = 0.2702247831245211;
/// 第二侧栈的输入频率放大系数。
///
/// Input frequency amplification for the second side of the stack.
const INPUT_FACTOR: f64 = 1.0181268882175227;

/// 由参数推导八度列表：归一化时先按项 `2^(n-1)/(2^n-1)` 调整振幅，
/// 修饰符为零的八度被丢弃。
///
/// Derives the octave list: when normalizing, the amplitude is first adjusted by
/// the term `2^(n-1)/(2^n-1)`; octaves with a zero modifier are dropped.
fn build_octaves(params: &NoiseParams26) -> Vec<OctaveInfo> {
    let normalize = params.normalize != Normalize26::Disabled;
    let count = params.octave_count as i32;
    let mut frequency = 2.0f64.powi(params.base_octave);
    let mut amplitude = params.base_amplitude;
    if normalize {
        amplitude = params.base_amplitude
            * (0.5f64.powi(-(count - 1)) / (0.5f64.powi(-count) - 1.0));
    }

    let mut octaves = Vec::with_capacity(params.octave_count);
    for i in 0..count {
        let modifier = params.amplitude_modifiers.get(i as usize).copied().unwrap_or(1.0);
        if modifier != 0.0 {
            octaves.push(OctaveInfo {
                octave_index: params.base_octave + i,
                frequency,
                amplitude: amplitude * modifier,
            });
        }
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    octaves
}

/// 按各八度振幅的方差和计算归一化系数；方差为 0 时返回 0。
///
/// Computes the normalization factor from the summed per-octave variance;
/// returns 0 when that sum is 0.
fn compute_normalization_factor(target_amplitude: f64, octaves: &[OctaveInfo]) -> f64 {
    let mut variance = 0.0;
    for octave in octaves {
        let layer_deviation = PERLIN_STANDARD_DEVIATION * octave.amplitude.abs();
        variance += layer_deviation * layer_deviation;
    }
    let input_deviation = variance.sqrt();
    if input_deviation == 0.0 {
        return 0.0;
    }
    let target_deviation = target_amplitude * 0.3333333333333333;
    target_deviation / (input_deviation * std::f64::consts::SQRT_2)
}

/// LEGACY 归一化路径的系数：只依赖非零八度修饰符的索引跨度。
///
/// Factor for the LEGACY normalization path; depends only on the index span of
/// the non-zero octave modifiers.
fn compute_parity_normalization_factor(
    base_amplitude: f64,
    octave_count: usize,
    modifiers: &[f64],
) -> f64 {
    let mut min_octave = i32::MAX;
    let mut max_octave = i32::MIN;
    for i in 0..octave_count {
        let modifier = modifiers.get(i).copied().unwrap_or(1.0);
        if modifier != 0.0 {
            min_octave = min_octave.min(i as i32);
            max_octave = max_octave.max(i as i32);
        }
    }
    base_amplitude * 0.5 * 0.3333333333333333 / parity_expected_deviation(max_octave - min_octave)
}

/// 按八度跨度估算的期望偏差。
///
/// Expected deviation estimated from the octave span.
fn parity_expected_deviation(octave_span: i32) -> f64 {
    0.1 * (1.0 + 1.0 / (octave_span as f64 + 1.0))
}

/// 按种子绑定的单资源噪声。
///
/// The seed-bound noise for one resource.
#[derive(Clone, Debug)]
pub struct NormalNoise26 {
    stack: NoiseStack,
    /// 对称取值范围上界（供诊断与校验）。
    ///
    /// Symmetric range bound (for diagnostics and validation).
    max: f64,
}

impl NormalNoise26 {
    /// 由参数与随机源构建双侧栈。`Normalize26::Legacy` 且系数非零时改用
    /// LEGACY 路径系数。
    ///
    /// Builds the two-sided stack from the parameters and random source. With
    /// `Normalize26::Legacy` and a non-zero factor the LEGACY path factor is
    /// used instead.
    pub fn new(params: &NoiseParams26, random: &mut RngSource) -> Self {
        let octaves = build_octaves(params);
        let mut target_amplitude = octaves.iter().map(|o| o.amplitude.abs()).sum();
        let mut normalization_factor = compute_normalization_factor(target_amplitude, &octaves);
        if params.normalize == Normalize26::Legacy && normalization_factor != 0.0 {
            let parity = compute_parity_normalization_factor(
                params.base_amplitude,
                params.octave_count,
                &params.amplitude_modifiers,
            );
            target_amplitude *= parity / normalization_factor;
            normalization_factor = parity;
        }

        let first_factory: PositionalFactory = random.fork_positional();
        let second_factory: PositionalFactory = random.fork_positional();

        let mut layers = Vec::with_capacity(octaves.len() * 2);
        for octave in &octaves {
            let seed = octave.seed();
            let value_factor = (normalization_factor * octave.amplitude) as f32;
            let mut first_rng = first_factory.from_hash_of(&seed);
            layers.push((
                super::synth::StackNoise::Perlin(super::synth::PerlinNoise::new(&mut first_rng)),
                octave.frequency,
                value_factor,
            ));
            let mut second_rng = second_factory.from_hash_of(&seed);
            layers.push((
                super::synth::StackNoise::Perlin(super::synth::PerlinNoise::new(&mut second_rng)),
                octave.frequency * INPUT_FACTOR,
                value_factor,
            ));
        }

        Self {
            stack: NoiseStack::new(layers),
            max: target_amplitude * 0.3333333333333333 * 6.0,
        }
    }

    /// 采样双侧栈：按层顺序 f32 累加。
    ///
    /// Samples the two-sided stack: f32 accumulation in layer order.
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        self.stack.get(x, y, z)
    }

    /// 对称取值范围上界。
    ///
    /// The symmetric range bound.
    pub fn max(&self) -> f64 {
        self.max
    }

    /// `legacy_random_source` 下下界维度使用的温度/植被噪声：两个 Fbm 栈
    /// 共享同一条 LCG 流，以频率系数 1.0 与 1.0181268882175227 相加。
    ///
    /// The temperature/vegetation noises under `legacy_random_source`: two Fbm
    /// stacks over the *same* LCG stream, added with frequency factors 1.0 and
    /// 1.0181268882175227.
    pub fn create_for_legacy_nether_biome(
        params: &NoiseParams26,
        random: &mut crate::noise::LegacyRandom,
    ) -> Self {
        use crate::noise::synth::NoiseStack;

        let octaves = build_octaves(params);
        let target_amplitude: f64 = octaves.iter().map(|o| o.amplitude.abs()).sum();
        let mut normalization_factor = compute_normalization_factor(target_amplitude, &octaves);
        if params.normalize == Normalize26::Legacy && normalization_factor != 0.0 {
            normalization_factor = compute_parity_normalization_factor(
                params.base_amplitude,
                params.octave_count,
                &params.amplitude_modifiers,
            );
        }
        let mut amplitudes = params.amplitude_modifiers.clone();
        if amplitudes.is_empty() {
            amplitudes = vec![1.0; params.octave_count];
        }
        let first = legacy_fbm_initializer(random, params.base_octave, &amplitudes);
        let second = legacy_fbm_initializer(random, params.base_octave, &amplitudes);
        let value_factor = (normalization_factor * params.base_amplitude) as f32;
        // Both sub-stacks are flattened into one layer list; the f32
        // accumulation order of the resulting list is observable.
        let mut layers = Vec::new();
        first.flatten_into(&mut layers, 1.0, value_factor);
        second.flatten_into(&mut layers, INPUT_FACTOR, value_factor);
        let stack = NoiseStack::new(layers);
        Self { stack, max: target_amplitude * 0.3333333333333333 * 6.0 }
    }
}

/// 从零八度向下逐层构建 Perlin 层；振幅为零或缺失时消耗 262 次抽取以推进流。
///
/// Builds Perlin layers downward from the zero octave; zero or missing
/// amplitudes consume 262 draws to advance the stream.
fn legacy_fbm_initializer(
    random: &mut crate::noise::LegacyRandom,
    first_octave: i32,
    amplitudes: &[f64],
) -> NoiseStack {
    use crate::noise::synth::{NoiseStack, PerlinNoise, StackNoise};

    let octaves = amplitudes.len() as i32;
    let zero_octave_index = -first_octave;
    let mut levels: Vec<Option<PerlinNoise>> = vec![None; amplitudes.len()];
    let zero_octave = PerlinNoise::new(random);
    if zero_octave_index >= 0 && zero_octave_index < octaves {
        if amplitudes[zero_octave_index as usize] != 0.0 {
            levels[zero_octave_index as usize] = Some(zero_octave);
        }
    }
    let mut i = zero_octave_index - 1;
    while i >= 0 {
        if i < octaves {
            if amplitudes[i as usize] != 0.0 {
                levels[i as usize] = Some(PerlinNoise::new(random));
            } else {
                // Skipped octave: consume 262 draws.
                random.consume_count(262);
            }
        } else {
            random.consume_count(262);
        }
        i -= 1;
    }
    let mut factor = 2.0f64.powi(-zero_octave_index);
    let mut value_factor = 2.0f64.powi(octaves - 1) / (2.0f64.powi(octaves) - 1.0);
    let mut layers = Vec::new();
    for (index, level) in levels.into_iter().enumerate() {
        if let Some(noise) = level {
            layers.push((
                StackNoise::Perlin(noise),
                factor,
                (value_factor * amplitudes[index]) as f32,
            ));
        }
        factor *= 2.0;
        value_factor /= 2.0;
    }
    NoiseStack::new(layers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::positional::XoroshiroRandomSource;

    fn rng(seed: i64) -> RngSource {
        RngSource::Xoroshiro(XoroshiroRandomSource::from_legacy_seed(seed))
    }

    #[test]
    fn octave_build_matches_java_math() {
        // continentalness-shaped params: base_octave -9, 9 octaves.
        let params = NoiseParams26 {
            base_amplitude: 0.8880832896205223,
            base_octave: -9,
            octave_count: 9,
            normalize: Normalize26::Enabled,
            amplitude_modifiers: vec![1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0],
        };
        let octaves = build_octaves(&params);
        assert_eq!(octaves.len(), 9);
        assert_eq!(octaves[0].octave_index, -9);
        assert_eq!(octaves[0].frequency, 2.0f64.powi(-9));
        // normalize factor for 9 octaves: 2^8/(2^9-1).
        let expected_amp = params.base_amplitude * (2.0f64.powi(8) / (2.0f64.powi(9) - 1.0));
        assert!((octaves[0].amplitude - expected_amp).abs() < 1e-18);
        // Frequency doubles; amplitude halves per octave.
        assert!((octaves[1].frequency - 2.0 * octaves[0].frequency).abs() < 1e-12);
        assert!((octaves[1].amplitude - 0.5 * octaves[0].amplitude).abs() < 1e-18);
    }

    #[test]
    fn zero_modifier_octaves_are_dropped() {
        let params = NoiseParams26 {
            base_amplitude: 1.0,
            base_octave: -3,
            octave_count: 5,
            normalize: Normalize26::Enabled,
            amplitude_modifiers: vec![1.0, 1.0, 0.0, 0.0, 1.0],
        };
        let octaves = build_octaves(&params);
        assert_eq!(octaves.len(), 3, "zero modifiers must drop octaves");
        assert_eq!(octaves.iter().map(|o| o.octave_index).collect::<Vec<_>>(), vec![-3, -2, 1]);
    }

    #[test]
    fn disabled_normalization_uses_raw_amplitude() {
        let params = NoiseParams26 {
            base_amplitude: 2.0,
            base_octave: -2,
            octave_count: 2,
            normalize: Normalize26::Disabled,
            amplitude_modifiers: vec![1.0, 1.0],
        };
        let octaves = build_octaves(&params);
        assert_eq!(octaves[0].amplitude, 2.0);
        assert_eq!(octaves[1].amplitude, 1.0);
    }

    #[test]
    fn construction_is_deterministic_and_bounded() {
        let params = NoiseParams26 {
            base_amplitude: 0.875,
            base_octave: -6,
            octave_count: 3,
            normalize: Normalize26::Enabled,
            amplitude_modifiers: vec![1.0, 1.0, 1.0],
        };
        let mut a = rng(42);
        let noise_a = NormalNoise26::new(&params, &mut a);
        let mut b = rng(42);
        let noise_b = NormalNoise26::new(&params, &mut b);
        let mut c = rng(43);
        let noise_c = NormalNoise26::new(&params, &mut c);
        assert_eq!(noise_a.get(1.0, 2.0, 3.0), noise_b.get(1.0, 2.0, 3.0));
        assert_ne!(noise_a.get(1.0, 2.0, 3.0), noise_c.get(1.0, 2.0, 3.0));
        for i in 0..64 {
            let x = i as f64 * 0.41;
            let v = noise_a.get(x, x * 0.3, -x);
            assert!(v.is_finite());
        }
    }
}
