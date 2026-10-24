//! 噪声内核：播种的随机源、Perlin 八度栈、双栈 `NormalNoise`，以及按种子
//! 隔离的 [`NoiseBank`]。
//!
//! Noise kernel: seeded random sources, perlin octave stacks, the two-stack
//! `NormalNoise`, and the per-seed [`NoiseBank`].
//!
//! 所有算法的输出按位确定：同一输入与种子必须永远产生相同取值。
//!
//! Every algorithm is bit-deterministic: the same input and seed must always
//! produce identical values.

pub mod bank;
pub mod legacy_random;
pub mod normal26;
pub mod noise;
pub mod perlin;
pub mod positional;
pub mod random;
pub mod simplex;
pub mod synth;

pub use bank::NoiseBank;
pub use legacy_random::{java_string_hash_code, LegacyRandom};
pub use normal26::{Normalize26, NormalNoise26, NoiseParams26};
pub use noise::{NormalNoise, PerlinNoise};
pub use perlin::{OctavePerlinNoiseSampler, PerlinNoiseSampler};
pub use positional::{
    PositionalFactory, RngSource, Seed128, WorldPositional, Xoroshiro128PlusPlus,
    XoroshiroPositional, XoroshiroRandomSource,
};
pub use random::{MtRandom, RandomSourceProvider, Xoroshiro128};
pub use simplex::SimplexNoise;
pub use synth::{NoiseStack, SmearedPerlinNoise, StackNoise};
