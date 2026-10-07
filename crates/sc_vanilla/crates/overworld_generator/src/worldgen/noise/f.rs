//! Port of the float-precision noise family.
//!
//! - `NoiseF.java` (abstract base) → [`NoiseF`] (fields) + [`NoiseFSampler`] (virtuals + octave loops)
//! - `PerlinF.java` → [`PerlinF`]
//! - `SimplexF.java` → [`SimplexF`]
//!
//! Reference sources live under `.fetch/`.
//!
//! The inheritance chain `SimplexF extends PerlinF extends NoiseF` becomes
//! composition in Rust: `SimplexF { perlin: PerlinF }`, `PerlinF { base: NoiseF }`;
//! abstract `getNoise2D/getNoise3D` plus concrete `noise2D/noise3D` (octave loops)
//! live on the [`NoiseFSampler`] trait.
//!
//! Constructor overloads map to Rust names:
//! - `new(random, octaves, persistence)` ← `(RandomSourceProvider, float, float)`
//! - `new_with_expansion(random, octaves, persistence, expansion)`
//!   ← `(RandomSourceProvider, float, float, float)`

use crate::worldgen::random::RandomSourceProvider;

// ---------------------------------------------------------------------------
// NoiseF (abstract base as a field struct plus utilities).
// NoiseF.
// ---------------------------------------------------------------------------

/// Instance-field half of the abstract base.
///
/// `protected` fields become `pub(crate)` (noise family only).
#[derive(Clone, Debug)]
pub struct NoiseF {
    /// Permutation table (filled by the `PerlinF` constructor, 512 long).
    pub(crate) perm: Vec<i32>,
    /// Java: `protected float offsetX = 0`.
    pub(crate) offset_x: f32,
    /// Java: `protected float offsetY = 0`.
    pub(crate) offset_y: f32,
    /// Java: `protected float offsetZ = 0`.
    pub(crate) offset_z: f32,
    /// Octave count as float (loop comparisons use float semantics).
    pub(crate) octaves: f32,
    /// Java: `protected float persistence`.
    pub(crate) persistence: f32,
    /// Java: `protected float expansion`.
    pub(crate) expansion: f32,
}

impl NoiseF {
    /// Java: `public static int floor(float)`(L16-18).
    pub fn floor(x: f32) -> i32 {
        if x >= 0.0 {
            x as i32
        } else {
            (x - 1.0) as i32
        }
    }

    /// Java: `public static float fade(float)`(L20-22).
    pub fn fade(x: f32) -> f32 {
        x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
    }

    /// Java: `public static float lerp(float, float, float)`(L24-26).
    pub fn lerp(x: f32, y: f32, z: f32) -> f32 {
        y + x * (z - y)
    }

    /// Java: `public static float linearLerp(float, float, float, float, float)`(L28-30).
    pub fn linear_lerp(x: f32, x1: f32, x2: f32, q0: f32, q1: f32) -> f32 {
        ((x2 - x) / (x2 - x1)) * q0 + ((x - x1) / (x2 - x1)) * q1
    }

    /// Java: `public static float bilinearLerp(...)`(L32-41).
    #[allow(clippy::too_many_arguments)]
    pub fn bilinear_lerp(
        x: f32,
        y: f32,
        q00: f32,
        q01: f32,
        q10: f32,
        q11: f32,
        x1: f32,
        x2: f32,
        y1: f32,
        y2: f32,
    ) -> f32 {
        let dx1 = (x2 - x) / (x2 - x1);
        let dx2 = (x - x1) / (x2 - x1);

        ((y2 - y) / (y2 - y1)) * (dx1 * q00 + dx2 * q10)
            + ((y - y1) / (y2 - y1)) * (dx1 * q01 + dx2 * q11)
    }

    /// Java: `public static float trilinearLerp(...)`(L43-62).
    #[allow(clippy::too_many_arguments)]
    pub fn trilinear_lerp(
        x: f32,
        y: f32,
        z: f32,
        q000: f32,
        q001: f32,
        q010: f32,
        q011: f32,
        q100: f32,
        q101: f32,
        q110: f32,
        q111: f32,
        x1: f32,
        x2: f32,
        y1: f32,
        y2: f32,
        z1: f32,
        z2: f32,
    ) -> f32 {
        let dx1 = (x2 - x) / (x2 - x1);
        let dx2 = (x - x1) / (x2 - x1);
        let dy1 = (y2 - y) / (y2 - y1);
        let dy2 = (y - y1) / (y2 - y1);

        ((z2 - z) / (z2 - z1)) * (dy1 * (dx1 * q000 + dx2 * q100) + dy2 * (dx1 * q001 + dx2 * q101))
            + ((z - z1) / (z2 - z1))
                * (dy1 * (dx1 * q010 + dx2 * q110) + dy2 * (dx1 * q011 + dx2 * q111))
    }

    /// Java: `public static float grad(int, float, float, float)`(L64-71).
    pub fn grad(hash: i32, x: f32, y: f32, z: f32) -> f32 {
        let hash = hash & 15;
        let u = if hash < 8 { x } else { y };
        let v = if hash < 4 {
            y
        } else if hash == 12 || hash == 14 {
            x
        } else {
            z
        };

        (if (hash & 1) == 0 { u } else { -u }) + (if (hash & 2) == 0 { v } else { -v })
    }

    /// Java: `public void setOffset(float, float, float)`(L132-136).
    pub fn set_offset(&mut self, x: f32, y: f32, z: f32) {
        self.offset_x = x;
        self.offset_y = y;
        self.offset_z = z;
    }
}

// ---------------------------------------------------------------------------
// NoiseFSampler (abstracts plus octave loops as a trait).
// Abstract methods and concrete noise2D/noise3D.
// ---------------------------------------------------------------------------

/// Abstract `getNoise2D/getNoise3D` plus concrete octave-loop methods.
/// (`noise2D/noise3D`).
///
/// Virtual dispatch lives on the trait: `noise2D/noise3D` are default methods
/// dispatching through `self.get_noise_2d()/get_noise_3d()`.
pub trait NoiseFSampler {
    /// Inherited base fields via composition.
    fn base(&self) -> &NoiseF;

    /// Java: `abstract public float getNoise2D(float, float)`(L73).
    fn get_noise_2d(&self, x: f32, z: f32) -> f32;

    /// Java: `abstract public float getNoise3D(float, float, float)`(L75).
    fn get_noise_3d(&self, x: f32, y: f32, z: f32) -> f32;

    /// Java: `public float noise2D(float, float)`(L77-79).
    fn noise_2d(&self, x: f32, z: f32) -> f32 {
        self.noise_2d_normalized(x, z, false)
    }

    /// Java: `public float noise2D(float, float, boolean)`(L81-102).
    fn noise_2d_normalized(&self, x: f32, z: f32, normalized: bool) -> f32 {
        let mut result = 0.0f32;
        let mut amp = 1.0f32;
        let mut freq = 1.0f32;
        let mut max = 0.0f32;

        let x = x * self.base().expansion;
        let z = z * self.base().expansion;

        // Loop counter widens to float for the comparison.
        let mut i = 0i32;
        while (i as f32) < self.base().octaves {
            result += self.get_noise_2d(x * freq, z * freq) * amp;
            max += amp;
            freq *= 2.0;
            amp *= self.base().persistence;
            i += 1;
        }

        if normalized {
            result /= max;
        }

        result
    }

    /// Java: `public float noise3D(float, float, float)`(L104-106).
    fn noise_3d(&self, x: f32, y: f32, z: f32) -> f32 {
        self.noise_3d_normalized(x, y, z, false)
    }

    /// Java: `public float noise3D(float, float, float, boolean)`(L108-130).
    fn noise_3d_normalized(&self, x: f32, y: f32, z: f32, normalized: bool) -> f32 {
        let mut result = 0.0f32;
        let mut amp = 1.0f32;
        let mut freq = 1.0f32;
        let mut max = 0.0f32;

        let x = x * self.base().expansion;
        let y = y * self.base().expansion;
        let z = z * self.base().expansion;

        // Same int/float comparison semantics as noise2D.
        let mut i = 0i32;
        while (i as f32) < self.base().octaves {
            result += self.get_noise_3d(x * freq, y * freq, z * freq) * amp;
            max += amp;
            freq *= 2.0;
            amp *= self.base().persistence;
            i += 1;
        }

        if normalized {
            result /= max;
        }

        result
    }
}

// ---------------------------------------------------------------------------
// PerlinF
// PerlinF.
// ---------------------------------------------------------------------------

/// Java: `PerlinF extends NoiseF`.
///
/// Inherited fields via [`NoiseF`] composition (`self.base`).
pub struct PerlinF {
    /// Inherited base state.
    pub base: NoiseF,
}

impl PerlinF {
    /// Java: `PerlinF(RandomSourceProvider, float, float)`(L11-13).
    pub fn new<R: RandomSourceProvider>(random: &mut R, octaves: f32, persistence: f32) -> Self {
        Self::new_with_expansion(random, octaves, persistence, 1.0)
    }

    /// Java: `PerlinF(RandomSourceProvider, float, float, float)`(L15-33).
    ///
    /// Note `perm[i] = random.nextBoundedInt(255)`: the range is
    /// **[0, 255] inclusive**, so the first pass holds random values,
    /// not a permutation (unlike the minecraft-layer shuffle);
    /// the second pass shuffles into the high 256 slots.
    pub fn new_with_expansion<R: RandomSourceProvider>(
        random: &mut R,
        octaves: f32,
        persistence: f32,
        expansion: f32,
    ) -> Self {
        let offset_x = random.next_float() * 256.0;
        let offset_y = random.next_float() * 256.0;
        let offset_z = random.next_float() * 256.0;

        let mut perm = vec![0i32; 512];
        for i in 0..256usize {
            perm[i] = random.next_bounded_int(255);
        }
        for i in 0..256usize {
            let pos = (random.next_bounded_int(255 - i as i32) + i as i32) as usize;
            let old = perm[i];
            perm[i] = perm[pos];
            perm[pos] = old;
            perm[i + 256] = perm[i];
        }

        Self {
            base: NoiseF {
                perm,
                offset_x,
                offset_y,
                offset_z,
                octaves,
                persistence,
                expansion,
            },
        }
    }

    /// Java: `public float getValue(float, float, float)`(L35-37).
    pub fn get_value(&self, x: f32, y: f32, z: f32) -> f32 {
        self.get_noise_3d(x, y, z)
    }
}

impl NoiseFSampler for PerlinF {
    fn base(&self) -> &NoiseF {
        &self.base
    }

    /// 2D delegates to 3D.
    fn get_noise_2d(&self, x: f32, y: f32) -> f32 {
        self.get_noise_3d(x, y, 0.0)
    }

    /// Java: `@Override getNoise3D(float, float, float)`(L45-96).
    fn get_noise_3d(&self, x: f32, y: f32, z: f32) -> f32 {
        let x = x + self.base.offset_x;
        let y = y + self.base.offset_y;
        let z = z + self.base.offset_z;

        // Truncating cast (not floor).
        let floor_x = x as i32;
        let floor_y = y as i32;
        let floor_z = z as i32;

        let x_hi = floor_x & 0xFF;
        let y_hi = floor_y & 0xFF;
        let z_hi = floor_z & 0xFF;

        let x = x - floor_x as f32;
        let y = y - floor_y as f32;
        let z = z - floor_z as f32;

        // Fade curve (inlined, no fade() call).
        let f_x = x * x * x * (x * (x * 6.0 - 15.0) + 10.0);
        let f_y = y * y * y * (y * (y * 6.0 - 15.0) + 10.0);
        let f_z = z * z * z * (z * (z * 6.0 - 15.0) + 10.0);

        let perm = &self.base.perm;

        // Cube-corner hashing.
        let a = perm[x_hi as usize] + y_hi;
        let b = perm[(x_hi + 1) as usize] + y_hi;

        let aa = perm[a as usize] + z_hi;
        let ab = perm[(a + 1) as usize] + z_hi;
        let ba = perm[b as usize] + z_hi;
        let bb = perm[(b + 1) as usize] + z_hi;

        // Eight-corner gradients.
        let aa1 = NoiseF::grad(perm[aa as usize], x, y, z);
        let ba1 = NoiseF::grad(perm[ba as usize], x - 1.0, y, z);
        let ab1 = NoiseF::grad(perm[ab as usize], x, y - 1.0, z);
        let bb1 = NoiseF::grad(perm[bb as usize], x - 1.0, y - 1.0, z);
        let aa2 = NoiseF::grad(perm[(aa + 1) as usize], x, y, z - 1.0);
        let ba2 = NoiseF::grad(perm[(ba + 1) as usize], x - 1.0, y, z - 1.0);
        let ab2 = NoiseF::grad(perm[(ab + 1) as usize], x, y - 1.0, z - 1.0);
        let bb2 = NoiseF::grad(perm[(bb + 1) as usize], x - 1.0, y - 1.0, z - 1.0);

        // Trilinear interpolation.
        let x_lerp11 = aa1 + f_x * (ba1 - aa1);

        let z_lerp1 = x_lerp11 + f_y * (ab1 + f_x * (bb1 - ab1) - x_lerp11);

        let x_lerp21 = aa2 + f_x * (ba2 - aa2);

        z_lerp1 + f_z * (x_lerp21 + f_y * (ab2 + f_x * (bb2 - ab2) - x_lerp21) - z_lerp1)
    }
}

// ---------------------------------------------------------------------------
// SimplexF
// SimplexF.
// ---------------------------------------------------------------------------

/// Java: `SimplexF.grad3`(L11-15)——public static.
pub const GRAD3: [[i32; 3]; 12] = [
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

/// Java: `SimplexF.SQRT_3`(L31)——`(float) Math.sqrt(3)`.
///
/// (f64 value rounded to nearest f32; tests assert bitwise equality.)
pub const SQRT_3: f32 = 1.7320508;
/// `(float) Math.sqrt(5)` (4D-path spare, kept for parity).
#[allow(dead_code)]
pub const SQRT_5: f32 = 2.2360680;
/// Java: `SimplexF.F2`(L33)——`0.5f * (SQRT_3 - 1f)`.
pub const F2: f32 = 0.5 * (SQRT_3 - 1.0);
/// Java: `SimplexF.G2`(L34)——`(3f - SQRT_3) / 6f`.
pub const G2: f32 = (3.0 - SQRT_3) / 6.0;
/// Java: `SimplexF.G22`(L35)——`G2 * 2.0f - 1f`.
pub const G22: f32 = G2 * 2.0 - 1.0;
/// Java: `SimplexF.F3`(L36)——`1.0f / 3.0f`.
pub const F3: f32 = 1.0 / 3.0;
/// Java: `SimplexF.G3`(L37)——`1.0f / 6.0f`.
pub const G3: f32 = 1.0 / 6.0;
/// `(SQRT_5 - 1.0f) / 4.0f` (4D-path spare).
#[allow(dead_code)]
pub const F4: f32 = (SQRT_5 - 1.0) / 4.0;
/// `(5.0f - SQRT_5) / 20.0f` (4D-path spare).
#[allow(dead_code)]
pub const G4: f32 = (5.0 - SQRT_5) / 20.0;
/// `G4 * 2.0f` (4D-path spare).
#[allow(dead_code)]
pub const G42: f32 = G4 * 2.0;
/// `G4 * 3.0f` (4D-path spare).
#[allow(dead_code)]
pub const G43: f32 = G4 * 3.0;
/// `G4 * 4.0f - 1.0f` (4D-path spare).
#[allow(dead_code)]
pub const G44: f32 = G4 * 4.0 - 1.0;

/// Java: `SimplexF extends PerlinF`.
///
/// Inheritance chain via composition: `self.perlin.base`.
pub struct SimplexF {
    /// Parent (`PerlinF` plus its `NoiseF` fields).
    pub perlin: PerlinF,
    /// Java: `protected final float offsetW`.
    #[allow(dead_code)]
    pub(crate) offset_w: f32,
}

impl SimplexF {
    /// Java: `SimplexF(RandomSourceProvider, float, float)`(L45-48).
    pub fn new<R: RandomSourceProvider>(random: &mut R, octaves: f32, persistence: f32) -> Self {
        Self::new_with_expansion(random, octaves, persistence, 1.0)
    }

    /// Java: `SimplexF(RandomSourceProvider, float, float, float)`(L50-53).
    pub fn new_with_expansion<R: RandomSourceProvider>(
        random: &mut R,
        octaves: f32,
        persistence: f32,
        expansion: f32,
    ) -> Self {
        let perlin = PerlinF::new_with_expansion(random, octaves, persistence, expansion);
        let offset_w = random.next_float() * 256.0;
        Self { perlin, offset_w }
    }

    /// Inherited from `PerlinF.getValue(float, float, float)`,
    /// virtually dispatching to this override of `getNoise3D`.
    pub fn get_value(&self, x: f32, y: f32, z: f32) -> f32 {
        self.get_noise_3d(x, y, z)
    }

    /// Java: `protected static float dot2D(int[], float, float)`(L56-58).
    pub fn dot_2d(g: &[i32; 3], x: f32, y: f32) -> f32 {
        g[0] as f32 * x + g[1] as f32 * y
    }

    /// Java: `protected static float dot3D(int[], float, float, float)`(L60-62).
    pub fn dot_3d(g: &[i32; 3], x: f32, y: f32, z: f32) -> f32 {
        g[0] as f32 * x + g[1] as f32 * y + g[2] as f32 * z
    }

    /// Java: `protected static float dot4D(int[], float, float, float, float)`
    /// (4D-path spare, signature kept for parity.)
    ///
    /// Upstream reads `g[3]` but `grad3` has 3 components (no upstream callers);
    /// the signature is kept and calling it panics.
    #[allow(dead_code)]
    pub fn dot_4d(_g: &[i32; 3], _x: f32, _y: f32, _z: f32, _w: f32) -> f32 {
        panic!("dot4D needs a 4-component gradient table; no path calls it");
    }
}

impl NoiseFSampler for SimplexF {
    fn base(&self) -> &NoiseF {
        &self.perlin.base
    }

    /// Java: `@Override getNoise3D(float, float, float)`(L69-197).
    fn get_noise_3d(&self, x: f32, y: f32, z: f32) -> f32 {
        let x = x + self.perlin.base.offset_x;
        let y = y + self.perlin.base.offset_y;
        let z = z + self.perlin.base.offset_z;

        // Skew the input space to find the simplex cell.
        let s = (x + y + z) * F3;
        let i = (x + s) as i32;
        let j = (y + s) as i32;
        let k = (z + s) as i32;
        let t = (i + j + k) as f32 * G3;
        // Unskew back to the original space.
        let x0 = x - (i as f32 - t);
        let y0 = y - (j as f32 - t);
        let z0 = z - (k as f32 - t);

        // Locate the enclosing simplex.
        let (i1, j1, k1, i2, j2, k2);

        if x0 >= y0 {
            if y0 >= z0 {
                // X Y Z order
                i1 = 1;
                j1 = 0;
                k1 = 0;
                i2 = 1;
                j2 = 1;
                k2 = 0;
            } else if x0 >= z0 {
                // X Z Y order
                i1 = 1;
                j1 = 0;
                k1 = 0;
                i2 = 1;
                j2 = 0;
                k2 = 1;
            } else {
                // Z X Y order
                i1 = 0;
                j1 = 0;
                k1 = 1;
                i2 = 1;
                j2 = 0;
                k2 = 1;
            }
        } else {
            if y0 < z0 {
                // Z Y X order
                i1 = 0;
                j1 = 0;
                k1 = 1;
                i2 = 0;
                j2 = 1;
                k2 = 1;
            } else if x0 < z0 {
                // Y Z X order
                i1 = 0;
                j1 = 1;
                k1 = 0;
                i2 = 0;
                j2 = 1;
                k2 = 1;
            } else {
                // Y X Z order
                i1 = 0;
                j1 = 1;
                k1 = 0;
                i2 = 1;
                j2 = 1;
                k2 = 0;
            }
        }

        // Four-corner offsets.
        let x1 = x0 - i1 as f32 + G3;
        let y1 = y0 - j1 as f32 + G3;
        let z1 = z0 - k1 as f32 + G3;
        let x2 = x0 - i2 as f32 + 2.0 * G3;
        let y2 = y0 - j2 as f32 + 2.0 * G3;
        let z2 = z0 - k2 as f32 + 2.0 * G3;
        let x3 = x0 - 1.0 + 3.0 * G3;
        let y3 = y0 - 1.0 + 3.0 * G3;
        let z3 = z0 - 1.0 + 3.0 * G3;

        // Hashed corner indices.
        let ii = i & 255;
        let jj = j & 255;
        let kk = k & 255;

        let perm = &self.perlin.base.perm;
        // Indices stay in 0..=510: i32 index to usize.
        let p = |i: i32| perm[i as usize];
        let mut n = 0.0f32;

        // Corner 0 contribution.
        // (perm values are 0..=255 and lattice indices 0..=255,
        // so indices top out at 510 < 512: no overflow.)
        let t0 = 0.6 - x0 * x0 - y0 * y0 - z0 * z0;
        if t0 > 0.0 {
            let gi0 = &GRAD3[(p(ii + p(jj + p(kk))) % 12) as usize];
            n += t0 * t0 * t0 * t0 * (gi0[0] as f32 * x0 + gi0[1] as f32 * y0 + gi0[2] as f32 * z0);
        }

        // Corner 1 contribution.
        let t1 = 0.6 - x1 * x1 - y1 * y1 - z1 * z1;
        if t1 > 0.0 {
            let gi1 = &GRAD3[(p(ii + i1 + p(jj + j1 + p(kk + k1))) % 12) as usize];
            n += t1 * t1 * t1 * t1 * (gi1[0] as f32 * x1 + gi1[1] as f32 * y1 + gi1[2] as f32 * z1);
        }

        // Corner 2 contribution.
        let t2 = 0.6 - x2 * x2 - y2 * y2 - z2 * z2;
        if t2 > 0.0 {
            let gi2 = &GRAD3[(p(ii + i2 + p(jj + j2 + p(kk + k2))) % 12) as usize];
            n += t2 * t2 * t2 * t2 * (gi2[0] as f32 * x2 + gi2[1] as f32 * y2 + gi2[2] as f32 * z2);
        }

        // Corner 3 contribution.
        let t3 = 0.6 - x3 * x3 - y3 * y3 - z3 * z3;
        if t3 > 0.0 {
            let gi3 = &GRAD3[(p(ii + 1 + p(jj + 1 + p(kk + 1))) % 12) as usize];
            n += t3 * t3 * t3 * t3 * (gi3[0] as f32 * x3 + gi3[1] as f32 * y3 + gi3[2] as f32 * z3);
        }

        // Scale to [-1,1].
        32.0 * n
    }

    /// Java: `@Override getNoise2D(float, float)`(L200-264).
    fn get_noise_2d(&self, x: f32, y: f32) -> f32 {
        let x = x + self.perlin.base.offset_x;
        let y = y + self.perlin.base.offset_y;

        // Skew/unskew.
        let s = (x + y) * F2;
        let i = (x + s) as i32;
        let j = (y + s) as i32;
        // `(i + j)` addition wraps on overflow.
        let t = i.wrapping_add(j) as f32 * G2;
        let x0 = x - (i as f32 - t);
        let y0 = y - (j as f32 - t);

        // Locate the enclosing triangle.
        let (i1, j1) = if x0 > y0 {
            // lower triangle, XY order
            (1, 0)
        } else {
            // upper triangle, YX order
            (0, 1)
        };

        // Triangle corner offsets (last corner uses G22).
        let x1 = x0 - i1 as f32 + G2;
        let y1 = y0 - j1 as f32 + G2;
        let x2 = x0 + G22;
        let y2 = y0 + G22;

        // Java L237-238.
        let ii = i & 255;
        let jj = j & 255;

        let perm = &self.perlin.base.perm;
        // Index semantics match the 3D path.
        let p = |i: i32| perm[i as usize];
        let mut n = 0.0f32;

        // Corner 0 contribution (grad3 x/y as the 2D gradient).
        let t0 = 0.5 - x0 * x0 - y0 * y0;
        if t0 > 0.0 {
            let gi0 = &GRAD3[(p(ii + p(jj)) % 12) as usize];
            n += t0 * t0 * t0 * t0 * (gi0[0] as f32 * x0 + gi0[1] as f32 * y0);
        }

        // Corner 1 contribution.
        let t1 = 0.5 - x1 * x1 - y1 * y1;
        if t1 > 0.0 {
            let gi1 = &GRAD3[(p(ii + i1 + p(jj + j1)) % 12) as usize];
            n += t1 * t1 * t1 * t1 * (gi1[0] as f32 * x1 + gi1[1] as f32 * y1);
        }

        // Corner 2 contribution.
        let t2 = 0.5 - x2 * x2 - y2 * y2;
        if t2 > 0.0 {
            let gi2 = &GRAD3[(p(ii + 1 + p(jj + 1)) % 12) as usize];
            n += t2 * t2 * t2 * t2 * (gi2[0] as f32 * x2 + gi2[1] as f32 * y2);
        }

        // Scale to [-1,1].
        70.0 * n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::random::Xoroshiro128;

    #[test]
    fn noise_f_static_helpers() {
        // floor: negatives subtract 1 before truncating,
        // so negative integers shift down by 1 (upstream quirk).
        assert_eq!(NoiseF::floor(0.5), 0);
        assert_eq!(NoiseF::floor(-0.5), -1); // (int)(-1.5f) = -1
        assert_eq!(NoiseF::floor(-1.0), -2); // (int)(-2.0f) = -2
        assert_eq!(NoiseF::floor(3.7), 3);
        // fade endpoints.
        assert_eq!(NoiseF::fade(0.0), 0.0);
        assert_eq!(NoiseF::fade(1.0), 1.0);
        // lerp
        assert_eq!(NoiseF::lerp(0.0, 2.0, 4.0), 2.0);
        assert_eq!(NoiseF::lerp(1.0, 2.0, 4.0), 4.0);
        assert_eq!(NoiseF::lerp(0.5, 2.0, 4.0), 3.0);
        // grad: symmetric branches on hash&15.
        assert_eq!(NoiseF::grad(0, 1.0, 2.0, 3.0), 3.0); // u=x=1, v=y=2 → x+y
        assert_eq!(NoiseF::grad(12, 1.0, 2.0, 3.0), 3.0); // u=y=2, v=x=1 → x+y
        assert_eq!(NoiseF::grad(1, 1.0, 2.0, 3.0), 1.0); // u=x, -v → x-y
    }

    #[test]
    fn perlin_f_deterministic() {
        let mut r1 = Xoroshiro128::new(42);
        let p1 = PerlinF::new(&mut r1, 8.0, 0.5);
        let mut r2 = Xoroshiro128::new(42);
        let p2 = PerlinF::new(&mut r2, 8.0, 0.5);
        assert_eq!(p1.base.perm, p2.base.perm);
        assert_eq!(p1.base.offset_x, p2.base.offset_x);
        assert_eq!(p1.base.offset_z, p2.base.offset_z);

        for i in 0..20 {
            let x = i as f32 * 0.31;
            let v1 = p1.get_value(x, x * 0.7, -x);
            let v2 = p2.get_value(x, x * 0.7, -x);
            assert_eq!(v1, v2);
            assert!(v1.is_finite());
        }

        // 2D delegates to 3D (z=0).
        assert_eq!(p1.get_noise_2d(1.5, 2.5), p1.get_noise_3d(1.5, 2.5, 0.0));
    }

    #[test]
    fn perlin_f_perm_layout() {
        // First-round values span [0, 255] inclusive (not a permutation).
        let mut r = Xoroshiro128::new(7);
        let p = PerlinF::new(&mut r, 8.0, 0.5);
        assert_eq!(p.base.perm.len(), 512);
        assert!(p.base.perm.iter().all(|&v| (0..=255).contains(&v)));
        // High 256 slots duplicate: perm[i+256] == perm[i].
        for i in 0..256usize {
            assert_eq!(p.base.perm[i + 256], p.base.perm[i]);
        }
    }

    #[test]
    fn simplex_f_constants_bit_exact() {
        // Literals must match (float) Math.sqrt(x) bit for bit.
        assert_eq!(SQRT_3, (3.0f64.sqrt()) as f32);
        assert_eq!(SQRT_5, (5.0f64.sqrt()) as f32);
        assert_eq!(F2, 0.5f32 * (SQRT_3 - 1.0));
        assert_eq!(G2, (3.0f32 - SQRT_3) / 6.0);
        assert_eq!(G22, G2 * 2.0 - 1.0);
        assert_eq!(F3, 1.0f32 / 3.0);
        assert_eq!(G3, 1.0f32 / 6.0);
    }

    #[test]
    fn simplex_f_deterministic_and_bounded() {
        let mut r1 = Xoroshiro128::new(1234);
        let s1 = SimplexF::new_with_expansion(&mut r1, 2.0, 0.5, 0.1);
        let mut r2 = Xoroshiro128::new(1234);
        let s2 = SimplexF::new_with_expansion(&mut r2, 2.0, 0.5, 0.1);
        assert_eq!(s1.perlin.base.perm, s2.perlin.base.perm);
        assert_eq!(s1.offset_w, s2.offset_w);

        // offsetW consumes randomness after the three offsets and perm init.
        let mut probe = Xoroshiro128::new(1234);
        let _ = probe.next_float();
        let _ = probe.next_float();
        let _ = probe.next_float();
        for _ in 0..256 {
            let _ = probe.next_bounded_int(255);
        }
        for i in 0..256i32 {
            let _ = probe.next_bounded_int(255 - i);
        }
        // The next draw is the SimplexF constructor offsetW (* 256).
        assert_eq!(s1.offset_w, probe.next_float() * 256.0);

        for i in 0..20 {
            let x = i as f32 * 0.73;
            let z = i as f32 * 1.31;
            assert_eq!(s1.get_noise_2d(x, z), s2.get_noise_2d(x, z));
            assert_eq!(
                s1.get_noise_3d(x, x * 0.5, z),
                s2.get_noise_3d(x, x * 0.5, z)
            );
            assert_eq!(s1.get_value(x, x, z), s1.get_noise_3d(x, x, z));
        }

        // Single-octave 2D simplex spans about [-1, 1].
        for i in 0..200 {
            let x = i as f32 * 0.17;
            let z = i as f32 * 0.29;
            let v = s1.get_noise_2d(x, z);
            assert!(v.is_finite());
            assert!((v.abs() - 1.0) < 0.35, "v={v} outside the theoretical range");
        }
    }

    #[test]
    fn simplex_f_octave_loop_semantics() {
        // persistence=0: max=1, normalized equals unnormalized.
        let mut r = Xoroshiro128::new(99);
        let s = SimplexF::new(&mut r, 3.0, 0.0);
        assert_eq!(s.noise_2d(1.7, 2.3), s.noise_2d_normalized(1.7, 2.3, true));
        assert_eq!(
            s.noise_3d(1.7, 0.4, 2.3),
            s.noise_3d_normalized(1.7, 0.4, 2.3, true)
        );

        // octaves=2, persistence=0.5:max = 1 + 0.5 → normalized = raw / 1.5
        let mut r = Xoroshiro128::new(99);
        let s = SimplexF::new(&mut r, 2.0, 0.5);
        let raw = s.noise_2d(1.1, 2.2);
        let norm = s.noise_2d_normalized(1.1, 2.2, true);
        assert_eq!(norm, raw / 1.5);
        let raw3 = s.noise_3d(1.1, 0.5, 2.2);
        let norm3 = s.noise_3d_normalized(1.1, 0.5, 2.2, true);
        assert_eq!(norm3, raw3 / 1.5);

        // float octaves (e.g. 30f) loop 30 times.
        let mut r = Xoroshiro128::new(99);
        let s = SimplexF::new(&mut r, 30.0, 1.0 / 99.0);
        let v = s.noise_2d(3.3, 4.4);
        assert!(v.is_finite());
    }

    #[test]
    fn perlin_f_set_offset_changes_output() {
        let mut r = Xoroshiro128::new(5);
        let mut p = PerlinF::new(&mut r, 4.0, 0.5);
        let before = p.get_noise_3d(1.0, 2.0, 3.0);
        p.base.set_offset(100.0, 200.0, 300.0);
        let after = p.get_noise_3d(1.0, 2.0, 3.0);
        assert_ne!(before, after);
    }
}
