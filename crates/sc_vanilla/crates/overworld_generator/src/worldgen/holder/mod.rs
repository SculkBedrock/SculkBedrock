//! 按维度打包噪声与密度状态的容器类型。
//!
//! 各子容器在构造时按顺序推进同一个随机源。
//!
//! Holder types bundling noise and density state per dimension.
//!
//! Sub-holders advance one shared random source in construction order.

pub mod nether;
pub mod normal;
pub mod the_end;

use crate::worldgen::random::Xoroshiro128;

/// 空的 holder 标记 trait。
///
/// Empty holder marker trait.
pub trait ObjectHolder {}

/// 共享同一随机源的容器对象，构造期间以可变引用传递该随机源。
///
/// Holder object sharing one random source passed by mutable reference during construction.
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

/// 空容器对象。
///
/// Empty holder object.
#[derive(Debug)]
pub struct EmptyObjectHolder;

impl ObjectHolder for EmptyObjectHolder {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::material::MaterialBlocks;
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
}
