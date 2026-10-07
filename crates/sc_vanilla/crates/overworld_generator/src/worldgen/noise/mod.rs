//! Noise samplers (Perlin, simplex, octave stacks, splines).
//!
//! | Rust module | Java package |
//! |---|---|
//! | [`noise`] | `...noise.minecraft.noise`(Noise/NoiseSampler/PerlinNoise/NormalNoise) |
//! | [`perlin`] | `...noise.minecraft.perlin`(PerlinNoiseSampler/OctavePerlinNoiseSampler) |
//! | [`simplex`] | `...noise.minecraft.simplex`(SimplexNoiseSampler/OctaveSimplexNoiseSampler/SimplexNoise) |
//! | [`f`] | `...noise.f`(NoiseF/PerlinF/SimplexF for legacy terrain and features) |
//! | [`d`] | `...noise.d`(improved/octave noises for legacy trees) |
//! | [`spline`] | `...noise.spline`(spline generator plus factor/jaggedness/offset splines) |
//!
//! Reference sources live under .fetch/.

pub mod d;
pub mod f;
pub mod noise;
pub mod perlin;
pub mod simplex;
pub mod spline;
