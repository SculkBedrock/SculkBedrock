//! Perlin 噪声与多八度 Perlin 噪声采样器。
//!
//! Perlin noise and multi-octave Perlin noise samplers.

use super::noise::{Noise, skip_262_seed};
use super::random::RandomSourceProvider;

/// 把 `d` 缠绕进有界范围以保持噪声输入精度：
/// `d - floor(d / 2^25 + 0.5) * 2^25`。
///
/// Wraps `d` into a bounded range to keep noise inputs precise:
/// `d - floor(d / 2^25 + 0.5) * 2^25`.
#[inline]
pub(crate) fn maintain_precision(d: f64) -> f64 {
    let scaled = d / 3.3554432E7 + 0.5;
    let l = scaled as i64;
    let lfloor = if scaled >= l as f64 { l } else { l - 1 };
    d - lfloor as f64 * 3.3554432E7
}

/// 先 max 后 min 的钳制；该顺序决定了 NaN 输入下的取值。
///
/// Clamp that applies max before min; this order fixes the value returned for
/// NaN inputs.
#[inline]
#[allow(dead_code)]
pub(crate) fn clamp_f64(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

// ---------------------------------------------------------------------------
// PerlinNoiseSampler
// ---------------------------------------------------------------------------

/// 16 组平面梯度、每组 4 个分量（第 4 个恒为 0；采样只读前 3 个）。
///
/// 16 gradient sets of 4 components (4th always 0; sampling reads first 3).
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

/// 单个 Perlin 噪声采样器。
///
/// A single Perlin noise sampler.
pub struct PerlinNoiseSampler {
    /// 基类采样器状态（噪声原点与置换表）。
    ///
    /// Base sampler state (noise origin and permutation table).
    pub noise: Noise,
}

impl PerlinNoiseSampler {
    /// 用 `rand` 播种噪声原点与置换表。
    ///
    /// Seeds the noise origin and permutation table from `rand`.
    pub fn new<R: RandomSourceProvider>(rand: &mut R) -> Self {
        Self { noise: Noise::new(rand) }
    }

    /// 采样单点。`y_amplification` 非 0 时把 y 相对坐标量化到该标度，
    /// `min_y` 是量化的下钳位。
    ///
    /// Samples one point. A non-zero `y_amplification` quantizes the relative y
    /// coordinate to that scale, with `min_y` as the lower clamp.
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
            let y_clamp = if min_y >= 0.0 && min_y < local_y {
                min_y
            } else {
                local_y
            };
            y_offset = (y_clamp / y_amplification + 1.0E-7).floor() * y_amplification;
        }

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

    /// 格点角点梯度点积与 smoothstep 三线性混合的核心。
    /// `fade_local_y` 使用未量化的 y 相对坐标参与淡入。
    ///
    /// The core: corner gradient dot products blended with smoothstep
    /// trilinear interpolation. `fade_local_y` uses the un-quantized relative y
    /// coordinate for the fade.
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

/// 多八度 Perlin 噪声采样器：逐八度加权求和。
///
/// Multi-octave Perlin noise sampler: a weighted sum over octaves.
pub struct OctavePerlinNoiseSampler {
    pub lacunarity: f64,
    pub persistence: f64,
    /// 八度采样器数组（条目可能为空）。
    ///
    /// Octave sampler array (entries may be null).
    octave_samplers: Vec<Option<PerlinNoiseSampler>>,
    /// 每八度振幅列表；`None` 时每个八度按振幅 1 处理。
    ///
    /// Per-octave amplitude list; `None` means every octave uses amplitude 1.
    amplitudes_array: Option<Vec<f64>>,
    octave_samplers_count: usize,
}

/// 由八度列表推导 `(start, octave_places)`：`start` 为最小八度的相反数，
/// `octave_places` 在每个请求的八度位置置 1。
///
/// Derives `(start, octave_places)` from an octave list: `start` is the
/// negation of the smallest octave, and `octave_places` holds 1 at each
/// requested octave position.
fn make_amplitudes(octaves: &[i32]) -> (i32, Vec<f64>) {
    let mut sorted = octaves.to_vec();
    sorted.sort();
    assert!(!sorted.is_empty(), "Need some octaves!");
    let start = sorted[0].wrapping_neg();
    let end = sorted[sorted.len() - 1];
    let length = start.wrapping_add(end).wrapping_add(1);
    assert!(length >= 1, "Total number of octaves needs to be >= 1");
    let mut octave_places = vec![0.0; length as usize];
    for &octave in &sorted {
        octave_places[octave.wrapping_add(start) as usize] = 1.0;
    }
    (start, octave_places)
}

impl OctavePerlinNoiseSampler {
    /// 构建 `octave_count` 个八度；lacunarity/persistence 均为 1。
    ///
    /// Builds `octave_count` octaves with lacunarity and persistence both 1.
    pub fn new<R: RandomSourceProvider>(random: &mut R, octave_count: usize) -> Self {
        let octave_samplers = (0..octave_count)
            .map(|_| Some(PerlinNoiseSampler::new(random)))
            .collect();
        Self {
            lacunarity: 1.0,
            persistence: 1.0,
            octave_samplers,
            amplitudes_array: None,
            octave_samplers_count: octave_count,
        }
    }

    /// 按 `first_octave`（可为负）与每八度振幅列表构建；振幅列表长度决定
    /// 八度数量，振幅为 0 的槽位留空并按 `skip_262_seed` 推进随机种子。
    ///
    /// Builds from `first_octave` (possibly negative) and a per-octave amplitude
    /// list; the list length is the octave count, zero-amplitude slots stay
    /// empty and advance the random seed via `skip_262_seed`.
    pub fn new_with_first_octave<R: RandomSourceProvider>(
        rand: &mut R,
        first_octave: i32,
        amplitudes: Vec<f64>,
    ) -> Self {
        let start = first_octave;
        let perlin = PerlinNoiseSampler::new(rand);
        let perlin_zero_sample = perlin.sample(0.0, 0.0, 0.0, 0.0, 0.0);

        let length = amplitudes.len();
        let mut octave_samplers: Vec<Option<PerlinNoiseSampler>> =
            (0..length).map(|_| None).collect();

        if start >= 0 && (start as usize) < length {
            let d0 = amplitudes[start as usize];
            if d0 != 0.0 {
                octave_samplers[start as usize] = Some(perlin);
            }
        }

        for idx in (0..start).rev() {
            if (idx as usize) < length {
                let d1 = amplitudes[idx as usize];
                if d1 != 0.0 {
                    octave_samplers[idx as usize] = Some(PerlinNoiseSampler::new(rand));
                } else {
                    rand.set_seed(skip_262_seed(rand.get_seed()));
                }
            } else {
                rand.set_seed(skip_262_seed(rand.get_seed()));
            }
        }

        if start < length as i32 - 1 {
            let noise_seed = (perlin_zero_sample * 9.223372E18f32 as f64) as i64;
            rand.set_seed(noise_seed);
            for l in start.wrapping_add(1)..length as i32 {
                if l >= 0 {
                    let d2 = amplitudes[l as usize];
                    if d2 != 0.0 {
                        octave_samplers[l as usize] = Some(PerlinNoiseSampler::new(rand));
                    } else {
                        rand.set_seed(skip_262_seed(rand.get_seed()));
                    }
                } else {
                    rand.set_seed(skip_262_seed(rand.get_seed()));
                }
            }
        }

        let persistence = 2.0f64.powi(-start);
        let lacunarity = 2.0f64.powi(length as i32 - 1) / (2.0f64.powi(length as i32) - 1.0);
        Self {
            lacunarity,
            persistence,
            octave_samplers,
            amplitudes_array: Some(amplitudes),
            octave_samplers_count: length,
        }
    }

    /// 由八度索引列表构建（`old_blended_noise` 的 limit/main 八度使用）。
    /// 种子推进顺序依赖八度列表排序结果，必须保持一致。
    ///
    /// Builds from a list of octave indices (used by the limit/main octaves of
    /// `old_blended_noise`). The seed-advance order depends on the sorted octave
    /// list and must stay identical.
    pub fn new_old<R: RandomSourceProvider>(rand: &mut R, octaves: &[i32]) -> Self {
        let (start, octave_places) = make_amplitudes(octaves);
        // The old form derives end/length from the octave list itself.
        let mut sorted = octaves.to_vec();
        sorted.sort();
        let end = sorted[sorted.len() - 1];
        let length = octave_places.len() as i32;

        let perlin = PerlinNoiseSampler::new(rand);
        let perlin_zero_sample = perlin.sample(0.0, 0.0, 0.0, 0.0, 0.0);

        let mut octave_samplers: Vec<Option<PerlinNoiseSampler>> =
            (0..length as usize).map(|_| None).collect();

        if end >= 0 && end < length && octaves.contains(&0) {
            octave_samplers[end as usize] = Some(perlin);
        }

        for idx in end.wrapping_add(1)..length {
            if idx >= 0 && octaves.contains(&(end - idx)) {
                octave_samplers[idx as usize] = Some(PerlinNoiseSampler::new(rand));
            } else {
                rand.set_seed(skip_262_seed(rand.get_seed()));
            }
        }

        if end > 0 {
            let noise_seed = (perlin_zero_sample * 9.223372036854776E18f64) as i64;
            rand.set_seed(noise_seed);
            for index in (0..end).rev() {
                if index < length && octaves.contains(&(end - index)) {
                    octave_samplers[index as usize] = Some(PerlinNoiseSampler::new(rand));
                } else {
                    rand.set_seed(skip_262_seed(rand.get_seed()));
                }
            }
        }

        let persistence = 2.0f64.powi(end);
        let lacunarity = 1.0 / (2.0f64.powi(length) - 1.0);
        let _ = start;
        let octave_samplers_count = octave_samplers.len();
        Self {
            lacunarity,
            persistence,
            octave_samplers,
            amplitudes_array: None,
            octave_samplers_count,
        }
    }

    /// 无 y 放大、无 y 缺省的采样（等价于 `sample_full(..., 0.0, 0.0, false)`）。
    ///
    /// Sampling without y amplification or a defaulted y (same as
    /// `sample_full(..., 0.0, 0.0, false)`).
    pub fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        self.sample_full(x, y, z, 0.0, 0.0, false)
    }

    /// 逐八度累加：persistence 每次减半、lacunarity 每次加倍；
    /// 为空的八度槽位跳过。`use_default_y` 时该八度的 y 固定为其噪声原点。
    ///
    /// Accumulates octave by octave: persistence halves and lacunarity doubles
    /// each step; empty octave slots are skipped. With `use_default_y` the
    /// octave's y is fixed to its noise origin.
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
        let mut persistence = self.persistence;
        let mut lacunarity = self.lacunarity;

        let amplitudes = &self.amplitudes_array;
        for idx in 0..self.octave_samplers_count {
            if let Some(sampler) = &self.octave_samplers[idx] {
                let sample = sampler.sample(
                    maintain_precision(x * persistence),
                    if use_default_y {
                        -sampler.noise.origin_y
                    } else {
                        maintain_precision(y * persistence)
                    },
                    maintain_precision(z * persistence),
                    y_amplification * persistence,
                    min_y * persistence,
                ) * lacunarity;
                noise += amplitudes.as_ref().map(|a| a[idx]).unwrap_or(1.0) * sample;
            }
            persistence /= 2.0;
            lacunarity *= 2.0;
        }
        noise
    }

    /// 按索引查找八度。
    ///
    /// Octave lookup by index.
    pub fn get_octave(&self, octave: usize) -> Option<&PerlinNoiseSampler> {
        self.octave_samplers[octave].as_ref()
    }

    /// 八度槽位总数。
    ///
    /// Total octave slot count.
    pub fn get_count(&self) -> usize {
        self.octave_samplers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perlin_sampler_period_256() {
        let mut rand = super::super::random::Xoroshiro128::new(12345);
        let sampler = PerlinNoiseSampler::new(&mut rand);
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-10 * a.abs().max(1.0);
        for (x, y, z) in [(0.3, 1.7, 2.9), (-5.2, 0.0, 100.1), (63.9, -17.3, 0.5)] {
            let a = sampler.sample(x, y, z, 0.0, 0.0);
            assert!(close(a, sampler.sample(x + 256.0, y, z, 0.0, 0.0)), "x-axis period");
            assert!(close(a, sampler.sample(x, y + 256.0, z, 0.0, 0.0)), "y-axis period");
            assert!(close(a, sampler.sample(x, y, z + 256.0, 0.0, 0.0)), "z-axis period");
        }
    }

    #[test]
    fn perlin_sampler_deterministic() {
        use super::super::random::Xoroshiro128;
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
    }

    #[test]
    fn octave_old_method_layout() {
        use super::super::random::Xoroshiro128;
        // sorted [-3, 1, 2]: start=3, end=2, length=6.
        let mut rand = Xoroshiro128::new(5);
        let octave = OctavePerlinNoiseSampler::new_old(&mut rand, &[-3, 1, 2]);
        assert_eq!(octave.get_count(), 6);
        assert!(octave.get_octave(0).is_some());
        assert!(octave.get_octave(1).is_some());
        assert!(octave.get_octave(2).is_none());
        assert_eq!(octave.persistence, 2.0f64.powi(2));
        assert_eq!(octave.lacunarity, 1.0 / (2.0f64.powi(6) - 1.0));
        let mut rand2 = Xoroshiro128::new(5);
        let octave2 = OctavePerlinNoiseSampler::new_old(&mut rand2, &[-3, 1, 2]);
        assert_eq!(
            octave.sample(1.5, -2.5, 3.5),
            octave2.sample(1.5, -2.5, 3.5)
        );
    }

    #[test]
    fn octave_new_with_first_octave_zero_amplitude_slots() {
        use super::super::random::Xoroshiro128;
        let amplitudes = vec![1.0, 1.0, 0.5, 0.0, 0.25];
        let mut rand = Xoroshiro128::new(99);
        let octave =
            OctavePerlinNoiseSampler::new_with_first_octave(&mut rand, 2, amplitudes.clone());
        assert_eq!(octave.get_count(), 5);
        assert!(octave.get_octave(3).is_none(), "zero-amplitude octaves must be empty");
        assert_eq!(octave.persistence, 2.0f64.powi(-2));
        let v = octave.sample(1.0, 2.0, 3.0);
        let mut rand2 = Xoroshiro128::new(99);
        let octave2 =
            OctavePerlinNoiseSampler::new_with_first_octave(&mut rand2, 2, amplitudes);
        assert_eq!(v, octave2.sample(1.0, 2.0, 3.0));
    }

    #[test]
    fn make_amplitudes_marks_octave_slots() {
        let (start, places) = make_amplitudes(&[-2, 0, 3]);
        assert_eq!(start, 2);
        assert_eq!(places, vec![1.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
    }
}
