//! Structure populators.
//!
//! | Rust module | Contents |
//! |---|---|
//! | This module | `Populator` + `PopulatorStructure` base definitions |
//! | [`placement`] | Structure placement records |
//! | [`structures`] | Small structures (desert well, swamp hut, helpers) |
//! | [`normal`] | Portable subset of the 17 normal structure populators |
//!
//! Design notes:
//! - The abstract `Populator` base maps to the [`Populator`] trait: `root`
//!   merging is done via `ctx.queue_object` (a generator-root buffer), and
//!   the random source is a local `Xoroshiro128` inside `apply` (re-seeded
//!   on every call, matching the shared-source semantics).
//! - The `shouldGenerateStructures` flag (generator setting "structures",
//!   default true) maps to the `ctx.structures` field.
//! - Not covered: NBT-template populators (ruined portal, igloo, fossil,
//!   village, and others needing a structure-NBT loader).

pub mod normal;
pub mod placement;
pub mod structures;

use crate::worldgen::context::ChunkGenerateContext;

// ---------------------------------------------------------------------------
// Populator trait
// ---------------------------------------------------------------------------

/// Base populator: `apply` funnels structure blocks through
/// `ctx.queue_object(places)` into the root buffer, committed at the end
/// of the stage.
pub trait Populator: Send + Sync {
    /// Returns the populator name.
    fn name(&self) -> &'static str;

    /// Runs the populator against the chunk context.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>);
}

// ---------------------------------------------------------------------------
// PopulatorStructure
// ---------------------------------------------------------------------------

/// Returns whether structure generation is enabled (generator setting
/// "structures", default true).
pub fn should_generate_structures(ctx: &ChunkGenerateContext<'_>) -> bool {
    ctx.structures
}
