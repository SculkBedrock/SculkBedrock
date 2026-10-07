//! Density function combinators and spline builders.
//!
//! | Rust module | Java source |
//! |---|---|
//! | [`function`] | `DensityFunction.java`(trait plus noise holder and context) |
//! | [`common`] | `DensityCommon.java`(constants, combinators, caches, and shifted noises) |
//! | [`cubic_spline`] | `CubicSpline.java`(Hermite cubic spline) |
//! | [`continents`] | `DensityContinents.java` |
//! | [`erosion`] | `DensityErosion.java` |
//! | [`ridges`] | `DensityRidges.java` |
//! | [`ridges_folded`] | `DensityRidgesFolded.java` |
//! | [`depth`] | `DensityDepth.java` |
//! | [`offset`] | `DensityOffset.java` |
//! | [`factor`] | `DensityFactor.java` |
//! | [`jaggedness`] | `DensityJaggedness.java` |
//! | [`base3d`] | `DensityBase3dNoise.java` |
//! | [`sloped_cheese`] | `DensitySlopedCheese.java` |
//! | [`caves`] | `OverworldCavesDensity.java` |
//! | [`ore_veins`] | `DensityOreVeins.java` |
//! | [`nether`] | `DensityNether.java` |

pub mod base3d;
pub mod caves;
pub mod common;
pub mod continents;
pub mod cubic_spline;
pub mod depth;
pub mod erosion;
pub mod factor;
pub mod function;
pub mod jaggedness;
pub mod nether;
pub mod offset;
pub mod ore_veins;
pub mod ridges;
pub mod ridges_folded;
pub mod sloped_cheese;
