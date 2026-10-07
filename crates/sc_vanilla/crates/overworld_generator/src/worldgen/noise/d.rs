//! Port of the double-precision noise family (used by the legacy generator).
//!
//! - `NoiseGeneratorImprovedD.java` → [`NoiseGeneratorImprovedD`]
//! - `NoiseGeneratorOctavesD.java` → [`NoiseGeneratorOctavesD`]
//! - `NoiseGeneratorPerlinD.java` → [`NoiseGeneratorPerlinD`]
//! - `NoiseGeneratorSimplexD.java` → [`NoiseGeneratorSimplexD`]
//!
//! Reference sources live under `.fetch/`.
//!
//! Parameterless constructors (`System.currentTimeMillis()` seeds) are not
//! ported; deterministic generation only uses explicit
//! `RandomSourceProvider` constructors.

use crate::worldgen::math::floor_double_long;
use crate::worldgen::random::RandomSourceProvider;

// ---------------------------------------------------------------------------
// NoiseGeneratorImprovedD
// NoiseGeneratorImprovedD.
// ---------------------------------------------------------------------------

/// Java: `NoiseGeneratorImprovedD.GRAD_X`(L10).
const GRAD_X: [f64; 16] = [
    1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0,
];
/// Java: `NoiseGeneratorImprovedD.GRAD_Y`(L11).
const GRAD_Y: [f64; 16] = [
    1.0, 1.0, -1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0,
];
/// Java: `NoiseGeneratorImprovedD.GRAD_Z`(L12).
const GRAD_Z: [f64; 16] = [
    0.0, 0.0, 0.0, 0.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 0.0, 1.0, 0.0, -1.0,
];
/// Same values as `GRAD_X`.
const GRAD_2X: [f64; 16] = GRAD_X;
/// Same values as `GRAD_Z`.
const GRAD_2Z: [f64; 16] = GRAD_Z;

/// Java: `NoiseGeneratorImprovedD`.
pub struct NoiseGeneratorImprovedD {
    /// Shuffled permutation table (512 entries).
    permutations: Vec<i32>,
    /// Java: `public double xCoord`.
    pub x_coord: f64,
    /// Java: `public double yCoord`.
    pub y_coord: f64,
    /// Java: `public double zCoord`.
    pub z_coord: f64,
}

impl NoiseGeneratorImprovedD {
    /// Java: `NoiseGeneratorImprovedD(RandomSourceProvider)`(L24-42).
    ///
    /// Randomness order: 3x `nextDouble` (offsets), then 256x `nextBoundedInt` (shuffle).
    pub fn new<R: RandomSourceProvider>(random: &mut R) -> Self {
        let mut permutations = vec![0i32; 512];
        let x_coord = random.next_double() * 256.0;
        let y_coord = random.next_double() * 256.0;
        let z_coord = random.next_double() * 256.0;

        let mut i = 0usize;
        while i < 256 {
            permutations[i] = i as i32;
            i += 1;
        }

        for l in 0..256usize {
            let j = (random.next_bounded_int(256 - l as i32) + l as i32) as usize;
            let k = permutations[l];
            permutations[l] = permutations[j];
            permutations[j] = k;
            permutations[l + 256] = permutations[l];
        }

        Self {
            permutations,
            x_coord,
            y_coord,
            z_coord,
        }
    }

    /// Java: `public final double lerp(double, double, double)`(L44-46).
    #[inline]
    pub fn lerp(t: f64, a: f64, b: f64) -> f64 {
        a + t * (b - a)
    }

    /// Java: `public final double grad2(int, double, double)`(L48-51).
    #[inline]
    pub fn grad2(hash: i32, x: f64, z: f64) -> f64 {
        let i = (hash & 15) as usize;
        GRAD_2X[i] * x + GRAD_2Z[i] * z
    }

    /// Java: `public final double grad(int, double, double, double)`(L53-56).
    #[inline]
    pub fn grad(hash: i32, x: f64, y: f64, z: f64) -> f64 {
        let i = (hash & 15) as usize;
        GRAD_X[i] * x + GRAD_Y[i] * y + GRAD_Z[i] * z
    }

    /// Java: `public void populateNoiseArray(...)`(L61-180).
    ///
    /// Output length must be `xSize * ySize * zSize`; results accumulate
    /// (`+=`) into `noise_array`, x-outer / z-middle / y-inner.
    /// (The 2D branch with `ySize == 1` is x-outer / z-inner.)
    ///
    /// Upstream declares placeholder cache variables outside; this port silences
    /// the resulting assigned-never-read warnings.
    #[allow(clippy::too_many_arguments)]
    #[allow(unused_assignments)]
    pub fn populate_noise_array(
        &self,
        noise_array: &mut [f64],
        x_offset: f64,
        y_offset: f64,
        z_offset: f64,
        x_size: usize,
        y_size: usize,
        z_size: usize,
        x_scale: f64,
        y_scale: f64,
        z_scale: f64,
        noise_scale: f64,
    ) {
        let perm = &self.permutations;

        if y_size == 1 {
            // 2D branch (ySize == 1).
            let mut l5 = 0usize;
            let d16 = 1.0 / noise_scale;

            for j2 in 0..x_size {
                let mut d17 = x_offset + j2 as f64 * x_scale + self.x_coord;
                let mut i6 = d17 as i32;
                if d17 < i6 as f64 {
                    i6 -= 1;
                }

                let k2 = (i6 & 255) as usize;
                d17 = d17 - i6 as f64;
                let d18 = d17 * d17 * d17 * (d17 * (d17 * 6.0 - 15.0) + 10.0);

                for j6 in 0..z_size {
                    let mut d19 = z_offset + j6 as f64 * z_scale + self.z_coord;
                    let mut k6 = d19 as i32;
                    if d19 < k6 as f64 {
                        k6 -= 1;
                    }

                    let l6 = (k6 & 255) as usize;
                    d19 = d19 - k6 as f64;
                    let d20 = d19 * d19 * d19 * (d19 * (d19 * 6.0 - 15.0) + 10.0);

                    let i5 = perm[k2] as usize;
                    let j5 = (perm[i5] + l6 as i32) as usize;
                    let j = perm[k2 + 1] as usize;
                    let k5 = (perm[j] + l6 as i32) as usize;

                    let d14 = Self::lerp(
                        d18,
                        Self::grad2(perm[j5], d17, d19),
                        Self::grad(perm[k5], d17 - 1.0, 0.0, d19),
                    );
                    let d15 = Self::lerp(
                        d18,
                        Self::grad(perm[j5 + 1], d17, 0.0, d19 - 1.0),
                        Self::grad(perm[k5 + 1], d17 - 1.0, 0.0, d19 - 1.0),
                    );
                    let d21 = Self::lerp(d20, d14, d15);
                    noise_array[l5] += d21 * d16;
                    l5 += 1;
                }
            }
        } else {
            // 3D branch.
            let mut i = 0usize;
            let d0 = 1.0 / noise_scale;
            let mut k: i32 = -1;
            let mut l = 0usize;
            let mut i1 = 0usize;
            let mut j1 = 0usize;
            let mut k1 = 0usize;
            let mut l1 = 0usize;
            let mut i2 = 0usize;
            let mut d1 = 0.0f64;
            let mut d2 = 0.0f64;
            let mut d3 = 0.0f64;
            let mut d4 = 0.0f64;

            for l2 in 0..x_size {
                let mut d5 = x_offset + l2 as f64 * x_scale + self.x_coord;
                let mut i3 = d5 as i32;
                if d5 < i3 as f64 {
                    i3 -= 1;
                }

                let j3 = (i3 & 255) as usize;
                d5 = d5 - i3 as f64;
                let d6 = d5 * d5 * d5 * (d5 * (d5 * 6.0 - 15.0) + 10.0);

                for k3 in 0..z_size {
                    let mut d7 = z_offset + k3 as f64 * z_scale + self.z_coord;
                    let mut l3 = d7 as i32;
                    if d7 < l3 as f64 {
                        l3 -= 1;
                    }

                    let i4 = (l3 & 255) as usize;
                    d7 = d7 - l3 as f64;
                    let d8 = d7 * d7 * d7 * (d7 * (d7 * 6.0 - 15.0) + 10.0);

                    for j4 in 0..y_size {
                        let mut d9 = y_offset + j4 as f64 * y_scale + self.y_coord;
                        let mut k4 = d9 as i32;
                        if d9 < k4 as f64 {
                            k4 -= 1;
                        }

                        let l4 = k4 & 255;
                        d9 = d9 - k4 as f64;
                        let d10 = d9 * d9 * d9 * (d9 * (d9 * 6.0 - 15.0) + 10.0);

                        // Y-layer cache, recomputed on grid change.
                        if j4 == 0 || l4 != k {
                            k = l4;
                            l = (perm[j3] + l4) as usize;
                            i1 = (perm[l] + i4 as i32) as usize;
                            j1 = (perm[l + 1] + i4 as i32) as usize;
                            k1 = (perm[j3 + 1] + l4) as usize;
                            l1 = (perm[k1] + i4 as i32) as usize;
                            i2 = (perm[k1 + 1] + i4 as i32) as usize;
                            d1 = Self::lerp(
                                d6,
                                Self::grad(perm[i1], d5, d9, d7),
                                Self::grad(perm[l1], d5 - 1.0, d9, d7),
                            );
                            d2 = Self::lerp(
                                d6,
                                Self::grad(perm[j1], d5, d9 - 1.0, d7),
                                Self::grad(perm[i2], d5 - 1.0, d9 - 1.0, d7),
                            );
                            d3 = Self::lerp(
                                d6,
                                Self::grad(perm[i1 + 1], d5, d9, d7 - 1.0),
                                Self::grad(perm[l1 + 1], d5 - 1.0, d9, d7 - 1.0),
                            );
                            d4 = Self::lerp(
                                d6,
                                Self::grad(perm[j1 + 1], d5, d9 - 1.0, d7 - 1.0),
                                Self::grad(perm[i2 + 1], d5 - 1.0, d9 - 1.0, d7 - 1.0),
                            );
                        }

                        let d11 = Self::lerp(d10, d1, d2);
                        let d12 = Self::lerp(d10, d3, d4);
                        let d13 = Self::lerp(d8, d11, d12);
                        noise_array[i] += d13 * d0;
                        i += 1;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// NoiseGeneratorOctavesD
// NoiseGeneratorOctavesD.
// ---------------------------------------------------------------------------

/// Java: `NoiseGeneratorOctavesD`.
pub struct NoiseGeneratorOctavesD {
    /// Java: `private final NoiseGeneratorImprovedD[] generatorCollection`.
    generator_collection: Vec<NoiseGeneratorImprovedD>,
    /// Java: `private final int octaves`.
    octaves: usize,
}

impl NoiseGeneratorOctavesD {
    /// Java: `NoiseGeneratorOctavesD(RandomSourceProvider, int)`(L13-20).
    pub fn new<R: RandomSourceProvider>(seed: &mut R, octaves_in: usize) -> Self {
        Self {
            octaves: octaves_in,
            generator_collection: (0..octaves_in)
                .map(|_| NoiseGeneratorImprovedD::new(seed))
                .collect(),
        }
    }

    /// Java: `generateNoiseOctaves(int, int, int, int, int, int, double, double, double)`
    ///(L26-48).
    ///
    /// par2/3/4 are noise offsets stitching adjacent segments;
    /// par5/6/7 = x/y/zArraySize,par8/10/12 = x/y/z noiseScale.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_noise_octaves(
        &self,
        x_offset: i32,
        y_offset: i32,
        z_offset: i32,
        x_size: usize,
        y_size: usize,
        z_size: usize,
        x_scale: f64,
        y_scale: f64,
        z_scale: f64,
    ) -> Vec<f64> {
        let mut noise_array = vec![0.0f64; x_size * y_size * z_size];

        let mut d3 = 1.0f64;

        for j in 0..self.octaves {
            let mut d0 = x_offset as f64 * d3 * x_scale;
            let d1 = y_offset as f64 * d3 * y_scale;
            let mut d2 = z_offset as f64 * d3 * z_scale;
            let mut k = floor_double_long(d0);
            let mut l = floor_double_long(d2);
            d0 = d0 - k as f64;
            d2 = d2 - l as f64;
            // `k % 16777216L` (remainders may be negative; Rust `%` matches).
            k %= 16777216;
            l %= 16777216;
            d0 = d0 + k as f64;
            d2 = d2 + l as f64;
            self.generator_collection[j].populate_noise_array(
                &mut noise_array,
                d0,
                d1,
                d2,
                x_size,
                y_size,
                z_size,
                x_scale * d3,
                y_scale * d3,
                z_scale * d3,
                d3,
            );
            d3 /= 2.0;
        }

        noise_array
    }

    /// Java: `generateNoiseOctaves(int, int, int, int, double, double, double)`
    /// "Bouncer function" with fixed `yOffset=10, ySize=1, yScale=1.0`.
    ///
    /// The 7th parameter is unused upstream and kept for parity.
    pub fn generate_noise_octaves_xz(
        &self,
        x_offset: i32,
        z_offset: i32,
        x_size: usize,
        z_size: usize,
        x_scale: f64,
        z_scale: f64,
        _p_76305_10_: f64,
    ) -> Vec<f64> {
        self.generate_noise_octaves(
            x_offset, 10, z_offset, x_size, 1, z_size, x_scale, 1.0, z_scale,
        )
    }
}

// ---------------------------------------------------------------------------
// NoiseGeneratorSimplexD
// NoiseGeneratorSimplexD.
// ---------------------------------------------------------------------------

/// Java: `NoiseGeneratorSimplexD.SQRT_3`(L9)——`Math.sqrt(3.0D)`.
const SQRT_3: f64 = 1.7320508075688772;
/// Java: `NoiseGeneratorSimplexD.grad3`(L10)——private static.
const GRAD3: [[i32; 3]; 12] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
];
/// Java: `NoiseGeneratorSimplexD.F2`(L11).
const F2: f64 = 0.5 * (SQRT_3 - 1.0);
/// Java: `NoiseGeneratorSimplexD.G2`(L12).
const G2: f64 = (3.0 - SQRT_3) / 6.0;

/// Java: `NoiseGeneratorSimplexD`.
pub struct NoiseGeneratorSimplexD {
    /// Shuffled permutation table (512 entries).
    p: Vec<i32>,
    /// Java: `public double xo`.
    pub xo: f64,
    /// Java: `public double yo`.
    pub yo: f64,
    /// Java: `public double zo`.
    pub zo: f64,
}

impl NoiseGeneratorSimplexD {
    /// Java: `NoiseGeneratorSimplexD(RandomSourceProvider)`(L22-40).
    pub fn new<R: RandomSourceProvider>(rand: &mut R) -> Self {
        let mut p = vec![0i32; 512];
        let xo = rand.next_double() * 256.0;
        let yo = rand.next_double() * 256.0;
        let zo = rand.next_double() * 256.0;

        let mut i = 0usize;
        while i < 256 {
            p[i] = i as i32;
            i += 1;
        }

        for l in 0..256usize {
            let j = (rand.next_bounded_int(256 - l as i32) + l as i32) as usize;
            let k = p[l];
            p[l] = p[j];
            p[j] = k;
            p[l + 256] = p[l];
        }

        Self { p, xo, yo, zo }
    }

    /// Java: `private static int fastFloor(double)`(L42-44).
    ///
    /// Note: `value == 0.0` takes the else branch returning `-1` (kept for parity).
    #[inline]
    fn fast_floor(value: f64) -> i32 {
        if value > 0.0 {
            value as i32
        } else {
            value as i32 - 1
        }
    }

    /// Java: `private static double dot(int[], double, double)`(L46-48).
    #[inline]
    fn dot(g: &[i32; 3], x: f64, y: f64) -> f64 {
        g[0] as f64 * x + g[1] as f64 * y
    }

    /// Java: `public double getValue(double, double)`(L50-112).
    pub fn get_value(&self, x: f64, y: f64) -> f64 {
        // F2/G2 recompute locally (values match the constants).
        let d3 = 0.5 * (SQRT_3 - 1.0);
        let d4 = (x + y) * d3;
        let i = Self::fast_floor(x + d4);
        let j = Self::fast_floor(y + d4);
        let d5 = (3.0 - SQRT_3) / 6.0;
        // `(i + j)` addition wraps on overflow.
        let d6 = i.wrapping_add(j) as f64 * d5;
        let d7 = i as f64 - d6;
        let d8 = j as f64 - d6;
        let d9 = x - d7;
        let d10 = y - d8;

        // Middle-corner selection.
        let (j1, k1) = if d9 > d10 { (1i32, 0i32) } else { (0i32, 1i32) };

        let d11 = d9 - j1 as f64 + G2;
        let d12 = d10 - k1 as f64 + G2;
        let d13 = d9 - 1.0 + 2.0 * G2;
        let d14 = d10 - 1.0 + 2.0 * G2;
        let i1 = (i & 255) as usize;
        let j1m = (j & 255) as usize;
        let k1g = (self.p[i1 + self.p[j1m] as usize] % 12) as usize;
        let l1g = (self.p[i1 + j1 as usize + self.p[j1m + k1 as usize] as usize] % 12) as usize;
        let i2g = (self.p[i1 + 1 + self.p[j1m + 1] as usize] % 12) as usize;

        // Corner contributions (quartic falloff).
        let d15 = 0.5 - d9 * d9 - d10 * d10;
        let d0 = if d15 < 0.0 {
            0.0
        } else {
            let d15 = d15 * d15;
            d15 * d15 * Self::dot(&GRAD3[k1g], d9, d10)
        };

        let d16 = 0.5 - d11 * d11 - d12 * d12;
        let d1 = if d16 < 0.0 {
            0.0
        } else {
            let d16 = d16 * d16;
            d16 * d16 * Self::dot(&GRAD3[l1g], d11, d12)
        };

        let d17 = 0.5 - d13 * d13 - d14 * d14;
        let d2 = if d17 < 0.0 {
            0.0
        } else {
            let d17 = d17 * d17;
            d17 * d17 * Self::dot(&GRAD3[i2g], d13, d14)
        };

        70.0 * (d0 + d1 + d2)
    }

    /// Java: `public void add(double[], double, double, int, int, double, double, double)`
    ///(L113-183).
    ///
    /// Index layout: outer `z_size` (j), inner `x_size` (k);
    /// results accumulate (`+=`).
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &self,
        array: &mut [f64],
        x: f64,
        z: f64,
        x_size: usize,
        z_size: usize,
        x_scale: f64,
        z_scale: f64,
        scale: f64,
    ) {
        let mut i = 0usize;

        for j in 0..z_size {
            // Z-row coordinate (uses yo).
            let d0 = (z + j as f64) * z_scale + self.yo;

            for k in 0..x_size {
                // X-column coordinate (uses xo).
                let d1 = (x + k as f64) * x_scale + self.xo;
                let d5 = (d1 + d0) * F2;
                let l = Self::fast_floor(d1 + d5);
                let i1 = Self::fast_floor(d0 + d5);
                // `(l + i1)` addition wraps on overflow.
                let d6 = l.wrapping_add(i1) as f64 * G2;
                let d7 = l as f64 - d6;
                let d8 = i1 as f64 - d6;
                let d9 = d1 - d7;
                let d10 = d0 - d8;

                let (j1, k1) = if d9 > d10 { (1i32, 0i32) } else { (0i32, 1i32) };

                let d11 = d9 - j1 as f64 + G2;
                let d12 = d10 - k1 as f64 + G2;
                let d13 = d9 - 1.0 + 2.0 * G2;
                let d14 = d10 - 1.0 + 2.0 * G2;
                let l1 = (l & 255) as usize;
                let i2 = (i1 & 255) as usize;
                let j2 = (self.p[l1 + self.p[i2] as usize] % 12) as usize;
                let k2 =
                    (self.p[l1 + j1 as usize + self.p[i2 + k1 as usize] as usize] % 12) as usize;
                let l2 = (self.p[l1 + 1 + self.p[i2 + 1] as usize] % 12) as usize;

                let d15 = 0.5 - d9 * d9 - d10 * d10;
                let d2 = if d15 < 0.0 {
                    0.0
                } else {
                    let d15 = d15 * d15;
                    d15 * d15 * Self::dot(&GRAD3[j2], d9, d10)
                };

                let d16 = 0.5 - d11 * d11 - d12 * d12;
                let d3 = if d16 < 0.0 {
                    0.0
                } else {
                    let d16 = d16 * d16;
                    d16 * d16 * Self::dot(&GRAD3[k2], d11, d12)
                };

                let d17 = 0.5 - d13 * d13 - d14 * d14;
                let d4 = if d17 < 0.0 {
                    0.0
                } else {
                    let d17 = d17 * d17;
                    d17 * d17 * Self::dot(&GRAD3[l2], d13, d14)
                };

                array[i] += 70.0 * (d2 + d3 + d4) * scale;
                i += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// NoiseGeneratorPerlinD
// NoiseGeneratorPerlinD.
// ---------------------------------------------------------------------------

/// Java: `NoiseGeneratorPerlinD`.
pub struct NoiseGeneratorPerlinD {
    /// Java: `private final NoiseGeneratorSimplexD[] noiseLevels`.
    noise_levels: Vec<NoiseGeneratorSimplexD>,
    /// Java: `private final int levels`.
    levels: usize,
}

impl NoiseGeneratorPerlinD {
    /// Port of the `(Random, int)` constructor.
    pub fn new<R: RandomSourceProvider>(rand: &mut R, levels: usize) -> Self {
        Self {
            noise_levels: (0..levels)
                .map(|_| NoiseGeneratorSimplexD::new(rand))
                .collect(),
            levels,
        }
    }

    /// Java: `public double getValue(double, double)`(L19-29).
    pub fn get_value(&self, x: f64, y: f64) -> f64 {
        let mut d0 = 0.0f64;
        let mut d1 = 1.0f64;

        for i in 0..self.levels {
            d0 += self.noise_levels[i].get_value(x * d1, y * d1) / d1;
            d1 /= 2.0;
        }

        d0
    }

    /// Java: `getRegion(double[], double, double, int, int, double, double, double)`
    /// Delegates to the full form with lacunarity fixed at 0.5.
    #[allow(clippy::too_many_arguments)]
    pub fn get_region(
        &self,
        array: Option<Vec<f64>>,
        x: f64,
        y: f64,
        x_size: usize,
        z_size: usize,
        x_scale: f64,
        z_scale: f64,
        persistence: f64,
    ) -> Vec<f64> {
        self.get_region_with_lacunarity(
            array,
            x,
            y,
            x_size,
            z_size,
            x_scale,
            z_scale,
            persistence,
            0.5,
        )
    }

    /// Java: `getRegion(double[], double, double, int, int, double, double, double, double)`
    ///(L35-54).
    ///
    /// The last two parameters scale
    /// persistence (`d0 *=`) and lacunarity (`d1 *=`).
    #[allow(clippy::too_many_arguments)]
    pub fn get_region_with_lacunarity(
        &self,
        array: Option<Vec<f64>>,
        x: f64,
        y: f64,
        x_size: usize,
        z_size: usize,
        x_scale: f64,
        z_scale: f64,
        persistence: f64,
        lacunarity: f64,
    ) -> Vec<f64> {
        // Long-enough input arrays are zeroed and reused (keeping length).
        let mut array = match array {
            Some(a) if a.len() >= x_size * z_size => {
                let mut a = a;
                for v in a.iter_mut() {
                    *v = 0.0;
                }
                a
            }
            _ => vec![0.0f64; x_size * z_size],
        };

        let mut d1 = 1.0f64;
        let mut d0 = 1.0f64;

        for j in 0..self.levels {
            self.noise_levels[j].add(
                &mut array,
                x,
                y,
                x_size,
                z_size,
                x_scale * d0 * d1,
                z_scale * d0 * d1,
                0.55 / d1,
            );
            d0 *= persistence;
            d1 *= lacunarity;
        }

        array
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::random::Xoroshiro128;

    #[test]
    fn fast_floor_quirk() {
        // value > 0.0 ? (int)value : (int)value - 1; 0.0 maps to -1.
        assert_eq!(NoiseGeneratorSimplexD::fast_floor(0.0), -1);
        assert_eq!(NoiseGeneratorSimplexD::fast_floor(-0.0), -1);
        assert_eq!(NoiseGeneratorSimplexD::fast_floor(0.5), 0);
        assert_eq!(NoiseGeneratorSimplexD::fast_floor(1.7), 1);
        assert_eq!(NoiseGeneratorSimplexD::fast_floor(-1.2), -2);
        assert_eq!(NoiseGeneratorSimplexD::fast_floor(3.0), 3);
    }

    #[test]
    fn floor_double_long_semantics() {
        assert_eq!(floor_double_long(2.7), 2);
        assert_eq!(floor_double_long(-2.3), -3);
        assert_eq!(floor_double_long(0.0), 0);
        assert_eq!(floor_double_long(5.0), 5);
        assert_eq!(floor_double_long(-5.0), -5);
    }

    #[test]
    fn improved_d_permutation_layout() {
        let mut r = Xoroshiro128::new(11);
        let gen = NoiseGeneratorImprovedD::new(&mut r);
        // `j = nextBoundedInt(256 - l) + l` may reach 256, touching perm[256];
        // the low 256 slots are not a strict permutation (upstream quirk,
        // not standard Fisher-Yates).
        // Invariants that hold: all values still come from 0..=255;
        // high slots copied for l >= 1 are never touched again.
        assert_eq!(gen.permutations.len(), 512);
        assert!(gen.permutations.iter().all(|&v| (0..=255).contains(&v)));
        for l in 1..256usize {
            assert_eq!(gen.permutations[l + 256], gen.permutations[l]);
        }
        // Determinism: same seed rebuilds identically.
        let mut r2 = Xoroshiro128::new(11);
        let gen2 = NoiseGeneratorImprovedD::new(&mut r2);
        assert_eq!(gen.permutations, gen2.permutations);
    }

    #[test]
    fn improved_d_populate_2d_and_3d() {
        let mut r = Xoroshiro128::new(22);
        let gen = NoiseGeneratorImprovedD::new(&mut r);

        // 2D branch (ySize == 1): xSize*zSize, x-outer z-inner.
        let mut arr = vec![0.0f64; 4 * 5];
        gen.populate_noise_array(&mut arr, 0.0, 0.0, 0.0, 4, 1, 5, 1.0, 1.0, 1.0, 1.0);
        assert!(arr.iter().all(|v| v.is_finite()));
        assert!(arr.iter().any(|&v| v != 0.0));

        // Accumulation: a second run doubles values.
        let arr_once = arr.clone();
        gen.populate_noise_array(&mut arr, 0.0, 0.0, 0.0, 4, 1, 5, 1.0, 1.0, 1.0, 1.0);
        for (a, b) in arr.iter().zip(arr_once.iter()) {
            assert_eq!(*a, 2.0 * b);
        }

        // 3D branch: xSize*ySize*zSize.
        let mut arr3 = vec![0.0f64; 2 * 3 * 4];
        gen.populate_noise_array(&mut arr3, 0.0, 0.0, 0.0, 2, 3, 4, 1.0, 1.0, 1.0, 1.0);
        assert!(arr3.iter().all(|v| v.is_finite()));
        assert!(arr3.iter().any(|&v| v != 0.0));

        // Determinism.
        let mut r2 = Xoroshiro128::new(22);
        let gen2 = NoiseGeneratorImprovedD::new(&mut r2);
        let mut arr_b = vec![0.0f64; 4 * 5];
        gen2.populate_noise_array(&mut arr_b, 0.0, 0.0, 0.0, 4, 1, 5, 1.0, 1.0, 1.0, 1.0);
        assert_eq!(arr_once, arr_b);
    }

    #[test]
    fn simplex_d_deterministic_and_bounded() {
        let mut r1 = Xoroshiro128::new(33);
        let s1 = NoiseGeneratorSimplexD::new(&mut r1);
        let mut r2 = Xoroshiro128::new(33);
        let s2 = NoiseGeneratorSimplexD::new(&mut r2);
        assert_eq!(s1.p, s2.p);
        assert_eq!(s1.xo, s2.xo);
        assert_eq!(s1.yo, s2.yo);
        assert_eq!(s1.zo, s2.zo);

        for i in 0..200 {
            let x = i as f64 * 0.37;
            let y = i as f64 * 0.71;
            let v1 = s1.get_value(x, y);
            let v2 = s2.get_value(x, y);
            assert_eq!(v1, v2);
            assert!(v1.is_finite());
            // Classic 2D simplex range [-1, 1].
            assert!(v1.abs() <= 1.0 + 1e-9, "v={v1} outside [-1,1]");
        }

        // add() and getValue() differ: add() samples offset coordinates
        // while getValue() samples raw ones; not directly comparable.
        // add() scales linearly:
        let mut buf = [0.0f64; 1];
        s1.add(&mut buf, 3.25, -7.5, 1, 1, 1.0, 1.0, 1.0);
        assert!(buf[0].is_finite());
        let mut buf2 = [0.0f64; 1];
        s1.add(&mut buf2, 3.25, -7.5, 1, 1, 1.0, 1.0, 0.55);
        assert_eq!(buf2[0], 0.55 * buf[0]);
    }

    #[test]
    fn octaves_d_layout_and_determinism() {
        let mut r1 = Xoroshiro128::new(44);
        let o1 = NoiseGeneratorOctavesD::new(&mut r1, 4);
        let mut r2 = Xoroshiro128::new(44);
        let o2 = NoiseGeneratorOctavesD::new(&mut r2, 4);

        let a1 = o1.generate_noise_octaves(10, 20, 30, 3, 33, 3, 1.5, 2.5, 1.5);
        let a2 = o2.generate_noise_octaves(10, 20, 30, 3, 33, 3, 1.5, 2.5, 1.5);
        assert_eq!(a1.len(), 3 * 33 * 3);
        assert_eq!(a1, a2);
        assert!(a1.iter().all(|v| v.is_finite()));
        assert!(a1.iter().any(|&v| v != 0.0));

        // bouncer: yOffset=10/ySize=1/yScale=1, length xSize*zSize.
        let b = o1.generate_noise_octaves_xz(10, 30, 3, 3, 1.5, 1.5, 0.0);
        assert_eq!(b.len(), 3 * 3);
        let direct = o1.generate_noise_octaves(10, 10, 30, 3, 1, 3, 1.5, 1.0, 1.5);
        assert_eq!(b, direct);
    }

    #[test]
    fn perlin_d_region_and_value() {
        let mut r1 = Xoroshiro128::new(55);
        let p1 = NoiseGeneratorPerlinD::new(&mut r1, 3);
        let mut r2 = Xoroshiro128::new(55);
        let p2 = NoiseGeneratorPerlinD::new(&mut r2, 3);

        // getValue determinism.
        assert_eq!(p1.get_value(1.5, 2.5), p2.get_value(1.5, 2.5));
        assert!(p1.get_value(1.5, 2.5).is_finite());

        // get_region: None allocates w*h.
        let region = p1.get_region(None, 0.0, 0.0, 5, 7, 0.1, 0.2, 0.7);
        assert_eq!(region.len(), 5 * 7);
        assert!(region.iter().all(|v| v.is_finite()));
        assert!(region.iter().any(|&v| v != 0.0));

        // 8-arg get_region equals 9-arg with lacunarity=0.5.
        let region_full = p1.get_region_with_lacunarity(None, 0.0, 0.0, 5, 7, 0.1, 0.2, 0.7, 0.5);
        assert_eq!(region, region_full);

        // Reuse only when the input array holds w*h (zeroed, length kept).
        let reused = p1.get_region(Some(vec![9.0; 40]), 0.0, 0.0, 5, 7, 0.1, 0.2, 0.7);
        assert_eq!(reused.len(), 40, "reuses the input array (length kept)");
        // add() writes only the first w*h slots.
        assert!(reused[35..].iter().all(|&v| v == 0.0));
        assert!(reused[..35].iter().any(|&v| v != 0.0));

        // Some(short array): discarded, fresh w*h allocated.
        let too_short = p1.get_region(Some(vec![9.0; 12]), 0.0, 0.0, 5, 7, 0.1, 0.2, 0.7);
        assert_eq!(too_short.len(), 5 * 7);

        // Determinism.
        let region2 = p2.get_region(None, 0.0, 0.0, 5, 7, 0.1, 0.2, 0.7);
        assert_eq!(region, region2);
    }
}
