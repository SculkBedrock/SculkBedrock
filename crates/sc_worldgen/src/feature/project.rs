//! 区块碎片投射与清单生成。
//!
//! Chunk fragment projection and manifest generation.

use std::collections::BTreeMap;

use sc_world::generation::identity::FragmentId;
use sc_world::generation::owner::{ChunkFragmentPayload, FragmentBlockWrite};

use crate::feature::plan::{FeaturePlan, PlanManifest};

/// 将已接受或被拒绝的 [`FeaturePlan`] 投射到目标区块上。
///
/// Projects an accepted or rejected [`FeaturePlan`] onto target chunks.
///
/// 对于已接受的计划：
/// - 方块写入按目标区块（`x.div_euclid(16), z.div_euclid(16)`）划分；
/// - 坐标转换为区块内局部偏移（`0..16`）；
/// - 生成确定性的 [`FragmentId`] 与 [`PlanManifest`]。
///
/// For an accepted plan:
/// - Block writes are partitioned by target chunk (`x.div_euclid(16), z.div_euclid(16)`).
/// - Coordinates are translated to chunk-local offsets (`0..16`).
/// - Deterministic [`FragmentId`]s and [`PlanManifest`]s are generated.
///
/// 对于被拒绝的计划：
/// - 产出零碎片，以及带清单摘要的权威空清单。
///
/// For a rejected plan:
/// - Emits zero fragments and an authoritative empty manifest with manifest digest.
pub fn project_feature_plan(
    plan: &FeaturePlan,
) -> (Vec<ChunkFragmentPayload>, PlanManifest) {
    let manifest_digest = plan.manifest_digest();

    if !plan.is_accepted() || plan.writes.is_empty() {
        return (
            Vec::new(),
            PlanManifest {
                feature_id: plan.feature_id,
                manifest_digest,
                target_chunks: Vec::new(),
                fragment_count: 0,
            },
        );
    }

    // Group writes by target chunk (BTreeMap guarantees deterministic sorted chunk order)
    let mut by_chunk: BTreeMap<(i32, i32), Vec<FragmentBlockWrite>> = BTreeMap::new();

    for w in &plan.writes {
        let chunk_x = w.world_x.div_euclid(16);
        let chunk_z = w.world_z.div_euclid(16);
        let local_x = w.world_x.rem_euclid(16) as u8;
        let local_z = w.world_z.rem_euclid(16) as u8;

        by_chunk
            .entry((chunk_x, chunk_z))
            .or_default()
            .push(FragmentBlockWrite {
                local_x,
                y: w.world_y,
                local_z,
                runtime_id: w.runtime_id,
                layer: w.layer,
            });
    }

    let target_chunks: Vec<(i32, i32)> = by_chunk.keys().copied().collect();
    let mut fragments = Vec::with_capacity(target_chunks.len());

    for (part_index, ((tcx, tcz), mut writes)) in by_chunk.into_iter().enumerate() {
        writes.sort_unstable_by_key(|w| (w.local_x, w.y, w.local_z, w.layer));

        let fragment_id = FragmentId {
            feature: plan.feature_id,
            target_x: tcx,
            target_z: tcz,
            part_index: part_index as u32,
        };

        fragments.push(ChunkFragmentPayload {
            fragment_id,
            writes,
        });
    }

    let fragment_count = fragments.len() as u32;

    (
        fragments,
        PlanManifest {
            feature_id: plan.feature_id,
            manifest_digest,
            target_chunks,
            fragment_count,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::plan::{FeaturePlan, WorldBlockWrite};
    use sc_world::generation::identity::{FeatureId, FeatureType, GenerationStage, OriginCell};

    fn sample_feature_id() -> FeatureId {
        FeatureId {
            generator_fingerprint: 0xfeed_cafe,
            dimension: 0,
            stage: GenerationStage::Decoration,
            feature_type: FeatureType::Tree,
            origin: OriginCell::new(0, 0),
            candidate_ordinal: 0,
        }
    }

    #[test]
    fn boundary_crossing_feature_projects_into_multiple_target_chunks() {
        let fid = sample_feature_id();
        // Writes spanning across boundary: (15, 64, 15) in (0,0) and (16, 64, 16) in (1,1)
        let writes = vec![
            WorldBlockWrite { world_x: 15, world_y: 64, world_z: 15, runtime_id: 10, layer: 0 },
            WorldBlockWrite { world_x: 16, world_y: 64, world_z: 15, runtime_id: 20, layer: 0 },
            WorldBlockWrite { world_x: 15, world_y: 64, world_z: 16, runtime_id: 20, layer: 0 },
            WorldBlockWrite { world_x: 16, world_y: 64, world_z: 16, runtime_id: 20, layer: 0 },
        ];

        let plan = FeaturePlan::accepted(fid, writes);
        let (fragments, manifest) = project_feature_plan(&plan);

        // Intersects 4 chunks: (0,0), (1,0), (0,1), (1,1)
        assert_eq!(fragments.len(), 4);
        assert_eq!(manifest.fragment_count, 4);
        assert_eq!(
            manifest.target_chunks,
            vec![(0, 0), (0, 1), (1, 0), (1, 1)]
        );

        // Check local coordinates in each fragment
        for f in &fragments {
            for w in &f.writes {
                assert!(w.local_x < 16);
                assert!(w.local_z < 16);
            }
        }
    }

    #[test]
    fn rejected_plan_produces_empty_manifest_with_zero_fragments() {
        let fid = sample_feature_id();
        let plan = FeaturePlan::rejected(
            fid,
            crate::feature::plan::FeatureRejectReason::InvalidFoundation("test".into()),
        );
        let (fragments, manifest) = project_feature_plan(&plan);
        assert!(fragments.is_empty());
        assert_eq!(manifest.fragment_count, 0);
        assert!(manifest.target_chunks.is_empty());
        assert_ne!(manifest.manifest_digest, 0);
    }
}
