//! 确定性候选采样。
//!
//! Deterministic candidate sampling.
//!
//! 候选采样是纯粹、确定且按种子隔离的：
//!
//! Candidate sampling is pure, deterministic, and seed-isolated:
//! - 只依赖生成器指纹、维度、阶段、源单元格与候选序号；
//! - 通过 [`FeatureId::stream_seed`] 配合专用盐值使用隔离的随机流，
//!   确保新增一种特性类型绝不会改变其他特性的随机序列；
//! - 热路径采样零运行时分配、零 JSON 解析，且不依赖工作者到达顺序或
//!   线程身份。
//!
//! - It depends ONLY on the generator fingerprint, dimension, stage, origin cell,
//!   and candidate ordinal.
//! - It uses isolated random streams via [`FeatureId::stream_seed`] with dedicated salts,
//!   ensuring that adding one feature type never shifts the random sequence of another.
//! - Zero runtime allocations on hot sampling, zero JSON parsing, and zero dependency
//!   on worker arrival order or thread identity.

use sc_world::generation::identity::{
    checked_ordinal, FeatureId, FeatureType, GenerationStage, OriginCell,
};

/// 采样候选者在源单元格内 X 偏移（0..16）所用的盐值。
///
/// Salt for sampling candidate X offset within the origin cell (0..16).
pub const SALT_OFFSET_X: u64 = 0x1001;
/// 采样候选者在源单元格内 Z 偏移（0..16）所用的盐值。
///
/// Salt for sampling candidate Z offset within the origin cell (0..16).
pub const SALT_OFFSET_Z: u64 = 0x1002;
/// 采样候选者高度/类型变异所用的盐值。
///
/// Salt for sampling candidate height/type variation.
pub const SALT_VARIANT: u64 = 0x1003;
/// 结构生成概率/稀有度检查所用的盐值。
///
/// Salt for structure probability/rarity check.
pub const SALT_RARITY: u64 = 0x1004;

/// 等待依赖地形求解的确定性特性候选。
///
/// A deterministic feature candidate awaiting terrain-dependent solving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureCandidate {
    pub feature_id: FeatureId,
    pub feature_type: FeatureType,
    pub anchor_x: i32,
    pub anchor_z: i32,
    pub variant: u32,
}

/// 为单元格产出候选特性的确定性采样器。
///
/// Deterministic sampler producing candidate features for a cell.
pub struct CandidateSampler;

impl CandidateSampler {
    /// 为一个源单元格采样确定性候选。
    ///
    /// Samples deterministic candidates for an origin cell.
    pub fn sample_candidates_for_cell(
        generator_fingerprint: u64,
        dimension: i32,
        origin: OriginCell,
        feature_type: FeatureType,
        max_candidates: u32,
    ) -> Vec<FeatureCandidate> {
        let (stage, count) = match feature_type {
            FeatureType::Tree => (GenerationStage::Decoration, max_candidates.clamp(1, 16)),
            FeatureType::GroundDecoration => (GenerationStage::Decoration, max_candidates.clamp(1, 32)),
            FeatureType::DesertWell | FeatureType::SwampHut => (GenerationStage::Structure, 1),
            FeatureType::Ore => (GenerationStage::Base, max_candidates.clamp(1, 16)),
        };

        let mut candidates = Vec::with_capacity(count as usize);

        for ord in 0..count {
            let ordinal = match checked_ordinal(ord as usize) {
                Ok(o) => o,
                Err(_) => break,
            };

            let feature_id = FeatureId {
                generator_fingerprint,
                dimension,
                stage,
                feature_type,
                origin,
                candidate_ordinal: ordinal,
            };

            // Structure rarity roll: e.g. 1 in 16 cells for rare structures
            if matches!(feature_type, FeatureType::DesertWell | FeatureType::SwampHut) {
                let rarity_roll = feature_id.stream_seed(SALT_RARITY) % 16;
                // If the roll is not 0, no structure in this cell
                if rarity_roll != 0 {
                    continue;
                }
            }

            let off_x = (feature_id.stream_seed(SALT_OFFSET_X) % 16) as i32;
            let off_z = (feature_id.stream_seed(SALT_OFFSET_Z) % 16) as i32;
            let variant = (feature_id.stream_seed(SALT_VARIANT) % 4) as u32;

            let anchor_x = origin.x * 16 + off_x;
            let anchor_z = origin.z * 16 + off_z;

            candidates.push(FeatureCandidate {
                feature_id,
                feature_type,
                anchor_x,
                anchor_z,
                variant,
            });
        }

        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_sampling_is_deterministic_and_order_independent() {
        let origin = OriginCell::new(3, -5);
        let a = CandidateSampler::sample_candidates_for_cell(0x1234, 0, origin, FeatureType::Tree, 4);
        let b = CandidateSampler::sample_candidates_for_cell(0x1234, 0, origin, FeatureType::Tree, 4);
        assert_eq!(a, b);
        assert_eq!(a.len(), 4);

        // Offsets are strictly within cell bounds (0..16)
        for c in &a {
            let local_x = c.anchor_x.rem_euclid(16);
            let local_z = c.anchor_z.rem_euclid(16);
            assert!((0..16).contains(&local_x));
            assert!((0..16).contains(&local_z));
        }
    }

    #[test]
    fn different_feature_types_have_isolated_streams() {
        let origin = OriginCell::new(0, 0);
        let trees = CandidateSampler::sample_candidates_for_cell(0x1234, 0, origin, FeatureType::Tree, 1);
        let ores = CandidateSampler::sample_candidates_for_cell(0x1234, 0, origin, FeatureType::Ore, 1);
        assert_ne!(trees[0].anchor_x, ores[0].anchor_x);
    }
}
