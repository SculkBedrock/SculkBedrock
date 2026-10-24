//! 目标区块的依赖跟踪与组装屏障。
//!
//! Target chunk dependency tracking and assembly barrier.

use std::sync::Arc;

use crate::generation::assemble::assemble_final_candidate;
use crate::generation::error::GenerationContractError;
use crate::generation::owner::{
    ChunkFragmentPayload, FinalCandidatePayload, FragmentBlockWrite, SubmissionTicket,
};
use crate::generation::record::{ChunkGenerationRecord, ReadyCertificate};
use crate::generation::view::BaseSnapshot;

/// 跟踪单个目标区块的在途生成进度、传入的封包以及缓存的碎片。
///
/// Tracks in-flight generation progress, incoming seals, and buffered
/// fragments for one target chunk.
pub struct TargetChunkTracker {
    pub dimension: i32,
    pub target_x: i32,
    pub target_z: i32,
    /// 准备好的基础快照（尚未就绪时为 `None`）。
    ///
    /// Prepared base snapshot, `None` until available.
    pub base: Option<Arc<BaseSnapshot>>,
    /// 已接收并缓存的碎片。
    ///
    /// Received and buffered fragments.
    pub buffered_fragments: Vec<ChunkFragmentPayload>,
    /// 缓冲碎片数量上限。
    ///
    /// Maximum number of buffered fragments.
    pub max_buffered_fragments: usize,
    /// 缓冲字节数上限。
    ///
    /// Maximum buffered bytes.
    pub max_buffered_bytes: usize,
    /// 当前已缓冲的字节数。
    ///
    /// Currently buffered bytes.
    pub current_buffered_bytes: usize,
}

impl TargetChunkTracker {
    /// 创建一个目标区块跟踪器；各上限会被抬升到最小可用值。
    ///
    /// Creates a target chunk tracker; limits are raised to their minimum
    /// usable values.
    pub fn new(
        dimension: i32,
        target_x: i32,
        target_z: i32,
        max_buffered_fragments: usize,
        max_buffered_bytes: usize,
    ) -> Self {
        Self {
            dimension,
            target_x,
            target_z,
            base: None,
            buffered_fragments: Vec::new(),
            max_buffered_fragments: max_buffered_fragments.max(1),
            max_buffered_bytes: max_buffered_bytes.max(1024),
            current_buffered_bytes: 0,
        }
    }

    /// 基础快照准备好后记录一次。
    ///
    /// Records the base snapshot once prepared.
    pub fn set_base(&mut self, snapshot: Arc<BaseSnapshot>) {
        self.base = Some(snapshot);
    }

    /// 入队一个已校验的区块碎片，并强制执行缓冲上限。
    ///
    /// - `fragment`: 已校验碎片，其目标坐标必须匹配本跟踪器
    ///
    /// Enqueues a verified chunk fragment, enforcing buffer limits.
    ///
    /// - `fragment`: a verified fragment whose target must match this tracker
    pub fn push_fragment(
        &mut self,
        fragment: ChunkFragmentPayload,
    ) -> Result<(), GenerationContractError> {
        let fid = &fragment.fragment_id;
        if fid.target_x != self.target_x || fid.target_z != self.target_z {
            return Err(GenerationContractError::InvalidConfiguration(format!(
                "fragment for target ({},{}) pushed to tracker for ({},{})",
                fid.target_x, fid.target_z, self.target_x, self.target_z
            )));
        }

        if self.buffered_fragments.len() >= self.max_buffered_fragments {
            return Err(GenerationContractError::DependencyBudgetExceeded {
                target_x: self.target_x,
                target_z: self.target_z,
                required_cells: self.buffered_fragments.len() + 1,
                budget: self.max_buffered_fragments,
            });
        }

        let bytes = std::mem::size_of::<ChunkFragmentPayload>()
            + fragment.writes.len() * std::mem::size_of::<FragmentBlockWrite>();

        if self.current_buffered_bytes.saturating_add(bytes) > self.max_buffered_bytes {
            return Err(GenerationContractError::DependencyBudgetExceeded {
                target_x: self.target_x,
                target_z: self.target_z,
                required_cells: self.current_buffered_bytes + bytes,
                budget: self.max_buffered_bytes,
            });
        }

        self.current_buffered_bytes += bytes;
        self.buffered_fragments.push(fragment);
        Ok(())
    }

    /// 私有组装的所有前置条件是否已满足：
    /// 1. 基础快照可用；
    /// 2. 所有必需的结构源已封包；
    /// 3. 所有必需的装饰源已封包；
    /// 4. 所有声明的碎片都已接收并缓冲。
    ///
    /// Whether all preconditions for private assembly are met:
    /// 1. Base snapshot is available;
    /// 2. All required structure sources sealed;
    /// 3. All required decoration sources sealed;
    /// 4. All declared fragments have been received and buffered.
    pub fn is_ready_for_assembly(&self, record: &ChunkGenerationRecord) -> bool {
        self.base.is_some()
            && record.structure_seals.is_complete()
            && record.decoration_seals.is_complete()
            && record.received_fragments == record.declared_fragments
            && self.buffered_fragments.len() == record.received_fragments as usize
    }

    /// 在 worker 线程上执行私有最终组装，构造最终候选列及其已验证的
    /// ReadyCertificate。
    ///
    /// Executes private final assembly on a worker thread, constructing the
    /// final candidate column and its verified ReadyCertificate.
    pub fn assemble(
        &mut self,
        ticket: SubmissionTicket,
    ) -> Result<(FinalCandidatePayload, ReadyCertificate), GenerationContractError> {
        let base = self.base.as_ref().ok_or_else(|| {
            GenerationContractError::MissingDependency {
                dimension: self.dimension,
                chunk_x: self.target_x,
                chunk_z: self.target_z,
                stage: "base",
            }
        })?;

        assemble_final_candidate(ticket, base, &self.buffered_fragments, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPosition};
    use crate::generation::identity::{FeatureId, FeatureType, FragmentId, GenerationStage, OriginCell};
    use crate::generation::owner::RegionOwnerKey;

    fn make_test_base(cx: i32, cz: i32) -> Arc<BaseSnapshot> {
        let chunk = Chunk::empty(ChunkPosition::new(cx, cz), 0, -64, 319);
        Arc::new(BaseSnapshot::new(
            0,
            ChunkPosition::new(cx, cz),
            1,
            0xcafe,
            -64,
            319,
            Arc::new(chunk),
            [64; 256],
        ))
    }

    #[test]
    fn tracker_waits_for_complete_seals_and_fragments_before_assembly() {
        let mut tracker = TargetChunkTracker::new(0, 0, 0, 16, 100_000);
        let mut record = ChunkGenerationRecord::new_queued(0, 0, 0, 0xcafe);

        // Not ready without base
        assert!(!tracker.is_ready_for_assembly(&record));

        let base = make_test_base(0, 0);
        tracker.set_base(base.clone());
        record.base_digest = Some(base.digest);

        // Register seals
        record.register_expected_seals(GenerationStage::Structure, vec![OriginCell::new(0, 0)]).unwrap();
        record.register_expected_seals(GenerationStage::Decoration, vec![OriginCell::new(0, 0)]).unwrap();

        // Not ready while seals pending
        assert!(!tracker.is_ready_for_assembly(&record));

        // Apply structure seal (declared 1 fragment)
        record.apply_seal(GenerationStage::Structure, OriginCell::new(0, 0), 101, 1).unwrap();
        assert!(!tracker.is_ready_for_assembly(&record));

        // Buffer fragment
        let fid = FeatureId {
            generator_fingerprint: 0xcafe,
            dimension: 0,
            stage: GenerationStage::Structure,
            feature_type: FeatureType::DesertWell,
            origin: OriginCell::new(0, 0),
            candidate_ordinal: 0,
        };
        let frag = ChunkFragmentPayload {
            fragment_id: FragmentId {
                feature: fid,
                target_x: 0,
                target_z: 0,
                part_index: 0,
            },
            writes: vec![FragmentBlockWrite {
                local_x: 5,
                y: 65,
                local_z: 5,
                runtime_id: 111,
                layer: 0,
            }],
        };
        tracker.push_fragment(frag).unwrap();
        record.accept_fragment(1).unwrap();

        // Still not ready because decoration seal is not complete
        assert!(!tracker.is_ready_for_assembly(&record));

        // Apply decoration seal (0 declared fragments)
        record.apply_seal(GenerationStage::Decoration, OriginCell::new(0, 0), 102, 0).unwrap();

        // Now all conditions met!
        assert!(tracker.is_ready_for_assembly(&record));

        // Run assembly
        let ticket = SubmissionTicket::new(RegionOwnerKey::new(0, 0, 0), 1, 0, 0, 1);
        let (cand, cert) = tracker.assemble(ticket).unwrap();
        assert_eq!(cand.chunk_x, 0);
        assert_eq!(cand.chunk_z, 0);
        assert!(cert.all_fragments_verified);
    }
}
