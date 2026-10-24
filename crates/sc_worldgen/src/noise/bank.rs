//! 按种子绑定的噪声运行时：每个世界种子一个 `NoiseBank`，跨作业共享只读
//! 采样器；构造完成后无可变状态。
//!
//! Seed-bound noise runtime: one `NoiseBank` per world seed, sharing
//! read-only samplers across jobs; no mutable state after construction.
//!
//! **种子绑定合同：**
//!
//! **Seed-binding contract:**
//!
//! 1. `world_positional = WorldPositional::new(world_seed, legacy)`
//!    （链接后的设置请求 `legacy_random_source` 时使用 LCG 变体）；
//! 2. 每个噪声资源通过 `world_positional.from_hash_of(resource_id)` 派生其
//!    随机源——**对资源 id 取 MD5，与顺序无关**；
//! 3. 每个资源构建一个统计归一化噪声（[`NormalNoise26`]：每个八度两层
//!    Perlin、f32 累加）。
//!
//! 1. `world_positional = WorldPositional::new(world_seed, legacy)` (the LCG
//!    variant is used when the linked settings request
//!    `legacy_random_source`);
//! 2. every noise resource derives its random via
//!    `world_positional.from_hash_of(resource_id)` — **MD5 of the resource id,
//!    order-independent**;
//! 3. each resource builds a [`NormalNoise26`] (statistical stack, two perlin
//!    layers per octave, f32 accumulation).
//!
//! `old_blended_noise` 使用 [`BlendedRuntime26`]：三个 Fbm 栈（两个 limit
//! 加一个 main）由 `SmearedPerlinNoise` 层构成，种子来自
//! `world_positional.noise_random("minecraft:terrain")`。
//!
//! `old_blended_noise` uses [`BlendedRuntime26`]: three Fbm stacks (two limit
//! plus one main) of `SmearedPerlinNoise` layers, seeded from
//! `world_positional.noise_random("minecraft:terrain")`.

use std::collections::BTreeMap;

use super::simplex::SimplexNoise;
use super::legacy_random::LegacyRandom;
use super::normal26::{Normalize26, NormalNoise26, NoiseParams26};
use super::positional::{RngSource, WorldPositional};
use super::synth::{NoiseStack, SmearedPerlinNoise, StackNoise};
use crate::eval::{NoiseSampler, OldBlendedParams};
use crate::link::NoiseParams;

/// 混合噪声的基准标度。
///
/// Base scale of the blended noise.
const BLEND_BASE_SCALE: f64 = 684.412;
/// limit 栈与 main 栈的振幅系数。
///
/// Amplitude factors of the limit and main stacks.
const BLEND_LIMIT_FACTOR: f64 = 0.99998474;
const BLEND_MAIN_FACTOR: f64 = 12.75;
/// 混合噪声派生随机源使用的资源 id。
///
/// The resource id the blended noise derives its random source from.
const BLEND_NOISE_SEED: &str = "minecraft:terrain";
/// limit 栈与 main 栈的首八度。
///
/// First octaves of the limit and main stacks.
const BLEND_LIMIT_FIRST_OCTAVE: i32 = -15;
const BLEND_MAIN_FIRST_OCTAVE: i32 = -7;

/// 按种子绑定的 `old_blended_noise` 运行时，对应一种参数形态：
/// 两个 limit Fbm 栈加一个 main 栈，由钳制后的 `main + 0.5` 经 lerp 选取。
///
/// The seed-bound `old_blended_noise` runtime for one parameter shape: two limit
/// Fbm stacks plus one main stack, lerp-selected by the clamped `main + 0.5`.
struct BlendedRuntime26 {
    min_limit: NoiseStack,
    max_limit: NoiseStack,
    main: NoiseStack,
    xz_multiplier: f64,
    y_multiplier: f64,
    xz_factor: f64,
    y_factor: f64,
}

impl BlendedRuntime26 {
    /// 由地形随机源与参数构建三个 Fbm 栈；构建顺序（min_limit、max_limit、
    /// main）消耗 RNG，必须保持不变。
    ///
    /// Builds the three Fbm stacks from the terrain random and the parameters.
    /// The build order (min_limit, max_limit, main) consumes the RNG and must
    /// stay unchanged.
    fn new(terrain_rng: &mut impl crate::noise::synth::GradientRng, params: &OldBlendedParams) -> Self {
        let xz_multiplier = BLEND_BASE_SCALE * params.xz_scale;
        let y_multiplier = BLEND_BASE_SCALE * params.y_scale;
        let limit_smear = y_multiplier * params.smear_scale_multiplier;
        let main_smear = limit_smear / params.y_factor;
        Self {
            min_limit: create_fbm(terrain_rng, BLEND_LIMIT_FIRST_OCTAVE, limit_smear, BLEND_LIMIT_FACTOR),
            max_limit: create_fbm(terrain_rng, BLEND_LIMIT_FIRST_OCTAVE, limit_smear, BLEND_LIMIT_FACTOR),
            main: create_fbm(terrain_rng, BLEND_MAIN_FIRST_OCTAVE, main_smear, BLEND_MAIN_FACTOR),
            xz_multiplier,
            y_multiplier,
            xz_factor: params.xz_factor,
            y_factor: params.y_factor,
        }
    }

    /// 采样：`clamp(main + 0.5, 0, 1)` 作为 lerp 的 alpha 在两个 limit 栈之间
    /// 选取。整条路径使用 f32。
    ///
    /// Samples: `clamp(main + 0.5, 0, 1)` is the lerp alpha selecting between the
    /// two limit stacks. The whole path is f32.
    fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        let min = self.min_limit.get(
            x * self.xz_multiplier,
            y * self.y_multiplier,
            z * self.xz_multiplier,
        );
        let max = self.max_limit.get(
            x * self.xz_multiplier,
            y * self.y_multiplier,
            z * self.xz_multiplier,
        );
        let main = self.main.get(
            x * (self.xz_multiplier / self.xz_factor),
            y * (self.y_multiplier / self.y_factor),
            z * (self.xz_multiplier / self.xz_factor),
        );
        let choice = (main + 0.5f32).clamp(0.0, 1.0);
        // lerp short-circuits at alpha == 0 (first) and alpha == 1 (second).
        // The short-circuits are observable: first + 1.0 * (second - first)
        // is not always exactly second.
        let value = if choice == 0.0 {
            min
        } else if choice == 1.0 {
            max
        } else {
            min + choice * (max - min)
        };
        f64::from(value)
    }
}

/// 构建一个 Fbm 栈：`-first_octave + 1` 个 smear 层，频率逐层减半、
/// 取值因子逐层翻倍。`first_octave > 0` 时 panic。
///
/// Builds one Fbm stack: `-first_octave + 1` smeared layers with halving
/// frequencies and doubling value factors. Panics when `first_octave > 0`.
fn create_fbm(
    rng: &mut impl crate::noise::synth::GradientRng,
    first_octave: i32,
    smear_scale_y: f64,
    value_factor: f64,
) -> NoiseStack {
    assert!(first_octave <= 0, "firstOctave>0");
    let octaves = (-first_octave + 1) as usize;
    let mut factor = 1.0f64;
    let mut value_factor = value_factor / (2.0f64.powi(octaves as i32) - 1.0);
    let mut layers = Vec::with_capacity(octaves);
    for _ in 0..octaves {
        layers.push((
            StackNoise::Smeared(SmearedPerlinNoise::new(rng, smear_scale_y * factor)),
            factor,
            value_factor as f32,
        ));
        factor /= 2.0;
        value_factor *= 2.0;
    }
    NoiseStack::new(layers)
}

/// 五个混合参数上的全序键（位精确）。
///
/// Total-order key over the five blended parameters (bit-exact).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BlendedKey {
    xz_scale: u64,
    y_scale: u64,
    xz_factor: u64,
    y_factor: u64,
    smear_scale_multiplier: u64,
}

impl BlendedKey {
    fn of(params: &OldBlendedParams) -> Self {
        Self {
            xz_scale: params.xz_scale.to_bits(),
            y_scale: params.y_scale.to_bits(),
            xz_factor: params.xz_factor.to_bits(),
            y_factor: params.y_factor.to_bits(),
            smear_scale_multiplier: params.smear_scale_multiplier.to_bits(),
        }
    }
}

/// 按种子绑定的单世界种子运行时。
///
/// The seed-bound runtime for one world seed.
pub struct NoiseBank {
    world_seed: i64,
    /// 链接后的噪声资源（每个 id 一个统计归一化噪声）。
    ///
    /// Linked noise resources (one statistical normalized noise per id).
    noises: BTreeMap<String, NormalNoise26>,
    /// 预构建的 `old_blended_noise` 运行时，以节点的参数形态为键；
    /// 种子派生自 `minecraft:terrain` 位置随机源。
    ///
    /// Prebuilt `old_blended_noise` runtimes, keyed by the node's parameter
    /// shape; seeded from the `minecraft:terrain` positional random.
    blended: BTreeMap<BlendedKey, BlendedRuntime26>,
    /// 末地岛屿噪声：由 `LegacyRandom(world_seed)` 消耗 17292 次抽取后构建，
    /// 并丢弃噪声偏移。随机源直接由世界种子播种，不使用位置随机源。
    ///
    /// End-island noise: built from `LegacyRandom(world_seed)` after consuming
    /// 17292 draws, with the noise offset discarded. The random is seeded from
    /// the world seed directly, not positionally.
    island: SimplexNoise,
}


/// 把链接后的 JSON 参数转换为噪声栈输入（编解码器默认值：
/// `base_amplitude` 1.0、`octave_count` 1、`normalize` 启用、修饰符为空）。
///
/// Converts linked JSON params into the noise-stack inputs (codec defaults:
/// `base_amplitude` 1.0, `octave_count` 1, `normalize` ENABLED, modifiers
/// empty).
fn to_params26(params: &NoiseParams) -> NoiseParams26 {
    NoiseParams26 {
        base_amplitude: params.base_amplitude,
        base_octave: params.base_octave,
        octave_count: params
            .octave_count
            .filter(|count| *count > 0)
            .unwrap_or(1) as usize,
        normalize: params.normalize.unwrap_or(Normalize26::Enabled),
        amplitude_modifiers: params.amplitude_modifiers.clone().unwrap_or_default(),
    }
}

impl NoiseBank {
    /// 从链接后的噪声参数为一个世界种子构建噪声库，遵循模块文档的
    /// 种子绑定合同；此后噪声库不可变且 `Send + Sync`。
    /// `legacy` 选择位置随机源工厂的 `legacy_random_source` 变体。
    ///
    /// Builds the bank for one world seed from the linked noise parameters,
    /// following the seed-binding contract in the module docs; afterwards the
    /// bank is immutable and `Send + Sync`. `legacy` selects the
    /// `legacy_random_source` variant of the positional factory.
    ///
    /// `blended_params` 列出调用方的树中包含的每种 `old_blended_noise`
    /// 节点形态（它们的运行时栈由 `minecraft:terrain` 位置随机源预构建）。
    ///
    /// `blended_params` lists every `old_blended_noise` node shape the
    /// caller's trees contain (their runtime stacks are prebuilt from the
    /// `minecraft:terrain` positional random).
    pub fn new(
        world_seed: i64,
        params: &BTreeMap<String, NoiseParams>,
        blended_params: &[OldBlendedParams],
    ) -> Self {
        Self::with_legacy(world_seed, params, blended_params, false)
    }

    /// 与 [`Self::new`] 相同，但显式选择 `legacy_random_source`。
    ///
    /// Same as [`Self::new`] with an explicit `legacy_random_source` choice.
    pub fn with_legacy(
        world_seed: i64,
        params: &BTreeMap<String, NoiseParams>,
        blended_params: &[OldBlendedParams],
        legacy: bool,
    ) -> Self {
        let positional = WorldPositional::new(world_seed, legacy);
        let mut noises = BTreeMap::new();
        for (name, noise_params) in params {
            let params26 = to_params26(noise_params);
            // Under legacy random the nether temperature/vegetation noises are
            // seeded from `LegacyRandom(world_seed)` and
            // `LegacyRandom(world_seed + 1)` instead of the positional chain.
            let noise = if legacy {
                match name.as_str() {
                    "minecraft:nether/temperature" => {
                        let mut random = LegacyRandom::new(world_seed);
                        NormalNoise26::create_for_legacy_nether_biome(&params26, &mut random)
                    }
                    "minecraft:nether/vegetation" => {
                        let mut random = LegacyRandom::new(world_seed + 1);
                        NormalNoise26::create_for_legacy_nether_biome(&params26, &mut random)
                    }
                    _ => {
                        let mut random: RngSource = positional.noise_random(name);
                        NormalNoise26::new(&params26, &mut random)
                    }
                }
            } else {
                let mut random: RngSource = positional.noise_random(name);
                NormalNoise26::new(&params26, &mut random)
            };
            noises.insert(name.clone(), noise);
        }
        // The old_blended stacks derive from the positional "minecraft:terrain"
        // random, one fresh RngSource per parameter shape.
        let mut blended = BTreeMap::new();
        for shape in blended_params {
            // Under legacy random the blended noise is seeded from
            // `LegacyRandom(world_seed)`; the positional factory is used otherwise.
            let runtime = if legacy {
                let mut terrain_rng = LegacyRandom::new(world_seed);
                BlendedRuntime26::new(&mut terrain_rng, shape)
            } else {
                let mut terrain_rng = positional.noise_random(BLEND_NOISE_SEED);
                BlendedRuntime26::new(&mut terrain_rng, shape)
            };
            blended.insert(BlendedKey::of(shape), runtime);
        }
        // End islands: world-seeded LCG plus 17292 skipped draws.
        let mut island_random = LegacyRandom::new(world_seed);
        island_random.consume_count(17292);
        let island = SimplexNoise::new(&mut island_random, true);
        Self { world_seed, noises, blended, island }
    }

    /// 直接由链接后的设置构建噪声库：噪声、`legacy_random_source` 标志，
    /// 以及路由树中出现的每种 `old_blended_noise` 形态。
    ///
    /// Builds the bank straight from linked settings: noises, the
    /// `legacy_random_source` flag and every `old_blended_noise` shape found
    /// in the router trees.
    pub fn from_settings(world_seed: i64, settings: &crate::link::LinkedNoiseSettings) -> Self {
        let mut blended = Vec::new();
        for node in settings.router.values() {
            node.walk(&mut |visited| {
                if let crate::data::Node::OldBlendedNoise {
                    xz_scale,
                    y_scale,
                    xz_factor,
                    y_factor,
                    smear_scale_multiplier,
                } = visited
                {
                    let shape = OldBlendedParams {
                        xz_scale: *xz_scale,
                        y_scale: *y_scale,
                        xz_factor: *xz_factor,
                        y_factor: *y_factor,
                        smear_scale_multiplier: *smear_scale_multiplier,
                    };
                    if !blended.contains(&shape) {
                        blended.push(shape);
                    }
                }
            });
        }
        Self::with_legacy(
            world_seed,
            &settings.noises,
            &blended,
            settings.legacy_random_source,
        )
    }

    /// The seed this bank is bound to (diagnostics; results never depend on
    /// anything else).
    pub fn world_seed(&self) -> i64 {
        self.world_seed
    }

    /// 绑定的噪声资源数量。
    ///
    /// Number of bound noise resources.
    pub fn len(&self) -> usize {
        self.noises.len()
    }

    pub fn is_empty(&self) -> bool {
        self.noises.is_empty()
    }
}

impl NoiseSampler for NoiseBank {
    fn sample(&self, noise: &str, x: f64, y: f64, z: f64) -> f64 {
        let noise_impl = self.noises.get(noise).unwrap_or_else(|| {
            panic!(
                "NoiseBank: unbound noise resource '{noise}' reached sampling; \
                 the linked settings and the bank are out of sync"
            )
        });
        // The stack returns f32; widen for scalar evaluation.
        f64::from(noise_impl.get(x, y, z))
    }

    fn sample_old_blended(
        &self,
        params: &OldBlendedParams,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<f64, crate::eval::EvalError> {
        let runtime = self.blended.get(&BlendedKey::of(params)).unwrap_or_else(|| {
            panic!(
                "NoiseBank: old_blended_noise params {params:?} were not prebuilt; \
                 the linked trees and the bank are out of sync"
            )
        });
        Ok(runtime.sample(x, y, z))
    }

    fn sample_end_islands(
        &self,
        block_x: i32,
        block_z: i32,
    ) -> Result<f64, crate::eval::EvalError> {
        // (getHeightValue(blockX / 8, blockZ / 8) - 8) / 128 with integer
        // division (truncating toward zero) and f32 math.
        let doffs = end_island_doffs(&self.island, block_x / 8, block_z / 8);
        Ok(f64::from(((doffs - 8.0) / 128.0) as f32))
    }
}

/// 末地岛屿高度值：在 ±12 区块窗口内取符合条件岛屿的最大偏移值。
///
/// End-island height value: the maximum offset among qualifying islands within
/// the ±12 chunk window.
fn end_island_doffs(noise: &SimplexNoise, section_x: i32, section_z: i32) -> f32 {
    let chunk_x = section_x / 2;
    let chunk_z = section_z / 2;
    let sub_x = section_x % 2;
    let sub_z = section_z % 2;
    let mut doffs = -100.0f32;
    for xo in -12..=12i32 {
        for zo in -12..=12i32 {
            let total_x = i64::from(chunk_x + xo);
            let total_z = i64::from(chunk_z + zo);
            if total_x * total_x + total_z * total_z > 4096
                && noise.get_2d(total_x as f64, total_z as f64) < -0.9
            {
                let island_size = (total_x.unsigned_abs() as f32 * 3439.0
                    + total_z.unsigned_abs() as f32 * 147.0)
                    % 13.0
                    + 9.0;
                let xd = (sub_x - xo * 2) as f32;
                let zd = (sub_z - zo * 2) as f32;
                let mut new_doffs = 100.0 - (xd * xd + zd * zd).sqrt() * island_size;
                new_doffs = new_doffs.clamp(-100.0, 80.0);
                if new_doffs > doffs {
                    doffs = new_doffs;
                }
            }
        }
    }
    doffs
}

// ---------------------------------------------------------------------------
// Tests: the bank's samplers must stay bit-deterministic for a given seed.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::EvalError;
    use crate::noise::PerlinNoise;

    fn test_params(
        base_octave: i32,
        base_amplitude: f64,
        octave_count: Option<i32>,
        modifiers: Option<Vec<f64>>,
    ) -> NoiseParams {
        NoiseParams {
            base_octave,
            base_amplitude,
            octave_count,
            amplitude_modifiers: modifiers,
            normalize: None,
        }
    }

    /// 按资源 id 走位置派生链构建噪声：世界位置工厂 → `noise_random(id)` →
    /// `NormalNoise26::new`，用于与噪声库的取值做对比。
    ///
    /// Builds the noise through the positional derivation chain for a resource
    /// id: world positional factory → `noise_random(id)` →
    /// `NormalNoise26::new`, used to compare against the bank's values.
    fn reference_normal26(
        seed: i64,
        id: &str,
        params: &NoiseParams,
    ) -> NormalNoise26 {
        let positional = WorldPositional::new(seed, false);
        let mut random = positional.noise_random(id);
        NormalNoise26::new(&to_params26(params), &mut random)
    }

    #[test]
    fn bank_matches_the_positional_reference_chain() {
        // Seed-binding contract: the bank value must equal a noise built from
        // the resource id on the world positional factory.
        let seed = 42i64;
        let id = "minecraft:test";
        let params = test_params(-6, 1.0, Some(3), Some(vec![1.0, 1.0, 1.0]));
        let mut all = BTreeMap::new();
        all.insert(id.to_string(), params.clone());
        let bank = NoiseBank::new(seed, &all, &[]);
        let reference = reference_normal26(seed, id, &params);
        for (x, y, z) in [(0.0, 0.0, 0.0), (12.5, -64.25, 3001.75), (-999.5, 63.0, 7.0)] {
            assert_eq!(
                bank.sample(id, x, y, z),
                f64::from(reference.get(x, y, z)),
                "bank value diverges from the positional chain"
            );
        }
    }

    #[test]
    fn bank_is_deterministic_across_instances() {
        let mut params = BTreeMap::new();
        params.insert(
            "minecraft:a".to_string(),
            test_params(-5, 0.5, Some(4), None),
        );
        params.insert(
            "minecraft:b".to_string(),
            test_params(-10, 1.25, None, Some(vec![1.0, 2.0])),
        );
        let bank1 = NoiseBank::new(777, &params, &[]);
        let bank2 = NoiseBank::new(777, &params, &[]);
        for name in ["minecraft:a", "minecraft:b"] {
            for (x, y, z) in [(1.0, 2.0, 3.0), (-4.5, 88.0, 0.25)] {
                assert_eq!(
                    bank1.sample(name, x, y, z),
                    bank2.sample(name, x, y, z)
                );
            }
        }
        // A different seed yields different values.
        let bank3 = NoiseBank::new(778, &params, &[]);
        assert_ne!(
            bank1.sample("minecraft:a", 1.0, 2.0, 3.0),
            bank3.sample("minecraft:a", 1.0, 2.0, 3.0)
        );
    }

    #[test]
    fn legacy_variant_changes_the_derivation() {
        // legacy_random_source swaps the positional factory; values differ
        // from the xoroshiro chain with the same seed.
        let mut params = BTreeMap::new();
        params.insert(
            "minecraft:a".to_string(),
            test_params(-5, 0.5, Some(4), None),
        );
        let bank = NoiseBank::with_legacy(777, &params, &[], true);
        let xoroshiro = NoiseBank::new(777, &params, &[]);
        assert_ne!(
            bank.sample("minecraft:a", 1.0, 2.0, 3.0),
            xoroshiro.sample("minecraft:a", 1.0, 2.0, 3.0)
        );
        // And the legacy bank is itself deterministic.
        let bank2 = NoiseBank::with_legacy(777, &params, &[], true);
        assert_eq!(
            bank.sample("minecraft:a", 1.0, 2.0, 3.0),
            bank2.sample("minecraft:a", 1.0, 2.0, 3.0)
        );
    }

    /// The overworld bundle's node shape (`overworld/base_3d_noise.json`).
    fn overworld_blend_params() -> OldBlendedParams {
        OldBlendedParams {
            xz_scale: 0.25,
            y_scale: 0.125,
            xz_factor: 80.0,
            y_factor: 160.0,
            smear_scale_multiplier: 8.0,
        }
    }

    #[test]
    fn old_blended_noise_is_finite_and_deterministic() {
        let bank = NoiseBank::new(2026, &BTreeMap::new(), &[overworld_blend_params()]);
        let params = overworld_blend_params();
        for (x, y, z) in [(0.0, 64.0, 0.0), (12345.0, -32.0, -54321.0), (-9999.0, 200.0, 9999.0)] {
            let value = bank
                .sample_old_blended(&params, x, y, z)
                .expect("bank must support old_blended_noise");
            assert!(value.is_finite());
        }
        // Deterministic across banks (per-shape prebuild from the same
        // world positional random).
        let bank2 = NoiseBank::new(2026, &BTreeMap::new(), &[overworld_blend_params()]);
        assert_eq!(
            bank.sample_old_blended(&params, 1.0, 2.0, 3.0).unwrap(),
            bank2.sample_old_blended(&params, 1.0, 2.0, 3.0).unwrap()
        );
        // A different seed changes the blended terrain.
        let bank3 = NoiseBank::new(2027, &BTreeMap::new(), &[overworld_blend_params()]);
        assert_ne!(
            bank.sample_old_blended(&params, 1.0, 2.0, 3.0).unwrap(),
            bank3.sample_old_blended(&params, 1.0, 2.0, 3.0).unwrap()
        );
    }

    #[test]
    fn default_trait_impl_stays_unsupported_for_stubs() {
        // The eval-side stub sampler keeps its documented gap.
        struct Stub;
        impl NoiseSampler for Stub {
            fn sample(&self, _noise: &str, _x: f64, _y: f64, _z: f64) -> f64 {
                0.0
            }
        }
        let error = Stub.sample_old_blended(
            &OldBlendedParams {
                xz_scale: 1.0,
                y_scale: 1.0,
                xz_factor: 1.0,
                y_factor: 1.0,
                smear_scale_multiplier: 1.0,
            },
            0.0,
            0.0,
            0.0,
        );
        assert!(matches!(error, Err(EvalError::Unsupported { node: "old_blended_noise" })));
    }

    #[test]
    fn sample_reports_unbound_resources_loudly() {
        let bank = NoiseBank::new(1, &BTreeMap::new(), &[]);
        let result = std::panic::catch_unwind(|| bank.sample("minecraft:missing", 0.0, 0.0, 0.0));
        assert!(result.is_err(), "unbound noise sampling must panic loudly");
    }

    #[test]
    fn perlin_stacks_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NoiseBank>();
        assert_send_sync::<PerlinNoise>();
    }
}
