//! Vanilla overworld generation pipeline and its building blocks.
//!
//! Conventions used throughout this tree:
//! - **Mirrored layout**: modules follow the reference generator package
//!   structure 1:1 (type/field/constant/method order preserved), so each
//!   file can be checked against the reference sources under `.fetch/`.
//! - **Matching semantics**: integers use wrapping arithmetic (32/64-bit
//!   overflow semantics), `float`/`double` map to `f32`/`f64`, and
//!   floating-point evaluation order is never rearranged.
//! - Each file names its reference source path at the top, and method docs
//!   keep the reference line numbers.
//!
//! | Rust module | Contents |
//! |---|---|
//! | [`random`] | Seeded RNGs (Mersenne-Twister core plus the commons-rng 1.7 internals it builds on) |
//! | [`math`] | Integer/float math helpers (subset) + noise utility functions |
//! | [`noise`] | Perlin/Simplex noise family |
//! | [`densityfunction`] | Density function family |
//! | [`material`] | Material filler, aquifer, and ore veinifier |
//! | [`biome`] | Biome picker family + biome ID table |
//! | [`chunk`] | Chunk state enum + worldgen-chunk adapter subset |
//! | [`context`] | Chunk generation context + block manager (generation subset) |
//! | [`holder`] | Per-world object holders (Normal/Nether/TheEnd variants) |
//! | [`stages`] | Generation stages and the stage chain |
//! | [`feature`] | Generation features (ores and related infrastructure) |
//! | [`populator`] | Structure populators |
pub mod biome;
pub mod chunk;
pub mod context;
pub mod densityfunction;
pub mod feature;
pub mod holder;
pub mod material;
pub mod math;
pub mod noise;
pub mod populator;
pub mod random;
pub mod stages;
