//! Holder types bundling noise and density state per dimension.
//!
//! | Rust module | Java source |
//! |---|---|
//! | [`normal`] | `NormalObjectHolder.java` |
//! | [`nether`] | `NetherObjectHolder.java` |
//! | [`the_end`] | `TheEndObjectHolder.java` |
//!
//! Holders share one RNG: fork/next_long advance it, identical preserves it.
//! Holders share one RNG via trait plus struct composition.
//! Sub-holders advance shared RNG state in construction order.
//! fork/next_long advance shared state, identical preserves it.
//! fork/next_long advance shared state, identical preserves it. //

pub mod nether;
pub mod normal;
pub mod the_end;

use crate::worldgen::random::Xoroshiro128;

/// Empty holder marker trait.
pub trait ObjectHolder {}

/// Holder sharing a random source.
///
/// Stores a copyable random source threaded through as &mut during construction.
/// Stores a copyable random source threaded through as &mut during construction. //
/// Preserves fork/next_long ordering semantics.
#[derive(Clone, Copy, Debug)]
pub struct RandomizedObjectHolder {
    random: Xoroshiro128,
}

impl RandomizedObjectHolder {
    /// Builds from a random source.
    pub fn new(random: Xoroshiro128) -> Self {
        Self { random }
    }

    /// Java: `@Getter getRandomSourceProvider()`.
    pub fn random(&self) -> Xoroshiro128 {
        self.random
    }
}

impl ObjectHolder for RandomizedObjectHolder {}

/// Empty holder.
#[derive(Debug)]
pub struct EmptyObjectHolder;

impl ObjectHolder for EmptyObjectHolder {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::material::MaterialBlocks;
    use crate::worldgen::random::RandomSourceProvider;
    use sc_world::chunk::BlockRuntimeId;

    fn test_blocks() -> MaterialBlocks {
        MaterialBlocks {
            air: BlockRuntimeId(0),
            water: BlockRuntimeId(1),
            lava: BlockRuntimeId(2),
            stone: BlockRuntimeId(3),
            granite: BlockRuntimeId(4),
            tuff: BlockRuntimeId(5),
            copper_ore: BlockRuntimeId(6),
            deepslate_iron_ore: BlockRuntimeId(7),
            raw_copper_block: BlockRuntimeId(8),
            raw_iron_block: BlockRuntimeId(9),
        }
    }

    #[test]
    fn randomized_object_holder_stores_seed() {
        let r = RandomizedObjectHolder::new(Xoroshiro128::new(42));
        assert_eq!(r.random(), Xoroshiro128::new(42));
    }

    #[test]
    fn normal_holder_deterministic_construction() {
        // Same seed constructs identical RNG state (deterministic fork order).
        let blocks = test_blocks();
        let h1 = normal::NormalObjectHolder::new(Xoroshiro128::new(12345), blocks.clone());
        let h2 = normal::NormalObjectHolder::new(Xoroshiro128::new(12345), blocks);
        // RNG state matches after the same fork/identical/next_long sequence.
        assert_eq!(h1.random(), h2.random());
        assert_eq!(h1.biome_holder().random(), h2.biome_holder().random());
        assert_eq!(h1.terrain_holder().random(), h2.terrain_holder().random());
    }

    #[test]
    fn normal_holder_different_seeds_differ() {
        let blocks = test_blocks();
        let h1 = normal::NormalObjectHolder::new(Xoroshiro128::new(1), blocks.clone());
        let h2 = normal::NormalObjectHolder::new(Xoroshiro128::new(2), blocks);
        assert_ne!(h1.random(), h2.random());
    }

    #[test]
    fn nether_holder_deterministic_construction() {
        let h1 = nether::NetherObjectHolder::new(Xoroshiro128::new(999));
        let h2 = nether::NetherObjectHolder::new(Xoroshiro128::new(999));
        assert_eq!(h1.random(), h2.random());
    }

    #[test]
    fn the_end_holder_deterministic_construction() {
        let h1 = the_end::TheEndObjectHolder::new(Xoroshiro128::new(777));
        let h2 = the_end::TheEndObjectHolder::new(Xoroshiro128::new(777));
        assert_eq!(h1.random(), h2.random());
    }

    #[test]
    fn empty_object_holder_exists() {
        let _ = EmptyObjectHolder;
    }
}
