//! 48 位 LCG 随机源：乘数 25214903917、加数 11、48 位截断。
//!
//! 用于末地岛屿与 legacy 随机源路径；序列必须逐位保持。
//!
//! 48-bit LCG random source with multiplier 25214903917, addend 11, and 48-bit truncation.
//!
//! Used for end islands and the legacy random-source path; the sequence must remain bit-exact.

/// UTF-16 码元上的字符串哈希：`h = h * 31 + c`，i32 溢出回绕
/// （由名称派生种子时使用）。
///
/// String hash over UTF-16 code units: `h = h * 31 + c` with wrapping i32
/// arithmetic (used to derive seeds from names).
pub fn java_string_hash_code(name: &str) -> i32 {
    let mut h: i32 = 0;
    for c in name.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(i32::from(c));
    }
    h
}

/// 48 位 LCG 源；序列合同见模块级文档。
///
/// A 48-bit LCG source; the sequence contract is in the module docs.
#[derive(Clone, Debug)]
pub struct LegacyRandom {
    seed: i64,
}

const MULTIPLIER: i64 = 25214_90391_7; // 0x5DEECE66D
const INCREMENT: i64 = 11;
const MODULUS_MASK: i64 = (1 << 48) - 1;

impl LegacyRandom {
    /// 从种子构建源（`set_seed` 语义）。
    ///
    /// Builds the source from a seed (`set_seed` semantics).
    pub fn new(seed: i64) -> Self {
        let mut r = Self { seed: 0 };
        r.set_seed(seed);
        r
    }

    /// 重置内部种子，使后续序列从该种子重新开始。
    ///
    /// Resets the internal seed so the stream restarts from it.
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed ^ MULTIPLIER) & MODULUS_MASK;
    }

    /// 推进一步 LCG 并返回高位截取的 `bits` 位。
    ///
    /// Advances the LCG one step and returns the top `bits` bits.
    pub fn next(&mut self, bits: i32) -> i32 {
        self.seed = self.seed.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT) & MODULUS_MASK;
        (self.seed as u64 >> (48 - bits)) as i32
    }

    /// 无界整数：`next(32)`。
    ///
    /// Unbounded integer: `next(32)`.
    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    /// 返回 `[0, bound)` 内的整数；`bound <= 0` 时 panic。
    ///
    /// Returns an integer in `[0, bound)`; panics when `bound <= 0`.
    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "Bound must be positive");
        if bound & (bound - 1) == 0 {
            // Power of two: multiply-high without rejection.
            ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32
        } else {
            // Rejection loop over next(31) keeps the distribution uniform.
            loop {
                let sample = self.next(31);
                let modulo = sample % bound;
                if sample - modulo + (bound - 1) >= 0 {
                    return modulo;
                }
            }
        }
    }

    /// 两个 32 位抽取组合成 64 位整数。
    ///
    /// Two 32-bit draws combined into a 64-bit integer.
    pub fn next_long(&mut self) -> i64 {
        let high = self.next(32) as i64;
        let low = self.next(32) as i64;
        (high << 32) + low
    }

    /// 26/27 位抽取组合成 `[0, 1)` 的 f64。
    ///
    /// 26/27-bit draws combined into an f64 in `[0, 1)`.
    pub fn next_double(&mut self) -> f64 {
        let high = (self.next(26) as i64) << 27;
        let low = self.next(27) as i64;
        (high + low) as f64 / (1u64 << 53) as f64
    }

    /// 单个 24 位抽取映射到 `[0, 1)` 的 f32。
    ///
    /// One 24-bit draw mapped to an f32 in `[0, 1)`.
    pub fn next_float(&mut self) -> f32 {
        (self.next(24) as f32) / (1u32 << 24) as f32
    }

    /// 消耗 `rounds` 次 `next_int()`，推进流而不产出值。
    ///
    /// Consumes `rounds` × `next_int()`, advancing the stream without output.
    pub fn consume_count(&mut self, rounds: i32) {
        for _ in 0..rounds {
            self.next_int();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consume_count_equals_n_next_int_calls() {
        let mut a = LegacyRandom::new(2026);
        let mut b = LegacyRandom::new(2026);
        a.consume_count(17292);
        for _ in 0..17292 {
            b.next_int();
        }
        for _ in 0..16 {
            assert_eq!(a.next_int(), b.next_int());
        }
    }

    #[test]
    fn set_seed_resets_the_stream() {
        let mut r = LegacyRandom::new(1);
        let first: Vec<i32> = (0..8).map(|_| r.next_int()).collect();
        r.set_seed(1);
        let second: Vec<i32> = (0..8).map(|_| r.next_int()).collect();
        assert_eq!(first, second);
    }

    #[test]
    fn bounded_int_stays_in_range_and_reset() {
        for bound in [1i32, 7, 255, 256, 1023, 65536] {
            let mut r = LegacyRandom::new(99);
            for _ in 0..64 {
                let v = r.next_int_bounded(bound);
                assert!((0..bound).contains(&v), "bound={bound} value={v}");
            }
        }
    }

    #[test]
    fn java_string_hash_code_matches_known_vectors() {
        // Fixed hash anchors for noise names such as "minecraft:continentalness".
        assert_eq!(
            java_string_hash_code("minecraft:continentalness"),
            -48307223
        );
        assert_eq!(java_string_hash_code("minecraft:temperature"), -549971161);
        // Sanity: empty and ascii anchors.
        assert_eq!(java_string_hash_code(""), 0);
        assert_eq!(java_string_hash_code("a"), 97);
    }
}
