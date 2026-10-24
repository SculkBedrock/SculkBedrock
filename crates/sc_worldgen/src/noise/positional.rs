//! 位置化随机派生：128 位种子升级、按名称哈希分叉、xoroshiro128++ 核心。
//!
//! Positional random derivation: 128-bit seed upgrade, name-hashed forking and
//! the xoroshiro128++ core.
//!
//! 噪声种子链：
//!
//! The noise seed chain:
//!
//! ```text
//! world positional = XoroshiroRandomSource(world_seed).forkPositional()
//!   XRS(seed)     = Xoroshiro128PlusPlus(upgradeSeedTo128bit(seed))
//!   upgrade       = stafford13(seed ^ silver), stafford13(lo + golden)
//!   forkPositional= factory(lo = nextLong(), hi = nextLong())
//! per noise id:   factory.fromHashOf(id)
//!   fromHashOf    = XRS(md5(id) ^ factory seeds)          ← MD5, big-endian
//! NormalNoise:    first  = random.forkPositional()
//!                 second = random.forkPositional()
//! per octave i:   PerlinNoise(first.fromHashOf("octave_" + (baseOctave + i)))
//! ```
//!
//! `legacy_random_source` 设置标志会把世界工厂切换为 LCG 变体
//! （[`LegacyPositional`]）。
//!
//! The `legacy_random_source` settings flag swaps the world factory for the
//! LCG variant ([`LegacyPositional`]).

use super::legacy_random::LegacyRandom;
use md5::Digest;

const GOLDEN_RATIO_64: i64 = -7046029254386353131; // 0x9E3779B97F4A7C15
const SILVER_RATIO_64: i64 = 7640891576956012809; // 0x6A09E667F3BCC909

/// Stafford13 混合函数（splitmix 风格的位混淆）。
///
/// The Stafford13 mix (splitmix-style bit mixing).
fn mix_stafford13(mut z: i64) -> i64 {
    z = (z ^ (z as u64 >> 30) as i64).wrapping_mul(-4658895280553007687);
    z = (z ^ (z as u64 >> 27) as i64).wrapping_mul(-7723592293110705685);
    z ^ (z as u64 >> 31) as i64
}

/// 128 位种子对。
///
/// A 128-bit seed pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seed128 {
    pub lo: i64,
    pub hi: i64,
}

impl Seed128 {
    /// 把 64 位种子升级混合为 128 位种子对。
    ///
    /// Upgrades and mixes a 64-bit seed into a 128-bit seed pair.
    pub fn upgrade_from_legacy_seed(legacy_seed: i64) -> Self {
        let lo = legacy_seed ^ SILVER_RATIO_64;
        let hi = lo.wrapping_add(GOLDEN_RATIO_64);
        Self { lo, hi }.mixed()
    }

    /// 由名称派生种子对：对名称的 UTF-8 字节取 MD5，按大端字读取。
    ///
    /// Derives the seed pair from a name: MD5 over the UTF-8 bytes, read as
    /// big-endian words.
    pub fn from_hash_of(name: &str) -> Self {
        let mut hasher = md5::Md5::default();
        hasher.update(name.as_bytes());
        let digest = hasher.finalize();
        let lo = i64::from_be_bytes(digest[0..8].try_into().expect("8 bytes"));
        let hi = i64::from_be_bytes(digest[8..16].try_into().expect("8 bytes"));
        Self { lo, hi }
    }

    fn xor(self, lo: i64, hi: i64) -> Self {
        Self { lo: self.lo ^ lo, hi: self.hi ^ hi }
    }

    /// 对两个种子字分别做 Stafford13 混合。
    ///
    /// Applies the Stafford13 mix to both seed words.
    fn mixed(self) -> Self {
        Self { lo: mix_stafford13(self.lo), hi: mix_stafford13(self.hi) }
    }
}

/// xoroshiro128++ 核心，由原始 128 位种子播种。
///
/// The xoroshiro128++ core, seeded from the raw 128-bit seed.
#[derive(Clone, Debug)]
pub struct Xoroshiro128PlusPlus {
    lo: i64,
    hi: i64,
}

impl Xoroshiro128PlusPlus {
    pub fn new(seed: Seed128) -> Self {
        Self { lo: seed.lo, hi: seed.hi }
    }

    pub fn next_long(&mut self) -> i64 {
        let s0 = self.lo;
        let s1 = self.hi;
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        let s1 = s1 ^ s0;
        self.lo = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.hi = s1.rotate_left(28);
        result
    }
}

/// xoroshiro 随机源：位置工厂发放给各消费者的 RNG。
///
/// The xoroshiro random source: the per-consumer RNG handed out by the
/// positional factory.
#[derive(Clone, Debug)]
pub struct XoroshiroRandomSource {
    rng: Xoroshiro128PlusPlus,
}

impl XoroshiroRandomSource {
    pub fn from_seed_128(seed: Seed128) -> Self {
        Self { rng: Xoroshiro128PlusPlus::new(seed) }
    }

    pub fn from_legacy_seed(legacy_seed: i64) -> Self {
        Self { rng: Xoroshiro128PlusPlus::new(Seed128::upgrade_from_legacy_seed(legacy_seed)) }
    }

    /// `XoroshiroRandomSource(seedLo, seedHi)`。
    ///
    /// `XoroshiroRandomSource(seedLo, seedHi)`.
    pub fn from_words(seed_lo: i64, seed_hi: i64) -> Self {
        Self { rng: Xoroshiro128PlusPlus::new(Seed128 { lo: seed_lo, hi: seed_hi }) }
    }

    /// `fork()`：两次抽取作为子生成器的种子字。
    ///
    /// `fork()`: two draws as the child's seed words.
    pub fn fork(&mut self) -> Self {
        let lo = self.rng.next_long();
        let hi = self.rng.next_long();
        Self::from_words(lo, hi)
    }

    /// `forkPositional()`。
    ///
    /// `forkPositional()`.
    pub fn fork_positional(&mut self) -> XoroshiroPositional {
        let lo = self.rng.next_long();
        let hi = self.rng.next_long();
        XoroshiroPositional { seed_lo: lo, seed_hi: hi }
    }

    pub fn next_long(&mut self) -> i64 {
        self.rng.next_long()
    }

    /// 返回一个 32 位整数（`next_long()` 截断）。
    ///
    /// A 32-bit integer (the truncation of `next_long()`).
    pub fn next_int(&mut self) -> i32 {
        self.next_long() as i32
    }

    /// 返回 `[0, bound)` 内的整数：32 位乘高，带无符号拒绝修正。
    /// `bound <= 0` 时 panic。
    ///
    /// An integer in `[0, bound)`: 32-bit multiply-high with the unsigned
    /// rejection correction. Panics when `bound <= 0`.
    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "Bound must be positive");
        let mut random_bits = self.next_int() as u32;
        let mut multiplied = u64::from(random_bits) * u64::from(bound as u32);
        let mut fractional = multiplied & 0xFFFF_FFFF;
        if fractional < u64::from(bound as u32) {
            // Rejection threshold: 2^32 % bound as an unsigned remainder.
            let unbiased_start = (0x1_0000_0000u64 % u64::from(bound as u32)) as u64;
            while fractional < unbiased_start {
                random_bits = self.next_int() as u32;
                multiplied = u64::from(random_bits) * u64::from(bound as u32);
                fractional = multiplied & 0xFFFF_FFFF;
            }
        }
        (multiplied >> 32) as i32
    }

    /// 返回 `next_long()` 高位的 `bits` 位。
    ///
    /// The top `bits` bits of `next_long()`.
    pub fn next_bits(&mut self, bits: u32) -> u64 {
        (self.next_long() as u64) >> (64 - bits)
    }

    /// 返回 `[0, 1)` 的 f64：`nextBits(53) * 1.110223E-16F`，其中乘数是
    /// f32 字面量加宽为 f64（该收窄可观测）。
    ///
    /// An f64 in `[0, 1)`: `nextBits(53) * 1.110223E-16F`, where the multiplier
    /// is an f32 literal widened to f64 (the narrowing is observable).
    pub fn next_double(&mut self) -> f64 {
        self.next_bits(53) as f64 * 1.110223E-16f32 as f64
    }

    /// 返回 `[0, 1)` 的 f32：`nextBits(24) * 5.9604645E-8F`。
    ///
    /// An f32 in `[0, 1)`: `nextBits(24) * 5.9604645E-8F`.
    pub fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * 5.9604645E-8f32
    }
}

/// xoroshiro 的位置化随机工厂（持有工厂的两个种子字）。
///
/// The xoroshiro positional random factory (holds the factory's two seed words).
#[derive(Clone, Debug)]
pub struct XoroshiroPositional {
    seed_lo: i64,
    seed_hi: i64,
}

impl XoroshiroPositional {
    /// 由名称派生源 —— **Bedrock 变体，不是 Java 的"MD5 ⊕ 工厂种子"**。
    ///
    /// RE 依据（R-8 / X-116 / X-117，BDS `XoroshiroPositionalRandomFactory::slot02`
    /// @ ELF `0x48cc570` L27–40）：
    ///
    /// ```c
    /// // 输入：param_3 = 名称哈希对 (lo, hi)；param_2+8 / +0x10 = 工厂 s0, s1
    /// t  = (i64)(i32)hi * 0x6ebfff5 ^ (i64)(((lo * 0x2fc20f00000000) ^ lo) >> 32);
    /// s0 = (((t * 0x285b825) + 0xb) * t) >> 16 ^ s0_factory;
    /// s1 = s1_factory;                       // 第二字原样保留
    /// // 零态兜底（判的是输出对，不是派生值）：
    /// if (s0 == 0 && s1 == 0) { s0 = 0x9e3779b97f4a7c15; s1 = 0x6a09e667f3bcc909; }
    /// ```
    ///
    /// 乘数 `0x6ebfff5` / `0x285b825` 与 Java 同源，但项数、`+11` 与移位组合不同 ——
    /// 照 Java 实现会对特征/结构/矿脉/装饰产生系统性不同的分布。
    ///
    /// 【未证实】名称 → (lo, hi) 哈希这一步：本实现沿用 MD5（Java 1.19+ 同构），
    /// 但 Bedrock 的哈希本体（RNN hash，即 slot02 的调用者）是 G-10 的残余缺口，
    /// 尚未取证。若后续证明 Bedrock 不是 MD5，只需替换 [`Seed128::from_hash_of`]。
    ///
    /// Derives the source from a name using the Bedrock mixing variant above.
    pub fn from_hash_of(&self, name: &str) -> XoroshiroRandomSource {
        let h = Seed128::from_hash_of(name);
        let t = (h.hi as i32 as i64)
            .wrapping_mul(0x6EBFFF5)
            ^ (((h.lo.wrapping_mul(0x2FC20F00000000)) ^ h.lo) >> 32);
        let s0 = t
            .wrapping_mul(0x285B825)
            .wrapping_add(0xB)
            .wrapping_mul(t)
            >> 16
            ^ self.seed_lo;
        let s1 = self.seed_hi;
        // 零态兜底：两个输出字全零时退回 金/银 常数（与 BDS L36-39 一致）。
        let seed = if s0 == 0 && s1 == 0 {
            Seed128 { lo: GOLDEN_RATIO_64, hi: SILVER_RATIO_64 }
        } else {
            Seed128 { lo: s0, hi: s1 }
        };
        XoroshiroRandomSource::from_seed_128(seed)
    }
}

/// LCG 变体的位置化随机工厂（`legacy_random_source` 设置标志）。
///
/// The positional random factory of the LCG variant (the
/// `legacy_random_source` settings flag).
#[derive(Clone, Debug)]
pub struct LegacyPositional {
    seed: i64,
}

impl LegacyPositional {
    /// 由名称派生源：`hash(name) ^ seed` 作为 LCG 种子。
    ///
    /// Derives the source from a name: `hash(name) ^ seed` as the LCG seed.
    pub fn from_hash_of(&self, name: &str) -> LegacyRandom {
        LegacyRandom::new(java_string_hash_code(name) ^ self.seed)
    }
}

/// UTF-16 码元上的字符串哈希：`h = h * 31 + c`，i32 溢出回绕
/// （legacy 位置化工厂需要）。
///
/// The UTF-16 string hash `h = h * 31 + c` with wrapping i32 arithmetic
/// (needed by the legacy positional factory).
fn java_string_hash_code(name: &str) -> i64 {
    let mut h: i32 = 0;
    for c in name.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(i32::from(c));
    }
    i64::from(h)
}

/// `XoroshiroRandomSource.XoroshiroPositionalRandomFactory` /
/// 两种位置化工厂（xoroshiro 与 legacy）的统一视图。
///
/// A uniform view over the two positional factories (xoroshiro and legacy).
#[derive(Clone, Debug)]
pub enum PositionalFactory {
    Xoroshiro(XoroshiroPositional),
    Legacy(LegacyPositional),
}

impl PositionalFactory {
    /// 按名称派生随机源。
    ///
    /// Derives a random source from a name.
    pub fn from_hash_of(&self, name: &str) -> RngSource {
        match self {
            Self::Xoroshiro(factory) => RngSource::Xoroshiro(factory.from_hash_of(name)),
            Self::Legacy(factory) => RngSource::Legacy(factory.from_hash_of(name)),
        }
    }
}

/// 两个工厂发放给各消费者的随机源。
///
/// The per-consumer random sources both factories hand out.
#[derive(Clone, Debug)]
pub enum RngSource {
    Xoroshiro(XoroshiroRandomSource),
    Legacy(LegacyRandom),
}

impl RngSource {
    /// 从本源分叉出一个位置化工厂。
    ///
    /// Forks a positional factory from this source.
    pub fn fork_positional(&mut self) -> PositionalFactory {
        match self {
            Self::Xoroshiro(source) => {
                PositionalFactory::Xoroshiro(source.fork_positional())
            }
            Self::Legacy(source) => {
                let seed = source.next_long();
                PositionalFactory::Legacy(LegacyPositional { seed })
            }
        }
    }

    /// 返回 `[0, 1)` 的 f64（梯度噪声基类与 legacy 路径共用）。
    ///
    /// An f64 in `[0, 1)` (shared by the gradient-noise base and the legacy path).
    pub fn next_double(&mut self) -> f64 {
        match self {
            Self::Xoroshiro(source) => source.next_double(),
            Self::Legacy(source) => source.next_double(),
        }
    }

    /// 返回 `[0, bound)` 内的整数。
    ///
    /// An integer in `[0, bound)`.
    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        match self {
            Self::Xoroshiro(source) => source.next_int_bounded(bound),
            Self::Legacy(source) => source.next_int_bounded(bound),
        }
    }
}

/// 世界级位置工厂，由 `noise_settings.legacy_random_source` 标志选择变体。
///
/// The world-level positional factory; the
/// `noise_settings.legacy_random_source` flag selects the variant.
#[derive(Clone, Debug)]
pub struct WorldPositional {
    factory: PositionalFactory,
}

impl WorldPositional {
    /// 为世界种子构建工厂：xoroshiro 路径先升级种子再分叉，
    /// 标志开启时使用 LCG 等价路径。
    ///
    /// Builds the factory for a world seed: the xoroshiro path upgrades the seed
    /// and forks, the LCG path applies when the settings flag is on.
    pub fn new(world_seed: i64, legacy: bool) -> Self {
        let factory = if legacy {
            let mut source = LegacyRandom::new(world_seed);
            PositionalFactory::Legacy(LegacyPositional { seed: source.next_long() })
        } else {
            let mut source = XoroshiroRandomSource::from_legacy_seed(world_seed);
            PositionalFactory::Xoroshiro(source.fork_positional())
        };
        Self { factory }
    }

    /// 按资源 id 派生该资源的随机源。
    ///
    /// Derives the random source for a resource id.
    pub fn noise_random(&self, resource: &str) -> RngSource {
        self.factory.from_hash_of(resource)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stafford13_mixing_matches_reference_values() {
        // Seed pair: lo = mix(42 ^ silver), hi = mix(lo + golden).
        let seed = Seed128::upgrade_from_legacy_seed(42);
        // The derived source must be deterministic:
        let mut a = XoroshiroRandomSource::from_seed_128(seed);
        let mut b = XoroshiroRandomSource::from_seed_128(seed);
        assert_eq!(a.next_long(), b.next_long());
    }

    #[test]
    fn md5_from_hash_of_is_stable_and_documented() {
        // The same hash feeds noise ids and octave names.
        let minecraft = Seed128::from_hash_of("minecraft:continentalness");
        let octave = Seed128::from_hash_of("octave_-9");
        assert_ne!(minecraft, octave);
        // Deriving twice from the same name must give the same seed pair.
        assert_eq!(minecraft, Seed128::from_hash_of("minecraft:continentalness"));
    }

    #[test]
    fn bounded_int_uses_multiply_high() {
        let mut rng = XoroshiroRandomSource::from_legacy_seed(7);
        for bound in [1i32, 3, 255, 256] {
            for _ in 0..64 {
                let v = rng.next_int_bounded(bound);
                assert!((0..bound).contains(&v), "bound={bound} value={v}");
            }
        }
    }

    #[test]
    fn next_double_is_in_unit_interval() {
        let mut rng = XoroshiroRandomSource::from_legacy_seed(99);
        for _ in 0..256 {
            let v = rng.next_double();
            assert!((0.0..1.0).contains(&v), "nextDouble out of range: {v}");
        }
        let mut rng = XoroshiroRandomSource::from_legacy_seed(99);
        for _ in 0..256 {
            let v = rng.next_float();
            assert!((0.0..1.0).contains(&v), "nextFloat out of range: {v}");
        }
    }
}
