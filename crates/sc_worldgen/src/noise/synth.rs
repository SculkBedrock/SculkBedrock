//! 噪声合成采样器：梯度噪声、单形噪声、Perlin 噪声与噪声栈。
//!
//! Noise synth samplers: gradient noise, simplex noise, Perlin noise and
//! noise stacks.
//!
//! 这些是 [`crate::noise::normal26`] 统计栈背后的逐八度采样器。
//! 梯度噪声基类是共享的：三次 `nextDouble()` 抽取播种逐轴偏移，
//! 一次 `nextInt(256 - i)` Fisher-Yates 洗牌填充 `perms`。
//!
//! These are the per-octave samplers behind the statistical
//! [`crate::noise::normal26`] stack. The gradient-noise base is shared: three
//! `nextDouble()` draws seed the per-axis offsets and a `nextInt(256 - i)`
//! Fisher-Yates shuffle fills `perms`.

use super::legacy_random::LegacyRandom;
use super::positional::XoroshiroRandomSource;

/// 16 个梯度向量表（按 `hash & 15` 索引）。
///
/// The 16-entry gradient vector table, indexed by `hash & 15`.
const GRADIENT: [(i32, i32, i32); 16] = [
    (1, 1, 0),
    (-1, 1, 0),
    (1, -1, 0),
    (-1, -1, 0),
    (1, 0, 1),
    (-1, 0, 1),
    (1, 0, -1),
    (-1, 0, -1),
    (0, 1, 1),
    (0, -1, 1),
    (0, 1, -1),
    (0, -1, -1),
    (1, 1, 0),
    (0, -1, 1),
    (-1, 1, 0),
    (0, -1, -1),
];

/// 坐标回绕阈值：紧邻 2^24 之下的 f64（`nextDown(1.6777216E7)`）。
///
/// Coordinate wrap threshold: the f64 directly below 2^24
/// (`nextDown(1.6777216E7)`).
const HALF_ROUND_OFF: f64 = f64::from_bits(0x416F_FFFF_FFFF_FFFF);

/// 梯度噪声基类消费的随机数接口。
///
/// Random number interface consumed by the gradient-noise base.
pub trait GradientRng {
    fn next_double(&mut self) -> f64;
    fn next_int_bounded(&mut self, bound: i32) -> i32;
}

impl GradientRng for LegacyRandom {
    fn next_double(&mut self) -> f64 {
        LegacyRandom::next_double(self)
    }
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        LegacyRandom::next_int_bounded(self, bound)
    }
}

impl GradientRng for XoroshiroRandomSource {
    fn next_double(&mut self) -> f64 {
        XoroshiroRandomSource::next_double(self)
    }
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        XoroshiroRandomSource::next_int_bounded(self, bound)
    }
}

impl GradientRng for super::positional::RngSource {
    fn next_double(&mut self) -> f64 {
        super::positional::RngSource::next_double(self)
    }
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        super::positional::RngSource::next_int_bounded(self, bound)
    }
}

/// 梯度噪声基类：逐轴坐标偏移 + 256 项置换表。
///
/// Gradient-noise base: per-axis coordinate offsets plus a 256-entry
/// permutation table.
#[derive(Clone, Debug)]
pub struct GradientNoiseBase {
    offset_x: f64,
    offset_y: f64,
    offset_z: f64,
    perms: [u8; 256],
}

impl GradientNoiseBase {
    /// 用 `rng` 播种偏移（乘以 `offset_scale`）并洗牌置换表。
    ///
    /// Seeds the offsets from `rng` (scaled by `offset_scale`) and shuffles
    /// the permutation table.
    pub fn new(rng: &mut impl GradientRng, offset_scale: f64) -> Self {
        let offset_x = rng.next_double() * offset_scale;
        let offset_y = rng.next_double() * offset_scale;
        let offset_z = rng.next_double() * offset_scale;
        let mut perms = [0u8; 256];
        for (i, slot) in perms.iter_mut().enumerate() {
            *slot = i as u8;
        }
        for i in 0..256usize {
            let offset = rng.next_int_bounded((256 - i) as i32) as usize;
            perms.swap(i, offset + i);
        }
        Self { offset_x, offset_y, offset_z, perms }
    }

    /// 置换查找：按无符号字节取 `perms[x & 0xFF]`。
    ///
    /// Permutation lookup: `perms[x & 0xFF]` as an unsigned byte.
    #[inline]
    fn permute(&self, x: i32) -> i32 {
        i32::from(self.perms[(x & 0xFF) as usize])
    }

    /// 梯度向量与 (x, y, z) 的 f32 点积，向量按 `hash & 15` 选择。
    ///
    /// f32 dot product of the gradient selected by `hash & 15` with (x, y, z).
    #[inline]
    fn grad_dot(hash: i32, x: f32, y: f32, z: f32) -> f32 {
        let (gx, gy, gz) = GRADIENT[(hash & 15) as usize];
        (gx as f32) * x + (gy as f32) * y + (gz as f32) * z
    }

    /// 把超出 ±`HALF_ROUND_OFF` 的坐标回绕到 3.3554432E7 周期内。
    ///
    /// Wraps coordinates beyond ±`HALF_ROUND_OFF` into the 3.3554432E7 period.
    #[inline]
    fn wrap(x: f64) -> f64 {
        if x >= -HALF_ROUND_OFF && x < HALF_ROUND_OFF {
            x
        } else {
            x - (x / 3.3554432E7 + 0.5).floor() * 3.3554432E7
        }
    }
}

/// 单形噪声：末地岛屿使用其 2D 路径。
///
/// Simplex noise; end islands use the 2D path.
#[derive(Clone, Debug)]
pub struct SimplexNoise {
    base: GradientNoiseBase,
}

const SQRT_3: f64 = 1.7320508075688772;
const F2: f64 = 0.5 * (SQRT_3 - 1.0);
const G2: f64 = (3.0 - SQRT_3) / 6.0;

impl SimplexNoise {
    /// 构建单形噪声；`discard_noise_offset` 为真时把偏移标度置 0。
    ///
    /// Builds simplex noise; `discard_noise_offset` zeroes the offset scale.
    pub fn new(rng: &mut impl GradientRng, discard_noise_offset: bool) -> Self {
        let scale = if discard_noise_offset { 0.0 } else { 256.0 };
        Self { base: GradientNoiseBase::new(rng, scale) }
    }

    fn get_corner_noise_3d(index: usize, x: f64, y: f64, z: f64, base: f64) -> f64 {
        let t0 = base - x * x - y * y - z * z;
        if t0 < 0.0 {
            0.0
        } else {
            let t0 = t0 * t0;
            let (gx, gy, gz) = GRADIENT[index & 15];
            t0 * t0 * (f64::from(gx) * x + f64::from(gy) * y + f64::from(gz) * z)
        }
    }

    /// 2D 单形采样，返回 f32。
    ///
    /// Two-dimensional simplex sample, returned as f32.
    pub fn get_2d(&self, x_in: f64, y_in: f64) -> f32 {
        let base = &self.base;
        let xin = x_in + base.offset_x;
        let yin = y_in + base.offset_y;
        let s = (xin + yin) * F2;
        let i = (xin + s).floor() as i32;
        let j = (yin + s).floor() as i32;
        let t = f64::from(i + j) * G2;
        let x0 = xin - (f64::from(i) - t);
        let y0 = yin - (f64::from(j) - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - f64::from(i1) + G2;
        let y1 = y0 - f64::from(j1) + G2;
        let x2 = x0 - 1.0 + 2.0 * G2;
        let y2 = y0 - 1.0 + 2.0 * G2;
        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let gi0 = (base.permute(ii + base.permute(jj)) % 12) as usize;
        let gi1 = (base.permute(ii + i1 + base.permute(jj + j1)) % 12) as usize;
        let gi2 = (base.permute(ii + 1 + base.permute(jj + 1)) % 12) as usize;
        let n0 = Self::get_corner_noise_3d(gi0, x0, y0, 0.0, 0.5);
        let n1 = Self::get_corner_noise_3d(gi1, x1, y1, 0.0, 0.5);
        let n2 = Self::get_corner_noise_3d(gi2, x2, y2, 0.0, 0.5);
        (70.0 * (n0 + n1 + n2)) as f32
    }
}

/// Perlin 噪声采样器：f32 点积 + smoothstep + 三线性 lerp。
///
/// Perlin noise sampler: f32 dot products, smoothstep and trilinear lerp.
#[derive(Clone, Debug)]
pub struct PerlinNoise {
    base: GradientNoiseBase,
}

impl PerlinNoise {
    pub fn new(rng: &mut impl GradientRng) -> Self {
        Self { base: GradientNoiseBase::new(rng, 256.0) }
    }

    fn sample_and_lerp(
        &self,
        x: i32,
        y: i32,
        z: i32,
        relative_x: f32,
        relative_y: f32,
        relative_z: f32,
        original_relative_y: f32,
    ) -> f32 {
        let base = &self.base;
        let x0 = base.permute(x);
        let x1 = base.permute(x + 1);
        let xy00 = base.permute(x0 + y);
        let xy01 = base.permute(x0 + y + 1);
        let xy10 = base.permute(x1 + y);
        let xy11 = base.permute(x1 + y + 1);
        let d000 = GradientNoiseBase::grad_dot(base.permute(xy00 + z), relative_x, relative_y, relative_z);
        let d100 = GradientNoiseBase::grad_dot(base.permute(xy10 + z), relative_x - 1.0, relative_y, relative_z);
        let d010 = GradientNoiseBase::grad_dot(base.permute(xy01 + z), relative_x, relative_y - 1.0, relative_z);
        let d110 = GradientNoiseBase::grad_dot(base.permute(xy11 + z), relative_x - 1.0, relative_y - 1.0, relative_z);
        let d001 = GradientNoiseBase::grad_dot(base.permute(xy00 + z + 1), relative_x, relative_y, relative_z - 1.0);
        let d101 = GradientNoiseBase::grad_dot(base.permute(xy10 + z + 1), relative_x - 1.0, relative_y, relative_z - 1.0);
        let d011 = GradientNoiseBase::grad_dot(base.permute(xy01 + z + 1), relative_x, relative_y - 1.0, relative_z - 1.0);
        let d111 = GradientNoiseBase::grad_dot(base.permute(xy11 + z + 1), relative_x - 1.0, relative_y - 1.0, relative_z - 1.0);
        let x_alpha = smoothstep(relative_x);
        let y_alpha = smoothstep(original_relative_y);
        let z_alpha = smoothstep(relative_z);
        lerp3(
            x_alpha,
            y_alpha,
            z_alpha,
            d000,
            d100,
            d010,
            d110,
            d001,
            d101,
            d011,
            d111,
        )
    }

    /// 3D Perlin 采样：坐标回绕 + 偏移后取格点，角点值用 f32 混合。
    ///
    /// Three-dimensional sample: wrapped and offset coordinates are split into
    /// a lattice cell whose corners are blended in f32.
    pub fn get_3d(&self, x_in: f64, y_in: f64, z_in: f64) -> f32 {
        let base = &self.base;
        let x = GradientNoiseBase::wrap(x_in) + base.offset_x;
        let y = GradientNoiseBase::wrap(y_in) + base.offset_y;
        let z = GradientNoiseBase::wrap(z_in) + base.offset_z;
        let floor_x = x.floor() as i32;
        let floor_y = y.floor() as i32;
        let floor_z = z.floor() as i32;
        let relative_x = (x - f64::from(floor_x)) as f32;
        let relative_y = (y - f64::from(floor_y)) as f32;
        let relative_z = (z - f64::from(floor_z)) as f32;
        self.sample_and_lerp(
            floor_x,
            floor_y,
            floor_z,
            relative_x,
            relative_y,
            relative_z,
            relative_y,
        )
    }

    /// 2D Perlin 采样：以 y = 0 复用 3D 路径。
    ///
    /// Two-dimensional sample: reuses the 3D path with y = 0.
    pub fn get_2d(&self, x_in: f64, y_in: f64) -> f32 {
        self.get_3d(GradientNoiseBase::wrap(x_in), 0.0, GradientNoiseBase::wrap(y_in))
    }

    /// 共享的角点采样核心（[`SmearedPerlinNoise`] 也使用）。
    ///
    /// Shared corner-sampling core (also used by [`SmearedPerlinNoise`]).
    pub(crate) fn sample_and_lerp_pub(
        &self,
        x: i32,
        y: i32,
        z: i32,
        relative_x: f32,
        relative_y: f32,
        relative_z: f32,
        original_relative_y: f32,
    ) -> f32 {
        self.sample_and_lerp(x, y, z, relative_x, relative_y, relative_z, original_relative_y)
    }
}

/// 平滑插值曲线 `6x^5 - 15x^4 + 10x^3`（f32）。
///
/// Smoothing curve `6x^5 - 15x^4 + 10x^3` (f32).
#[inline]
fn smoothstep(x: f32) -> f32 {
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// 线性插值 `a + alpha * (b - a)`（f32）。
///
/// Linear interpolation `a + alpha * (b - a)` (f32).
#[inline]
fn lerp(alpha: f32, a: f32, b: f32) -> f32 {
    a + alpha * (b - a)
}

/// 双线性插值（f32）。
///
/// Bilinear interpolation (f32).
#[inline]
fn lerp2(a1: f32, a2: f32, x00: f32, x10: f32, x01: f32, x11: f32) -> f32 {
    lerp(a2, lerp(a1, x00, x10), lerp(a1, x01, x11))
}

/// 三线性插值（f32）；混合顺序固定，结果依赖它。
///
/// Trilinear interpolation (f32); the blending order is fixed and observable.
#[inline]
fn lerp3(
    a1: f32,
    a2: f32,
    a3: f32,
    x000: f32,
    x100: f32,
    x010: f32,
    x110: f32,
    x001: f32,
    x101: f32,
    x011: f32,
    x111: f32,
) -> f32 {
    lerp(
        a3,
        lerp2(a1, a2, x000, x100, x010, x110),
        lerp2(a1, a2, x001, x101, x011, x111),
    )
}

/// 涂抹版 Perlin 噪声：y 方向单元格采样被"涂抹"因子扰动，
/// 供 [`crate::noise::bank`] 的 Fbm 栈使用。
///
/// Smeared Perlin noise: the y-cell sampling is perturbed by a fudge factor,
/// used by the Fbm stacks of [`crate::noise::bank`].
#[derive(Clone, Debug)]
pub struct SmearedPerlinNoise {
    perlin: PerlinNoise,
    fudge_y_scale: f64,
}

/// y 方向涂抹取整时加上的 epsilon。
///
/// Epsilon added when rounding the y-direction smear.
const SHIFT_UP_EPSILON: f64 = 1.0E-7;

impl SmearedPerlinNoise {
    pub fn new(rng: &mut impl GradientRng, fudge_y_scale: f64) -> Self {
        Self { perlin: PerlinNoise::new(rng), fudge_y_scale }
    }

    fn compute_fudge_y(&self, original_y: f64, relative_y: f64) -> f64 {
        let fudge_limit = if original_y >= 0.0 && original_y < relative_y {
            original_y
        } else {
            relative_y
        };
        (fudge_limit / self.fudge_y_scale + SHIFT_UP_EPSILON).floor() * self.fudge_y_scale
    }

    /// 3D 采样：先用涂抹因子扰动 y 相对坐标，再复用 Perlin 的角点核心。
    ///
    /// Three-dimensional sample: perturbs the relative y coordinate with the
    /// fudge factor, then reuses the Perlin corner core.
    pub fn get_3d(&self, x_in: f64, y_in: f64, z_in: f64) -> f32 {
        let base = &self.perlin.base;
        let x = GradientNoiseBase::wrap(x_in) + base.offset_x;
        let y = GradientNoiseBase::wrap(y_in) + base.offset_y;
        let z = GradientNoiseBase::wrap(z_in) + base.offset_z;
        let floor_x = x.floor() as i32;
        let floor_y = y.floor() as i32;
        let floor_z = z.floor() as i32;
        let relative_x = (x - f64::from(floor_x)) as f32;
        let relative_y = y - f64::from(floor_y);
        let relative_z = (z - f64::from(floor_z)) as f32;
        let fudged_relative_y = (relative_y - self.compute_fudge_y(y_in, relative_y)) as f32;
        self.perlin.sample_and_lerp_pub(
            floor_x,
            floor_y,
            floor_z,
            relative_x,
            fudged_relative_y,
            relative_z,
            relative_y as f32,
        )
    }

    /// 文档声明的取值范围上界。
    ///
    /// Documented range bound.
    pub fn max(&self) -> f64 {
        self.fudge_y_scale.abs() + 2.0
    }
}

/// 噪声栈的层采样器种类。
///
/// A stack sampler kind for noise stack layers.
#[derive(Clone, Debug)]
pub enum StackNoise {
    Perlin(PerlinNoise),
    Smeared(SmearedPerlinNoise),
}

impl StackNoise {
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        match self {
            Self::Perlin(noise) => noise.get_3d(x, y, z),
            Self::Smeared(noise) => noise.get_3d(x, y, z),
        }
    }
}

/// 噪声栈：(采样器, 频率, 振幅) 层的 f32 累加。
/// 统计噪声栈使用 [`PerlinNoise`] 层，`BlendedNoise` 形态的 Fbm 栈使用
/// [`SmearedPerlinNoise`] 层。
///
/// A noise stack: f32 accumulation over (sampler, frequency, amplitude)
/// layers. The statistical stacks use [`PerlinNoise`] layers; Fbm stacks use
/// [`SmearedPerlinNoise`] layers.
#[derive(Clone, Debug)]
pub struct NoiseStack {
    layers: Vec<(StackNoise, f64, f32)>,
}

impl NoiseStack {
    pub fn new(layers: Vec<(StackNoise, f64, f32)>) -> Self {
        Self { layers }
    }

    /// 把本栈的层**展平**追加到 `out`，频率乘以 `frequency`、振幅乘以
    /// `amplitude`（f32 乘法）。累加顺序可观测，因此不得建模为嵌套层。
    ///
    /// **Flattens** this stack's layers into `out` with
    /// `frequency * outerFrequency` and `amplitude * outerAmplitude` (f32
    /// multiply). The accumulation order is observable, so this must not be
    /// modelled as a nested layer.
    pub fn flatten_into(&self, out: &mut Vec<(StackNoise, f64, f32)>, frequency: f64, amplitude: f32) {
        for (noise, inner_frequency, inner_amplitude) in &self.layers {
            out.push((
                noise.clone(),
                inner_frequency * frequency,
                inner_amplitude * amplitude,
            ));
        }
    }

    /// 原始层列表（供展平/合并）。
    ///
    /// The raw layers (for flattening and merging).
    pub fn layers(&self) -> &[(StackNoise, f64, f32)] {
        &self.layers
    }

    /// 按层顺序累加 `amplitude * sample(x*f, y*f, z*f)`，全程 f32。
    ///
    /// Accumulates `amplitude * sample(x*f, y*f, z*f)` in layer order, all in
    /// f32.
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut value = 0.0f32;
        for (noise, frequency, amplitude) in &self.layers {
            let frequency = *frequency;
            value += *amplitude * noise.get(x * frequency, y * frequency, z * frequency);
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::positional::XoroshiroRandomSource;

    #[test]
    fn perlin_ranges_are_bounded_and_deterministic() {
        let mut rng = XoroshiroRandomSource::from_legacy_seed(42);
        let perlin1 = PerlinNoise::new(&mut rng);
        let perlin2 = PerlinNoise::new(&mut rng);
        assert_ne!(
            perlin1.get_3d(1.0, 2.0, 3.0),
            perlin2.get_3d(1.0, 2.0, 3.0),
            "different permutation tables must produce different values"
        );
        let mut rng = XoroshiroRandomSource::from_legacy_seed(7);
        let perlin = PerlinNoise::new(&mut rng);
        for i in 0..64 {
            let x = i as f64 * 0.37;
            let v = perlin.get_3d(x, x * 0.5, -x);
            assert!(v.is_finite());
            assert!(v.abs() <= 2.0 + 1e-6, "perlin out of the documented range: {v}");
        }
    }

    #[test]
    fn noise_stack_sums_layers_in_f32() {
        let mut rng = XoroshiroRandomSource::from_legacy_seed(11);
        let perlin = PerlinNoise::new(&mut rng);
        // Single layer: the stack value equals amplitude * raw sample.
        let stack = NoiseStack::new(vec![(StackNoise::Perlin(perlin.clone()), 1.0, 0.5)]);
        let scaled = 0.5 * perlin.get_3d(2.0, 3.0, 4.0);
        assert_eq!(stack.get(2.0, 3.0, 4.0), scaled);
    }
}
