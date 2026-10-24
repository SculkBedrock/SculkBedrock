//! 单形噪声：`end_outer_islands` 使用的 2D 采样路径。
//!
//! Simplex noise: the 2D sampling path used by `end_outer_islands`.
//!
//! 末地岛屿按 `LegacyRandom(world_seed)` → `consume_count(17292)` →
//! `SimplexNoise::new(random, discard_noise_offset = true)` 构建它
//! （见 [`crate::eval`] 的 bank 集成与 `RandomState`），因此偏移标度为 **0.0**。
//!
//! End islands build it as `LegacyRandom(world_seed)` → `consume_count(17292)`
//! → `SimplexNoise::new(random, discard_noise_offset = true)` (see the bank
//! integration in [`crate::eval`] and `RandomState`), so the offset scale is
//! **0.0**.
//!
//! 数值合同：`get_2d` 与 `get_corner_noise_3d` 的全部运算都在 f64 中进行；
//! 只有返回值收窄为 f32（`(70.0 * (n0 + n1 + n2)) as f32`）。
//!
//! Numeric contract: all math in `get_2d` and `get_corner_noise_3d` runs in
//! f64; only the return value narrows to f32 (`(70.0 * (n0 + n1 + n2)) as f32`).

use super::legacy_random::LegacyRandom;

/// 16 组梯度向量表（按 `index & 15` 索引）。
///
/// The 16-entry gradient vector table, indexed by `index & 15`.
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

const SQRT_3: f64 = 1.7320508075688772; // f64::from(f32 sqrt3 const via literal)
const F2: f64 = 0.5 * (SQRT_3 - 1.0);
const G2: f64 = (3.0 - SQRT_3) / 6.0;

/// 单形噪声及其梯度噪声状态（偏移 + 置换表）；`end_outer_islands`
/// 是本模块唯一消费者。
///
/// Simplex noise with its gradient-noise state (offsets + permutation table);
/// `end_outer_islands` is the only consumer in this module.
#[derive(Clone, Debug)]
pub struct SimplexNoise {
    offset_x: f64,
    offset_y: f64,
    offset_z: f64,
    perms: [u8; 256],
}

impl SimplexNoise {
    /// 构建采样器：`discard_noise_offset` 为真时偏移标度取 0.0，否则取
    /// 256.0；随后消耗 3 次 `next_double` 与 256 次有界整数抽取。
    ///
    /// Builds the sampler: the offset scale is 0.0 when discarding, otherwise
    /// 256.0; three `next_double` draws and 256 bounded integer draws follow.
    pub fn new(random: &mut LegacyRandom, discard_noise_offset: bool) -> Self {
        let scale = if discard_noise_offset { 0.0 } else { 256.0 };
        let offset_x = random.next_double() * scale;
        let offset_y = random.next_double() * scale;
        let offset_z = random.next_double() * scale;

        let mut perms = [0u8; 256];
        for (i, slot) in perms.iter_mut().enumerate() {
            *slot = i as u8;
        }
        for i in 0..256usize {
            // Bounded draw over the range 256 - i.
            let offset = random.next_int_bounded((256 - i) as i32) as usize;
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

    /// 单个角点贡献：`base - x^2 - y^2 - z^2` 为负时返回 0，否则是该量的
    /// 四次方乘以梯度点积（全程 f64）。
    ///
    /// One corner contribution: 0 when `base - x^2 - y^2 - z^2` is negative,
    /// otherwise that quantity to the fourth times the gradient dot product
    /// (all f64).
    fn get_corner_noise_3d(
        index: usize,
        x: f64,
        y: f64,
        z: f64,
        base: f64,
    ) -> f64 {
        let t0 = base - x * x - y * y - z * z;
        if t0 < 0.0 {
            0.0
        } else {
            let t0 = t0 * t0;
            let (gx, gy, gz) = GRADIENT[index & 15];
            t0 * t0 * (f64::from(gx) * x + f64::from(gy) * y + f64::from(gz) * z)
        }
    }

    /// 2D 单形采样（末地岛屿使用的路径）；结果乘以 70 后收窄为 f32。
    ///
    /// Two-dimensional simplex sample (the path end islands use); the result is
    /// scaled by 70 and narrowed to f32.
    pub fn get_2d(&self, x_in: f64, y_in: f64) -> f32 {
        let xin = x_in + self.offset_x;
        let yin = y_in + self.offset_y;
        let s = (xin + yin) * F2;
        let i = (xin + s).floor() as i32;
        let j = (yin + s).floor() as i32;
        let t = f64::from(i + j) * G2;
        let x0_i = f64::from(i) - t;
        let y0_i = f64::from(j) - t;
        let x0 = xin - x0_i;
        let y0 = yin - y0_i;

        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };

        let x1 = x0 - f64::from(i1) + G2;
        let y1 = y0 - f64::from(j1) + G2;
        let x2 = x0 - 1.0 + 2.0 * G2;
        let y2 = y0 - 1.0 + 2.0 * G2;

        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let gi0 = (self.permute(ii + self.permute(jj)) % 12) as usize;
        let gi1 = (self.permute(ii + i1 + self.permute(jj + j1)) % 12) as usize;
        let gi2 = (self.permute(ii + 1 + self.permute(jj + 1)) % 12) as usize;

        let n0 = Self::get_corner_noise_3d(gi0, x0, y0, 0.0, 0.5);
        let n1 = Self::get_corner_noise_3d(gi1, x1, y1, 0.0, 0.5);
        let n2 = Self::get_corner_noise_3d(gi2, x2, y2, 0.0, 0.5);
        (70.0 * (n0 + n1 + n2)) as f32
    }

    /// 3D 单形采样；末地岛屿不使用。结果乘以 32 后收窄为 f32。
    ///
    /// Three-dimensional simplex sample, unused by end islands. The result is
    /// scaled by 32 and narrowed to f32.
    #[allow(dead_code)]
    pub fn get_3d(&self, x_in: f64, y_in: f64, z_in: f64) -> f32 {
        let xin = x_in + self.offset_x;
        let yin = y_in + self.offset_y;
        let zin = z_in + self.offset_z;
        let f3 = 0.3333333333333333f64;
        let s = (xin + yin + zin) * f3;
        let i = (xin + s).floor() as i32;
        let j = (yin + s).floor() as i32;
        let k = (zin + s).floor() as i32;
        let g3 = 0.16666666666666666f64;
        let t = f64::from(i + j + k) * g3;
        let x0 = xin - (f64::from(i) - t);
        let y0 = yin - (f64::from(j) - t);
        let z0 = zin - (f64::from(k) - t);

        let (i1, j1, k1, i2, j2, k2) = if x0 >= y0 {
            if y0 >= z0 {
                (1, 0, 0, 1, 1, 0)
            } else if x0 >= z0 {
                (1, 0, 0, 1, 0, 1)
            } else {
                (0, 0, 1, 1, 0, 1)
            }
        } else if y0 >= z0 {
            (0, 1, 0, 0, 1, 1)
        } else if x0 >= z0 {
            (0, 1, 0, 0, 1, 1)
        } else {
            (0, 0, 1, 0, 1, 1)
        };

        let x1 = x0 - f64::from(i1) + g3;
        let y1 = y0 - f64::from(j1) + g3;
        let z1 = z0 - f64::from(k1) + g3;
        let x2 = x0 - f64::from(i2) + 2.0 * g3;
        let y2 = y0 - f64::from(j2) + 2.0 * g3;
        let z2 = z0 - f64::from(k2) + 2.0 * g3;
        let x3 = x0 - 1.0 + 3.0 * g3;
        let y3 = y0 - 1.0 + 3.0 * g3;
        let z3 = z0 - 1.0 + 3.0 * g3;

        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let kk = k & 0xFF;
        let gi0 = (self.permute(ii + self.permute(jj + self.permute(kk))) % 12) as usize;
        let gi1 = (self.permute(ii + i1 + self.permute(jj + j1 + self.permute(kk + k1))) % 12) as usize;
        let gi2 = (self.permute(ii + i2 + self.permute(jj + j2 + self.permute(kk + k2))) % 12) as usize;
        let gi3 = (self.permute(ii + 1 + self.permute(jj + 1 + self.permute(kk + 1))) % 12) as usize;

        let n0 = Self::get_corner_noise_3d(gi0, x0, y0, z0, 0.6);
        let n1 = Self::get_corner_noise_3d(gi1, x1, y1, z1, 0.6);
        let n2 = Self::get_corner_noise_3d(gi2, x2, y2, z2, 0.6);
        let n3 = Self::get_corner_noise_3d(gi3, x3, y3, z3, 0.6);
        (32.0 * (n0 + n1 + n2 + n3)) as f32
    }
}
