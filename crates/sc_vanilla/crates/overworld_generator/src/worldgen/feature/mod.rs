//! Feature traits and submodules for chunk population.
//!
//! | Module | Contents |
//! |---|---|
//! | [`GenerateFeature`] trait | Base feature interface (plus the populator interface) |
//! | [`CountGenerateFeature`] | Count-based feature base |
//! | [`ore`] | Ore generator features and all concrete ore features |
//! | [`object`] | Object/grid/tree generator features |

pub mod decoration;
pub mod legacy_tree;
pub mod object;
pub mod ore;
pub mod tree;
pub mod tree_feature;

use crate::worldgen::context::ChunkGenerateContext;
use crate::worldgen::math::java_string_hashcode;
use crate::worldgen::random::RandomSourceProvider;
use crate::worldgen::stages::chunk_hash;

// ---------------------------------------------------------------------------
// GenerateFeature: base feature interface
// ---------------------------------------------------------------------------

/// Base feature interface.
///
/// The block-write root is managed by the stage and passed via `apply`;
/// the RNG is built per apply (seeded from the level seed, chunk position,
/// and feature-name hash).
///
/// Each feature implements `apply` + `name`; the interface matches the stage
/// interface but features run batched inside the chunk-feature stage.
pub trait GenerateFeature: Send + Sync {
    /// Applies the feature to the chunk context.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>);

    /// Returns the feature name.
    fn name(&self) -> &'static str;

    /// Returns the feature identifier (defaults to `name()`).
    fn identifier(&self) -> &'static str {
        self.name()
    }

    /// Builds a per-chunk, per-feature RNG.
    ///
    /// Seed mixes the chunk position hash with the level seed plus the
    /// feature-name hash (addition binds tighter than xor).
    fn make_random(
        &self,
        level_seed: i64,
        chunk_x: i32,
        chunk_z: i32,
    ) -> crate::worldgen::random::Xoroshiro128 {
        let name_hash = java_string_hashcode(self.name()) as i64;
        let seed = chunk_hash(chunk_x, chunk_z) ^ (level_seed.wrapping_add(name_hash));
        crate::worldgen::random::Xoroshiro128::new(seed)
    }
}

// ---------------------------------------------------------------------------
// CountGenerateFeature: count-based feature base
// ---------------------------------------------------------------------------

/// Count-based feature base.
///
/// Calls `populate` a total of `get_base() + random.next_bounded_int(get_random())`
/// times, with a per-chunk seed mixing the level seed, chunk position, and
/// feature-name hash.
///
/// Current concrete case: the extreme-hills surface emerald feature.
pub trait CountGenerateFeature: GenerateFeature {
    /// Base placement count.
    fn get_base(&self) -> i32;

    /// Random extra placement count (upper bound, exclusive).
    fn get_random(&self) -> i32;

    /// Places one instance with the given RNG.
    fn populate(&self, ctx: &mut ChunkGenerateContext<'_>, random: &mut dyn RandomSourceProvider);

    /// Applies the feature `count` times (final dispatch semantics).
    fn count_apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
        let chunk_x = ctx.chunk.x();
        let chunk_z = ctx.chunk.z();
        let level_seed = ctx.level_seed;
        let name_hash = java_string_hashcode(self.name()) as i64;
        let seed = level_seed ^ chunk_hash(chunk_x, chunk_z) ^ name_hash;
        let mut random = crate::worldgen::random::Xoroshiro128::new(seed);

        let count = self
            .get_base()
            .wrapping_add(random.next_bounded_int(self.get_random()));
        for _ in 0..count {
            self.populate(ctx, &mut random);
        }
    }
}
