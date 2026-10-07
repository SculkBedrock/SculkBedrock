//! Biome pickers and biome ID table.
//!
//! | Rust file | Contents |
//! |---|---|
//! | [`biome_id`] | Integer biome ID constants |
//! | [`overworld`] | Overworld biome picker + overworld biome result |
//!
//! Nether/TheEnd pickers are future work; the first version only covers
//! the overworld.
//!
//! Design notes:
//! - The generic `BiomePicker<E extends BiomeResult>` base maps to the
//!   [`BiomePicker`] trait (associated type `Result`), avoiding a generic
//!   trait on the Rust side.
//! - The mutable `biomeId` field of the `BiomeResult` base maps to the
//!   [`BiomeResult`] trait (`biome_id` / `set_biome_id`); each concrete
//!   result embeds a `biome_id: i32` field.
//! - The base-class seeded RNG field is kept on the struct for shape
//!   fidelity even though picking does not consume it (a Nether picker
//!   would fork it).

pub mod biome_id;
pub mod overworld;

/// Mutable biome-result base: `biome_id` is overwritten by depth-based
/// correction and restored to `original` by `reset()`.
pub trait BiomeResult {
    /// Returns the current biome id.
    fn biome_id(&self) -> i32;

    /// Overwrites the biome id (used by correction/reset).
    fn set_biome_id(&mut self, id: i32);
}

/// Picks the biome at a block position.
pub trait BiomePicker {
    type Result: BiomeResult;

    /// Picks the biome result for the given block coordinates.
    fn pick(&self, x: i32, y: i32, z: i32) -> Self::Result;
}
