//! Math utilities for world generation:
//! - Integer math helpers (subset covering the LCG/discrete-log dependencies)
//! - `MathHelper` (gradient/interpolation helpers for noise sampling)
//! - `LCG` (linear congruential generator with seed skipping)
//! - `{Pair, Quad, Triplet}` (small tuples)
//!
//! Reference sources live under `.fetch/`.
//! Discrete-log and `LCG.distance` have no callers in the worldgen path and
//! are not covered (`LCG::distance` is an explicit `unimplemented!`).

// ---------------------------------------------------------------------------
// Integer math helpers
// ---------------------------------------------------------------------------

/// Returns true if the value is a power of two.
#[inline]
pub fn is_power_of_2(value: i64) -> bool {
    (value & value.wrapping_neg()) == value
}

/// Returns 2^`bits`.
#[inline]
pub fn get_pow2(bits: i32) -> i64 {
    1i64 << bits
}

/// Returns a mask with the low `bits` bits set.
#[inline]
pub fn get_mask(bits: i32) -> i64 {
    if bits >= 64 {
        !0
    } else {
        get_pow2(bits) - 1
    }
}

/// Keeps the low `bits` bits of the value.
#[inline]
pub fn mask(value: i64, bits: i32) -> i64 {
    value & get_mask(bits)
}

/// Keeps the low `bits` bits, sign-extended from the highest kept bit.
///
/// Note: a shift of `value << (64 - bits) >> (64 - bits)` masks its shift
/// amount to the low 6 bits (so `bits == 0` shifts by nothing); Rust
/// replicates that with an explicit `& 63`.
#[inline]
pub fn mask_signed(value: i64, bits: i32) -> i64 {
    let sh = (64 - bits) & 63;
    (value << sh) >> sh
}

/// Modular inverse under a power-of-two modulus (Newton iteration).
#[inline]
pub fn mod_inverse(value: i64, bits: i32) -> i64 {
    let mut x = ((((value << 1) ^ value) & 4) << 1) ^ value;
    x = x.wrapping_mul(2i64.wrapping_sub(value.wrapping_mul(x)));
    x = x.wrapping_mul(2i64.wrapping_sub(value.wrapping_mul(x)));
    x = x.wrapping_mul(2i64.wrapping_sub(value.wrapping_mul(x)));
    x = x.wrapping_mul(2i64.wrapping_sub(value.wrapping_mul(x)));
    mask(x, bits)
}

/// Clamps an `f64` to `[min, max]`.
#[inline]
pub fn clamp_f64(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else {
        value.min(max)
    }
}

/// Clamps an `i32` to `[min, max]`.
#[inline]
pub fn clamp_i32(value: i32, min: i32, max: i32) -> i32 {
    if value < min {
        min
    } else {
        value.min(max)
    }
}

/// Linearly remaps `input` from `[in_min, in_max]` to `[out_min, out_max]`.
#[inline]
pub fn remap_f32(input: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    out_min + ((input - in_min) / (in_max - in_min) * (out_max - out_min))
}

/// Remaps a normalized `[-1, 1]` input to `[out_min, out_max]`.
#[inline]
pub fn remap_from_normalized(input: f32, out_min: f32, out_max: f32) -> f32 {
    remap_f32(input, -1.0, 1.0, out_min, out_max)
}

/// Standard string hash (`h = 31*h + c`, `i32` wrapping).
///
/// Shared with the private hasher in `noise/simplex.rs`; exposed here for
/// the `"clay_bands"` hash used by `NormalSurfaceOverwriteStage`.
pub fn java_string_hashcode(s: &str) -> i32 {
    let mut h: i32 = 0;
    for c in s.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as i32);
    }
    h
}

/// Rounds an `f32` half-up (toward +∞), unlike Rust's `f32::round()`
/// (half away from zero).
///
/// Noise-offset inputs stay within ≈[-4,4], so there is no overflow/NaN risk.
#[inline]
pub fn java_math_round_f32(x: f32) -> i32 {
    (x + 0.5).floor() as i32
}

// ---------------------------------------------------------------------------
// randomRange / randomRangeTriangle (used by ore features)
// ---------------------------------------------------------------------------

/// Draws a uniform integer in `[start, end]`.
///
/// Uses `start + (random.nextInt() % (end + 1 - start))`; `%` on negative
/// dividends keeps the dividend's sign in both languages, so the behavior
/// matches directly.
#[inline]
pub fn random_range<R: crate::worldgen::random::RandomSourceProvider + ?Sized>(
    random: &mut R,
    start: i32,
    end: i32,
) -> i32 {
    start + (random.next_int() % (end + 1 - start))
}

/// Draws a triangle-distributed integer in `[start, end]` (sums two
/// half-range uniform draws so results cluster in the middle).
#[inline]
pub fn random_range_triangle<R: crate::worldgen::random::RandomSourceProvider>(
    random: &mut R,
    start: i32,
    end: i32,
) -> i32 {
    let height_diff = (end - start).abs();
    let height_diff_half = height_diff / 2;
    let height_diff_half2 = height_diff - height_diff_half;
    start.min(end)
        + random_range(random, 0, height_diff_half2)
        + random_range(random, 0, height_diff_half)
}

// ---------------------------------------------------------------------------
// MathHelper (gradient/interpolation helpers for noise sampling)
// ---------------------------------------------------------------------------

/// Perlin gradient table.
pub const GRADIENTS: [[i32; 3]; 16] = [
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
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

/// Gradient dot product selected by the low 4 bits of `hash`.
pub fn grad(hash: i32, x: f64, y: f64, z: f64) -> f64 {
    match hash & 0xF {
        0x0 => x + y,
        0x1 => -x + y,
        0x2 => x - y,
        0x3 => -x - y,
        0x4 => x + z,
        0x5 => -x + z,
        0x6 => x - z,
        0x7 => -x - z,
        0x8 => y + z,
        0x9 | 0xD => -y + z,
        0xA => y - z,
        0xB | 0xF => -y - z,
        0xC => y + x,
        0xE => y - x,
        _ => 0.0, // never happens
    }
}

/// Floors an `f64` to `i64`.
///
/// Truncation-toward-zero with saturation (NaN → 0) matches Rust's `as i64`.
pub fn lfloor(d: f64) -> i64 {
    let l = d as i64;
    if d < l as f64 {
        l - 1
    } else {
        l
    }
}

/// Dot product of a gradient vector with `(x, y, z)`.
pub fn dot(g: &[i32; 3], x: f64, y: f64, z: f64) -> f64 {
    g[0] as f64 * x + g[1] as f64 * y + g[2] as f64 * z
}

/// Trilinear interpolation over a unit cube.
#[allow(clippy::too_many_arguments)]
pub fn lerp3(
    delta_x: f64,
    delta_y: f64,
    delta_z: f64,
    val000: f64,
    val100: f64,
    val010: f64,
    val110: f64,
    val001: f64,
    val101: f64,
    val011: f64,
    val111: f64,
) -> f64 {
    lerp(
        delta_z,
        lerp2(delta_x, delta_y, val000, val100, val010, val110),
        lerp2(delta_x, delta_y, val001, val101, val011, val111),
    )
}

/// Bilinear interpolation over a unit square.
pub fn lerp2(delta_x: f64, delta_y: f64, val00: f64, val10: f64, val01: f64, val11: f64) -> f64 {
    lerp(
        delta_y,
        lerp(delta_x, val00, val10),
        lerp(delta_x, val01, val11),
    )
}

/// Linear interpolation from `start` to `end`.
#[inline]
pub fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

/// Smootherstep 10x³-15x⁴+6x⁵.
#[inline]
pub fn smooth_step(d: f64) -> f64 {
    d * d * d * (d * (d * 6.0 - 15.0) + 10.0)
}

/// Floors an `f64` to `i32`.
pub fn floor(d: f64) -> i32 {
    let i = d as i32;
    if d < i as f64 {
        i - 1
    } else {
        i
    }
}

/// Floors an `f64` to `i64` (truncation with saturation, NaN → 0,
/// matching Rust's `as i64`).
pub fn floor_double_long(d: f64) -> i64 {
    let l = d as i64;
    if d >= l as f64 {
        l
    } else {
        l - 1
    }
}

/// Wraps `d` into a bounded range to keep noise inputs precise.
#[inline]
pub fn maintain_precision(d: f64) -> f64 {
    d - lfloor(d / 3.3554432E7 + 0.5) as f64 * 3.3554432E7
}

// ---------------------------------------------------------------------------
// LCG (linear congruential generator with seed skipping)
// ---------------------------------------------------------------------------

/// Linear congruential generator with seed skipping.
///
/// Fields are immutable; equality/debugging come from derived
/// `PartialEq`/`Debug`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lcg {
    pub multiplier: i64,
    pub addend: i64,
    pub modulus: i64,
    is_power_of_2: bool,
    trailing_zeros: i32,
}

impl Lcg {
    /// Builds an LCG with a 2^64 modulus (represented as 0).
    pub const fn new(multiplier: i64, addend: i64) -> Self {
        Self::with_modulus_raw(multiplier, addend, 0)
    }

    /// Builds an LCG with an explicit modulus.
    pub const fn with_modulus(multiplier: i64, addend: i64, modulus: i64) -> Self {
        Self::with_modulus_raw(multiplier, addend, modulus)
    }

    /// Shared constructor logic; power-of-two detection and trailing-zero
    /// counting are hand-written for `const fn`.
    const fn with_modulus_raw(multiplier: i64, addend: i64, modulus: i64) -> Self {
        let is_power_of_2 = (modulus & modulus.wrapping_neg()) == modulus;
        let trailing_zeros = if is_power_of_2 {
            // Trailing-zero count: modulus 0 means 64; otherwise count the
            // low zero bits
            let n = modulus as u64;
            if n == 0 {
                64
            } else {
                let mut count = 0;
                let mut v = n;
                while v & 1 == 0 {
                    v >>= 1;
                    count += 1;
                }
                count
            }
        } else {
            -1
        };
        Self {
            multiplier,
            addend,
            modulus,
            is_power_of_2,
            trailing_zeros,
        }
    }

    // Well-known LCG parameter sets
    pub const CC65_M23: Lcg = Lcg::with_modulus(65793, 4282663, 1 << 23);
    pub const VISUAL_BASIC: Lcg = Lcg::with_modulus(1140671485, 12820163, 1 << 24);
    pub const RTL_UNIFORM: Lcg = Lcg::with_modulus(2147483629, 2147483587, (1 << 31) - 1);
    pub const MINSTD_RAND0_C: Lcg = Lcg::with_modulus(16807, 0, (1 << 31) - 1);
    pub const MINSTD_RAND_C: Lcg = Lcg::with_modulus(48271, 0, (1 << 31) - 1);
    pub const CC65_M31: Lcg = Lcg::with_modulus(16843009, 826366247, 1 << 23);
    pub const RANDU: Lcg = Lcg::with_modulus(65539, 0, 1 << 31);
    pub const GLIB_C: Lcg = Lcg::with_modulus(1103515245, 12345, 1 << 31);
    pub const BORLAND_C: Lcg = Lcg::with_modulus(22695477, 1, 1 << 32);
    pub const PASCAL: Lcg = Lcg::with_modulus(134775813, 1, 1 << 32);
    pub const OPEN_VMS: Lcg = Lcg::with_modulus(69069, 1, 1 << 32);
    pub const NUMERICAL_RECIPES: Lcg = Lcg::with_modulus(1664525, 1013904223, 1 << 32);
    pub const MS_VISUAL_C: Lcg = Lcg::with_modulus(214013, 2531011, 1 << 32);
    pub const JAVA: Lcg = Lcg::with_modulus(25214903917, 11, 1 << 48);
    pub const JAVA_UNIQUIFIER_OLD: Lcg = Lcg::new(181783497276652981, 0);
    pub const JAVA_UNIQUIFIER_NEW: Lcg = Lcg::new(1181783497276652981, 0);
    pub const MMIX: Lcg = Lcg::new(6364136223846793005, 1442695040888963407);
    pub const NEWLIB_C: Lcg = Lcg::new(6364136223846793005, 1);
    pub const XKCD: Lcg = Lcg::new(0, 4);

    /// Returns whether the modulus is a power of two (`const`, used by
    /// `const` callers).
    pub const fn is_mod_power_of_2(&self) -> bool {
        self.is_power_of_2
    }

    /// Returns the number of trailing zeros of the modulus.
    pub const fn get_mod_trailing_zeroes(&self) -> i32 {
        self.trailing_zeros
    }

    /// Returns whether the generator is multiplicative (zero addend).
    pub fn is_multiplicative(&self) -> bool {
        self.addend == 0
    }

    /// Advances the seed by one step.
    pub fn next_seed(&self, seed: i64) -> i64 {
        self.mod_(seed.wrapping_mul(self.multiplier).wrapping_add(self.addend))
    }

    /// Reduces `n` modulo the modulus (`const` so `NoiseSampler::SKIP_262`
    /// can be evaluated at compile time).
    pub const fn mod_(&self, n: i64) -> i64 {
        if self.is_mod_power_of_2() {
            n & self.modulus.wrapping_sub(1)
        } else if n <= 1i64 << 32 {
            // Java: Long.remainderUnsigned(n, modulus)
            ((n as u64) % (self.modulus as u64)) as i64
        } else {
            // `unimplemented!` (a formatting macro) is unavailable in
            // `const fn`; use a literal `panic!` instead.
            panic!("LCG.mod: n > 2^32 with a non-power-of-two modulus (Java throws UnsupportedOperationException)")
        }
    }

    /// Returns the LCG that jumps `steps` steps at once (binary
    /// exponentiation); `const` for compile-time `SKIP_262` evaluation.
    pub const fn combine_steps(&self, steps: i64) -> Lcg {
        let mut multiplier: i64 = 1;
        let mut addend: i64 = 0;

        let mut intermediate_multiplier = self.multiplier;
        let mut intermediate_addend = self.addend;

        let mut k = steps as u64; // Unsigned right shift
        while k != 0 {
            if k & 1 != 0 {
                multiplier = multiplier.wrapping_mul(intermediate_multiplier);
                addend = intermediate_multiplier
                    .wrapping_mul(addend)
                    .wrapping_add(intermediate_addend);
            }

            intermediate_addend = intermediate_multiplier
                .wrapping_add(1)
                .wrapping_mul(intermediate_addend);
            intermediate_multiplier = intermediate_multiplier.wrapping_mul(intermediate_multiplier);
            k >>= 1;
        }

        Lcg::with_modulus(self.mod_(multiplier), self.mod_(addend), self.modulus)
    }

    /// Composes two LCGs with the same modulus.
    pub fn combine(&self, lcg: &Lcg) -> Lcg {
        if self.modulus != lcg.modulus {
            unimplemented!("combine: mismatched moduli (Java throws UnsupportedOperationException)");
        }
        Lcg::with_modulus(
            self.multiplier.wrapping_mul(lcg.multiplier),
            lcg.multiplier
                .wrapping_mul(self.addend)
                .wrapping_add(lcg.addend),
            self.modulus,
        )
    }

    /// Returns the inverse (single step backwards).
    pub fn invert(&self) -> Lcg {
        self.combine_steps(-1)
    }

    /// Seed distance between two states.
    ///
    /// Needs discrete-log arithmetic and has no callers in the worldgen
    /// path, so it is not covered.
    pub fn distance(&self, _seed1: i64, _seed2: i64) -> i64 {
        unimplemented!("LCG.distance: discrete log not ported (no worldgen callers)")
    }
}

// ---------------------------------------------------------------------------
// Pair / Quad / Triplet (small tuples)
// ---------------------------------------------------------------------------

/// Two-element tuple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pair<A, B> {
    a: A,
    b: B,
}

impl<A, B> Pair<A, B> {
    pub const fn new(a: A, b: B) -> Self {
        Self { a, b }
    }

    /// Returns a reference to the first element.
    pub fn get_first(&self) -> &A {
        &self.a
    }

    /// Returns a reference to the second element.
    pub fn get_second(&self) -> &B {
        &self.b
    }

    /// Consuming convenience accessor (no reference-source counterpart).
    pub fn into_first(self) -> A {
        self.a
    }

    /// Consuming convenience accessor (no reference-source counterpart).
    pub fn into_second(self) -> B {
        self.b
    }
}

/// Three-element tuple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Triplet<A, B, C> {
    a: A,
    b: B,
    c: C,
}

impl<A, B, C> Triplet<A, B, C> {
    pub const fn new(a: A, b: B, c: C) -> Self {
        Self { a, b, c }
    }

    /// Returns a reference to the first element.
    pub fn get_first(&self) -> &A {
        &self.a
    }

    /// Returns a reference to the second element.
    pub fn get_second(&self) -> &B {
        &self.b
    }

    /// Returns a reference to the third element.
    pub fn get_third(&self) -> &C {
        &self.c
    }
}

/// Four-element tuple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Quad<A, B, C, D> {
    a: A,
    b: B,
    c: C,
    d: D,
}

impl<A, B, C, D> Quad<A, B, C, D> {
    pub const fn new(a: A, b: B, c: C, d: D) -> Self {
        Self { a, b, c, d }
    }

    /// Returns a reference to the first element.
    pub fn get_first(&self) -> &A {
        &self.a
    }

    /// Returns a reference to the second element.
    pub fn get_second(&self) -> &B {
        &self.b
    }

    /// Returns a reference to the third element.
    pub fn get_third(&self) -> &C {
        &self.c
    }

    /// Returns a reference to the fourth element.
    pub fn get_fourth(&self) -> &D {
        &self.d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_helper_basics() {
        // lfloor / floor: round negative values down
        assert_eq!(lfloor(1.5), 1);
        assert_eq!(lfloor(-1.5), -2);
        assert_eq!(floor(-1.5), -2);
        // lerp
        assert_eq!(lerp(0.0, 10.0, 20.0), 10.0);
        assert_eq!(lerp(1.0, 10.0, 20.0), 20.0);
        assert_eq!(lerp(0.5, 10.0, 20.0), 15.0);
        // smoothStep endpoints
        assert_eq!(smooth_step(0.0), 0.0);
        assert_eq!(smooth_step(1.0), 1.0);
        // maintainPrecision period
        let v = maintain_precision(3.3554432E7 + 1.0);
        assert!((v - 1.0).abs() < 1e-6, "maintain_precision wraparound period: {v}");
        // Spot-check the grad table (against GRADIENTS)
        assert_eq!(grad(0, 2.0, 3.0, 5.0), 5.0); // x+y
        assert_eq!(grad(0xC, 2.0, 3.0, 5.0), 5.0); // y+x
        assert_eq!(grad(0xB, 2.0, 3.0, 5.0), -8.0); // -y-z
        assert_eq!(dot(&GRADIENTS[0], 1.0, 2.0, 3.0), 3.0);
    }

    #[test]
    fn lcg_java_next_seed() {
        // Seed advance: seed' = (seed * 0x5DEECE66D + 0xB) mod 2^48
        let java = Lcg::JAVA;
        assert_eq!(java.next_seed(0), 11);
        // combine(0) = identity
        let id = java.combine_steps(0);
        assert_eq!(id.multiplier, 1);
        assert_eq!(id.addend, 0);
        // combine(1) == the original LCG
        let one = java.combine_steps(1);
        assert_eq!(one.multiplier, java.multiplier);
        assert_eq!(one.addend, java.addend);
        // combine(2) == combine(LCG, LCG) (note: combine(LCG) skips the
        // modulus while combine(long) applies it)
        let two = java.combine_steps(2);
        let manual = java.combine(&java);
        assert_eq!(two.multiplier, java.mod_(manual.multiplier));
        assert_eq!(two.addend, java.mod_(manual.addend));
        // invert: nextSeed, then invert, then nextSeed returns to the origin
        let inv = java.invert();
        let round_trip = inv.next_seed(java.next_seed(12345));
        assert_eq!(round_trip, 12345);
    }

    #[test]
    fn lcg_mod_semantics() {
        // Power-of-two modulus: bit-and
        assert_eq!(
            Lcg::JAVA.mod_(0x123456789ABCD),
            0x123456789ABCD & ((1i64 << 48) - 1)
        );
        // modulus 0 (2^64): n & -1 = n
        let mmix = Lcg::MMIX;
        assert_eq!(mmix.mod_(-42), -42);
        assert!(mmix.is_mod_power_of_2());
        assert_eq!(mmix.get_mod_trailing_zeroes(), 64);
        // Non-power-of-two modulus takes the unsigned-remainder path
        // (n <= 2^32 branch)
        let rtl = Lcg::RTL_UNIFORM; // modulus = 2^31 - 1
        assert_eq!(rtl.mod_(100), 100 % 2147483647);
        assert_eq!(rtl.mod_(-1), ((-1i64) as u64 % 2147483647u64) as i64);
    }

    #[test]
    fn pair_triplet_quad_accessors() {
        let p = Pair::new(1i32, 2.5f64);
        assert_eq!(*p.get_first(), 1);
        assert_eq!(*p.get_second(), 2.5);
        let t = Triplet::new(1, 2, 3);
        assert_eq!((*t.get_first(), *t.get_second(), *t.get_third()), (1, 2, 3));
        let q = Quad::new(1, 2, 3, 4);
        assert_eq!((*q.get_first(), *q.get_fourth()), (1, 4));
    }

    #[test]
    fn mask_signed_java_shift_semantics() {
        // Shift amounts wrap to the low 6 bits: bits=0/64 → no shift
        assert_eq!(mask_signed(-1, 0), -1);
        assert_eq!(mask_signed(-1, 64), -1);
        // Sign-extension semantics: keep the low `bits` bits and sign-extend
        // from the highest kept bit
        assert_eq!(mask_signed(0x80, 8), -128); // High bit of the low byte is 1
        assert_eq!(mask_signed(0x7F, 8), 127);
        assert_eq!(mask_signed(i64::MAX, 8), -1); // Low byte 0xFF → sign-extends to -1
        assert_eq!(mask_signed(-256, 8), 0); // 0xFFFFFF00 has zero low 8 bits
    }

    #[test]
    fn remap_and_remap_from_normalized() {
        // remap: linear map [inMin, inMax] → [outMin, outMax]
        assert_eq!(remap_f32(0.0, -1.0, 1.0, 0.0, 10.0), 5.0);
        assert_eq!(remap_f32(-1.0, -1.0, 1.0, 0.0, 10.0), 0.0);
        assert_eq!(remap_f32(1.0, -1.0, 1.0, 0.0, 10.0), 10.0);
        // remapFromNormalized: [-1,1] → [outMin, outMax]
        assert_eq!(remap_from_normalized(-1.0, 1.0, 4.0), 1.0);
        assert_eq!(remap_from_normalized(0.0, 1.0, 4.0), 2.5);
        assert_eq!(remap_from_normalized(1.0, 1.0, 4.0), 4.0);
    }

    #[test]
    fn java_string_hashcode_known_values() {
        // Check against known values
        assert_eq!(java_string_hashcode(""), 0);
        assert_eq!(java_string_hashcode("hello"), 99162322);
        // "clay_bands".hashCode() — verified step by step by hand
        assert_eq!(java_string_hashcode("clay_bands"), 169392512);
    }

    #[test]
    fn java_math_round_f32_half_up() {
        // Java Math.round(float) = (int) Math.floor(a + 0.5f)
        assert_eq!(java_math_round_f32(0.0), 0);
        assert_eq!(java_math_round_f32(0.4), 0);
        assert_eq!(java_math_round_f32(0.5), 1); // Half rounds up
        assert_eq!(java_math_round_f32(-0.5), 0); // Half rounds up (not away from zero)
        assert_eq!(java_math_round_f32(-1.5), -1);
        assert_eq!(java_math_round_f32(-1.6), -2);
        assert_eq!(java_math_round_f32(2.5), 3);
    }
}
