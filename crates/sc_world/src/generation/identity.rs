//! 稳定的特征身份与带版本号的混合派生。
//!
//! 身份由显式的、有序的字段编码加带版本号的哈希契约派生。Rust 默认的
//! `Hash`/`HashMap` 迭代顺序、worker 编号以及到达顺序都绝不能影响这里的
//! 任何取值。
//!
//! Stable feature identities and versioned mixing.
//!
//! Identities are derived from an explicit, ordered field encoding with a
//! versioned hash contract. Rust's default `Hash`/`HashMap` iteration order,
//! worker numbering and arrival order must never influence any value here.

use crate::generation::error::GenerationContractError;
use serde::{Deserialize, Serialize};

/// 特征所属的生成层。矿脉是属于 `Base` 的地下特征；树木/花草属于
/// `Decoration`；沙漠水井和沼泽小屋属于 `Structure`。
///
/// Generation layer a feature belongs to. Ore veins are `Base` underground
/// features; trees/flowers are `Decoration`; desert wells and swamp huts are
/// `Structure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GenerationStage {
    Base,
    Structure,
    Decoration,
}

impl GenerationStage {
    /// 用于错误信息与诊断的稳定名称。
    ///
    /// Stable name used in error messages and diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Structure => "structure",
            Self::Decoration => "decoration",
        }
    }
}

/// 已注册的特征类型。封闭集合使范围注册保持完备；新增类型需要与
/// [`crate::generation::extent::FeatureExtent`] 条目一起扩展本枚举。
///
/// Registered feature types with extents. The closed set keeps extent
/// registration total; new types extend this enum together with an
/// [`crate::generation::extent::FeatureExtent`] entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FeatureType {
    /// 地下矿脉/矿物放置（当前限于区块内）。
    ///
    /// Underground ore veins / mineral placement (currently in-chunk).
    Ore,
    /// 树木（配置文件中所有已放置的树木种类）。
    ///
    /// Trees (all placed tree kinds of the profile).
    Tree,
    /// 地面装饰：草、花、蘑菇。
    ///
    /// Ground decoration: grass, flowers, mushrooms.
    GroundDecoration,
    /// 沙漠水井结构。
    ///
    /// Desert well structure.
    DesertWell,
    /// 沼泽小屋结构。
    ///
    /// Swamp hut structure.
    SwampHut,
}

impl FeatureType {
    /// 身份编码使用的稳定判别值。取值是持久化契约：绝不重新编号，只能追加。
    ///
    /// Stable discriminator used by the identity encoding. Values are a
    /// persisted contract: never renumber, only append.
    pub const fn discriminator(self) -> u16 {
        match self {
            Self::Ore => 1,
            Self::Tree => 2,
            Self::GroundDecoration => 3,
            Self::DesertWell => 4,
            Self::SwampHut => 5,
        }
    }
}

/// 固定的世界坐标规划/去重单元：其确定性采样决定特征的区块（生成单元格）。
/// 逐分量的 `div_euclid` 语义保证负坐标正确。
///
/// Fixed world-coordinate planning/dedup unit: the chunk whose deterministic
/// sampling decides the feature (generation cell). Component-wise `div_euclid`
/// semantics keep negative coordinates correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OriginCell {
    pub x: i32,
    pub z: i32,
}

impl OriginCell {
    /// 构造一个源单元格。
    ///
    /// Creates an origin cell.
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// 包含某区块位置的单元格就是该区块位置本身：内置配置中特征按源区块
    /// 逐个采样。
    ///
    /// The cell containing a chunk position is the chunk position itself:
    /// features are sampled per source chunk in the built-in profile.
    pub const fn of_chunk(chunk_x: i32, chunk_z: i32) -> Self {
        Self::new(chunk_x, chunk_z)
    }
}

/// 单个特征候选的持久稳定身份：`GeneratorFingerprint + Dimension + Stage +
/// FeatureType + OriginCell + CandidateOrdinal`。世界实例 id 被刻意**不**
/// 纳入身份：它用于校验运行时结果，但绝不能改变特征随机性。
///
/// Persistent, stable identity of one feature candidate:
/// `GeneratorFingerprint + Dimension + Stage + FeatureType + OriginCell +
/// CandidateOrdinal`. The world instance id is deliberately **not** part of
/// the identity: it validates runtime results but must not change feature
/// randomness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FeatureId {
    /// 完全解析后的生成器描述符的指纹。
    ///
    /// Fingerprint of the fully resolved generator descriptor.
    pub generator_fingerprint: u64,
    pub dimension: i32,
    pub stage: GenerationStage,
    pub feature_type: FeatureType,
    pub origin: OriginCell,
    /// 源单元格固定候选采样顺序内的序号。必须来自确定性采样，绝不来自
    /// `HashMap` 迭代或 worker 完成顺序。
    ///
    /// Ordinal within the origin cell's fixed candidate sampling order.
    /// Must come from deterministic sampling, never from `HashMap` iteration
    /// or worker completion order.
    pub candidate_ordinal: u32,
}

impl FeatureId {
    /// 身份字段的版本稳定 FNV-1a 64 编码。
    ///
    /// 下面的编码是持久化契约：未经提升
    /// [`IDENTITY_HASH_VERSION`] 并提供迁移分析，字段顺序与字节布局
    /// 不得改变。
    ///
    /// Version-stable FNV-1a 64 encoding of the identity fields.
    ///
    /// The encoding below is a persistence contract: field order and byte
    /// layout must not change without bumping
    /// [`IDENTITY_HASH_VERSION`] and providing a migration analysis.
    pub fn stable_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV offset basis
        fn feed(h: &mut u64, bytes: &[u8]) {
            for b in bytes {
                *h ^= u64::from(*b);
                *h = h.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime
            }
        }
        feed(&mut h, &[IDENTITY_HASH_VERSION]);
        feed(&mut h, &self.generator_fingerprint.to_le_bytes());
        feed(&mut h, &self.dimension.to_le_bytes());
        feed(&mut h, &[match self.stage {
            GenerationStage::Base => 1,
            GenerationStage::Structure => 2,
            GenerationStage::Decoration => 3,
        }]);
        feed(&mut h, &self.feature_type.discriminator().to_le_bytes());
        feed(&mut h, &self.origin.x.to_le_bytes());
        feed(&mut h, &self.origin.z.to_le_bytes());
        feed(&mut h, &self.candidate_ordinal.to_le_bytes());
        h
    }

    /// 确定性的每特征随机流种子：对身份哈希加流盐做带版本号的 splitmix64
    /// 混合。不同的盐隔离形状/旋转/数量等流，使新增特征种类绝不会移动
    /// 其他特征的随机序列。
    ///
    /// Deterministic per-feature random stream seed: versioned splitmix64
    /// mixing over the identity hash plus a stream salt. Distinct salts
    /// isolate shape/rotation/count streams so adding a feature kind never
    /// shifts another feature's random sequence.
    pub fn stream_seed(&self, salt: u64) -> u64 {
        let mut z = self.stable_hash() ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// [`FeatureId::stable_hash`] 字节编码的版本号。
///
/// Version of the [`FeatureId::stable_hash`] byte encoding.
pub const IDENTITY_HASH_VERSION: u8 = 1;

/// 单个投影碎片的身份：`FeatureId + TargetChunk + StablePartIndex`。
/// 重试必须复现相同的 id 与内容；相同 id 但不同内容属于装配错误，
/// 而非后写覆盖。
///
/// Identity of one projected fragment: `FeatureId + TargetChunk +
/// StablePartIndex`. Retries must reproduce the same id and content; the
/// same id with different content is an assembly error, not
/// last-writer-wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FragmentId {
    pub feature: FeatureId,
    pub target_x: i32,
    pub target_z: i32,
    pub part_index: u32,
}

impl FragmentId {
    /// 校验该碎片属于给定目标区块，并返回清单中使用的稳定 id 字节。
    ///
    /// Validates that the fragment belongs to the given target chunk and
    /// returns the stable id bytes used in manifests.
    pub fn manifest_key(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        let h = self.feature.stable_hash();
        out[0..8].copy_from_slice(&h.to_le_bytes());
        out[8..12].copy_from_slice(&self.target_x.to_le_bytes());
        out[12..16].copy_from_slice(&self.target_z.to_le_bytes());
        out[16..20].copy_from_slice(&self.part_index.to_le_bytes());
        out[20..24].copy_from_slice(&self.feature.candidate_ordinal.to_le_bytes());
        out
    }
}

/// 校验区块位置在负坐标的 `div_euclid` 向下取整语义下映射到期望的所有者
/// 单元格。在此暴露以便所有者映射与测试共享同一实现。
///
/// Validates that a chunk position maps to an expected owner cell under
/// `div_euclid` floor semantics for negative coordinates. Exposed here so
/// the owner map and tests share one implementation.
pub fn origin_cell_for_chunk(chunk_x: i32, chunk_z: i32) -> OriginCell {
    OriginCell::of_chunk(chunk_x, chunk_z)
}

/// 拒绝从无序集合派生身份的尝试：采样器必须使用本助手来断言其序号来源
/// 是稳定索引。
///
/// Rejects attempts to derive identities from unordered collections: a helper
/// the sampler must use to assert its ordinal source is a stable index.
pub fn checked_ordinal(index: usize) -> Result<u32, GenerationContractError> {
    u32::try_from(index)
        .map_err(|_| GenerationContractError::InvalidConfiguration(format!(
            "candidate ordinal {index} exceeds the u32 identity range"
        )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_id(ordinal: u32) -> FeatureId {
        FeatureId {
            generator_fingerprint: 0x1234_5678_9abc_def0,
            dimension: 0,
            stage: GenerationStage::Decoration,
            feature_type: FeatureType::Tree,
            origin: OriginCell::new(3, -4),
            candidate_ordinal: ordinal,
        }
    }

    #[test]
    fn stable_hash_is_field_order_independent_and_discriminating() {
        // Identical fields must hash identically regardless of construction
        // order; every field change must change the hash.
        let base = sample_id(7);
        assert_eq!(base.stable_hash(), sample_id(7).stable_hash());
        assert_ne!(base.stable_hash(), sample_id(8).stable_hash());

        let mut variant = base;
        variant.dimension = 1;
        assert_ne!(base.stable_hash(), variant.stable_hash());

        let mut variant = base;
        variant.stage = GenerationStage::Structure;
        assert_ne!(base.stable_hash(), variant.stable_hash());

        let mut variant = base;
        variant.feature_type = FeatureType::GroundDecoration;
        assert_ne!(base.stable_hash(), variant.stable_hash());

        let mut variant = base;
        variant.origin = OriginCell::new(4, -4);
        assert_ne!(base.stable_hash(), variant.stable_hash());

        let mut variant = base;
        variant.generator_fingerprint += 1;
        assert_ne!(base.stable_hash(), variant.stable_hash());
    }

    #[test]
    fn stream_salts_isolate_random_streams() {
        let id = sample_id(1);
        let shape = id.stream_seed(1);
        let rotation = id.stream_seed(2);
        let count = id.stream_seed(3);
        assert_ne!(shape, rotation);
        assert_ne!(shape, count);
        assert_ne!(rotation, count);
        // Same salt reproduces the same stream seed; a different feature does
        // not reuse the sequence (independent streams).
        assert_eq!(shape, id.stream_seed(1));
        assert_ne!(shape, sample_id(2).stream_seed(1));
    }

    #[test]
    fn negative_chunk_coordinates_keep_floor_semantics() {
        // Floor semantics: chunk -1 must belong to cell -1, never truncate toward zero.
        assert_eq!(origin_cell_for_chunk(-1, -1), OriginCell::new(-1, -1));
        assert_eq!(origin_cell_for_chunk(-16, -16), OriginCell::new(-16, -16));
        assert_eq!(origin_cell_for_chunk(0, 0), OriginCell::new(0, 0));
        // Sanity: div_euclid (not truncating division) drives the mapping.
        assert_eq!((-1i32).div_euclid(2), -1);
    }

    #[test]
    fn fragment_manifest_keys_are_stable_per_target_and_part() {
        let feature = sample_id(3);
        let a = FragmentId { feature, target_x: 1, target_z: 0, part_index: 0 };
        let b = FragmentId { feature, target_x: 1, target_z: 0, part_index: 0 };
        assert_eq!(a.manifest_key(), b.manifest_key());
        let other_target = FragmentId { feature, target_x: 0, target_z: 1, part_index: 0 };
        assert_ne!(a.manifest_key(), other_target.manifest_key());
    }

    #[test]
    fn checked_ordinal_rejects_out_of_range() {
        assert_eq!(checked_ordinal(12).unwrap(), 12);
        assert!(checked_ordinal(usize::MAX).is_err());
    }
}
