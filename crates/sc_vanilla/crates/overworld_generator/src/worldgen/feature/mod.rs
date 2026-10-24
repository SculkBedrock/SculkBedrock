//! Feature traits and submodules for chunk population.
//!
//! | Module | Contents |
//! |---|---|
//! | [`GenerateFeature`] trait | Base feature interface (plus the populator interface) |
//! | [`CountGenerateFeature`] | Count-based feature base |
//! | [`ore`] | Ore generator features and all concrete ore features |
//! | [`object`] | Object/grid/tree generator features |

/// Shared feature-impl template (unifies the former six `impl_*_feature!`
/// macros: `object`/`grid`/`legacy` call `*_apply` directly, while
/// `disc`/`surface`/`count` additionally wire `CountGenerateFeature`).
///
/// Call sites pass the family as the first argument, e.g.
/// `crate::impl_feature!(object, MyFeature, "minecraft:my_feature")`.
#[macro_export]
macro_rules! impl_feature {
    (object, $ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.object_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
    };
    (grid, $ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.grid_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
    };
    (legacy, $ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.legacy_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
    };
    (disc, $ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.count_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
        impl CountGenerateFeature for $ty {
            fn get_base(&self) -> i32 {
                self.disc_base()
            }
            fn get_random(&self) -> i32 {
                self.disc_random()
            }
            fn populate(
                &self,
                ctx: &mut ChunkGenerateContext<'_>,
                random: &mut dyn RandomSourceProvider,
            ) {
                self.disc_populate(ctx, random);
            }
        }
    };
    (surface, $ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.count_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
        impl CountGenerateFeature for $ty {
            fn get_base(&self) -> i32 {
                self.surface_base()
            }
            fn get_random(&self) -> i32 {
                self.surface_random()
            }
            fn populate(
                &self,
                ctx: &mut ChunkGenerateContext<'_>,
                random: &mut dyn RandomSourceProvider,
            ) {
                self.surface_populate(ctx, random);
            }
        }
    };
    (count, $ty:ty, $name:expr) => {
        impl GenerateFeature for $ty {
            fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
                self.count_apply(ctx);
            }
            fn name(&self) -> &'static str {
                $name
            }
        }
        impl CountGenerateFeature for $ty {
            fn get_base(&self) -> i32 {
                self.count_base()
            }
            fn get_random(&self) -> i32 {
                self.count_random()
            }
            fn populate(
                &self,
                ctx: &mut ChunkGenerateContext<'_>,
                random: &mut dyn RandomSourceProvider,
            ) {
                self.count_populate(ctx, random);
            }
        }
    };
}

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

/// 生成特征接口：在区块生成上下文中应用特征。
///
/// Generation feature interface applying a feature within a chunk generation context.
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

/// 按基础数量加随机数量重复执行 populate 的特征接口。
///
/// Feature interface repeating populate a base count plus a random count of times.
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
