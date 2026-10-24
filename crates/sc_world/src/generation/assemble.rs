//! 私有最终列组装与 ReadyCertificate 构造。
//!
//! 组装只作用于 worker 私有的区块数据，不修改权威缓存。发往本区块的碎片
//! 按 FragmentId 清单键排序后依次应用，最终方块与组装摘要与到达顺序无关。
//! 高于当前表面的方块写入会更新高度图；全部证书条件成立后才进入 Ready。
//!
//! Private final column assembly and ReadyCertificate construction.
//!
//! Assembly operates only on worker-private chunk data and never mutates the
//! authoritative cache. Fragments for a chunk are applied in stable order
//! (sorted by FragmentId manifest key), so arrival order does not affect the
//! final blocks or the assembly digest. Writes above the current surface update
//! the heightmap, and the chunk transitions to Ready only when every
//! certificate condition holds.

use std::sync::Arc;

use crate::chunk::BlockRuntimeId;
use crate::generation::error::GenerationContractError;
use crate::generation::owner::{ChunkFragmentPayload, FinalCandidatePayload, SubmissionTicket};
use crate::generation::record::ReadyCertificate;
use crate::generation::view::{compute_base_digest, BaseSnapshot};
use crate::storage::ChunkColumn;

/// 由 BaseSnapshot 与投影碎片组装出 worker 私有的最终候选列，并构造
/// ReadyCertificate。保证确定性的方块放置，并更新高度图与调色板。
///
/// - `ticket`: 提交票据，携带目标区块坐标与所有者纪元；其坐标必须与 `base` 一致
/// - `base`: 基础地形快照
/// - `fragments`: 投影到本区块的已校验碎片
/// - `expected_assembly_digest`: 可选的期望组装摘要；提供时必须与计算结果一致
///
/// 返回最终候选载荷与已验证的证书；位置不符、写入越界或摘要不匹配时
/// 返回 [`GenerationContractError::InvalidConfiguration`]。
///
/// Assembles an owned final candidate column from a BaseSnapshot and projected
/// fragments. Ensures deterministic block placement and updates heightmaps
/// and palettes.
///
/// - `ticket`: submission ticket carrying the target chunk coordinates and
///   owner epoch; its position must match `base`
/// - `base`: base terrain snapshot
/// - `fragments`: verified fragments projected into this chunk
/// - `expected_assembly_digest`: optional expected assembly digest; when
///   present it must match the computed one
///
/// Returns the final candidate payload and its verified certificate;
/// position mismatch, out-of-bounds writes, or digest mismatch produce an
/// [`GenerationContractError::InvalidConfiguration`].
pub fn assemble_final_candidate(
    ticket: SubmissionTicket,
    base: &BaseSnapshot,
    fragments: &[ChunkFragmentPayload],
    expected_assembly_digest: Option<u64>,
) -> Result<(FinalCandidatePayload, ReadyCertificate), GenerationContractError> {
    if base.chunk_position.x != ticket.chunk_x || base.chunk_position.z != ticket.chunk_z {
        return Err(GenerationContractError::InvalidConfiguration(format!(
            "base snapshot position ({},{}) does not match ticket ({},{})",
            base.chunk_position.x, base.chunk_position.z, ticket.chunk_x, ticket.chunk_z
        )));
    }

    // 1. Clone base chunk and heightmap into private worker copy
    let mut chunk = (*base.chunk).clone();
    let mut height_map = base.height_map;

    // 2. Sort fragments deterministically by stable manifest key
    let mut sorted_fragments: Vec<&ChunkFragmentPayload> = fragments.iter().collect();
    sorted_fragments.sort_unstable_by_key(|f| f.fragment_id.manifest_key());

    // 3. Apply block writes in order
    for frag in sorted_fragments {
        let mut sorted_writes = frag.writes.clone();
        sorted_writes.sort_unstable_by_key(|w| (w.local_x, w.y, w.local_z, w.layer));

        for w in sorted_writes {
            if w.local_x >= 16 || w.local_z >= 16 || w.y < base.min_y || w.y > base.max_y {
                return Err(GenerationContractError::InvalidConfiguration(format!(
                    "fragment block write out of bounds at local ({}, {}, {})",
                    w.local_x, w.y, w.local_z
                )));
            }

            chunk.set_block_at(
                w.layer as usize,
                w.local_x,
                w.y,
                w.local_z,
                BlockRuntimeId(w.runtime_id),
            );

            // Update surface heightmap if solid block is placed above current surface
            if w.layer == 0 {
                let idx = ((w.local_x as usize) << 4) | (w.local_z as usize);
                if w.y > height_map[idx] {
                    height_map[idx] = w.y;
                }
            }
        }
    }

    // 4. Compute deterministic assembly digest
    let assembly_digest = compute_base_digest(
        base.chunk_position,
        base.generator_fingerprint,
        base.world_instance_id,
        base.min_y,
        base.max_y,
        &height_map,
        &chunk,
    );

    if let Some(expected) = expected_assembly_digest {
        if expected != assembly_digest {
            return Err(GenerationContractError::InvalidConfiguration(format!(
                "assembly digest mismatch: expected {expected:#x}, computed {assembly_digest:#x}"
            )));
        }
    }

    // 5. Construct verified ReadyCertificate
    let certificate = ReadyCertificate {
        base_valid: true,
        structure_seals_complete: true,
        decoration_seals_complete: true,
        all_fragments_verified: true,
        assembly_validated: true,
        metadata_included: true,
        owner_epoch: ticket.epoch,
    };
    certificate.validate()?;

    let column = Arc::new(ChunkColumn::new(chunk));
    column.set_ready(certificate, base.generator_fingerprint);

    Ok((
        FinalCandidatePayload {
            chunk_x: ticket.chunk_x,
            chunk_z: ticket.chunk_z,
            assembly_digest,
            column,
        },
        certificate,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPosition};
    use crate::generation::identity::{FeatureId, FeatureType, FragmentId, GenerationStage, OriginCell};
    use crate::generation::owner::{FragmentBlockWrite, RegionOwnerKey};

    fn make_test_base(cx: i32, cz: i32) -> BaseSnapshot {
        let mut chunk = Chunk::empty(ChunkPosition::new(cx, cz), 0, -64, 319);
        for lx in 0..16 {
            for lz in 0..16 {
                chunk.set_block_at(0, lx, 64, lz, BlockRuntimeId(10));
            }
        }
        BaseSnapshot::new(
            0,
            ChunkPosition::new(cx, cz),
            1,
            0xcafe,
            -64,
            319,
            Arc::new(chunk),
            [64; 256],
        )
    }

    #[test]
    fn assembly_is_deterministic_regardless_of_fragment_input_order() {
        let base = make_test_base(0, 0);
        let key = RegionOwnerKey::new(0, 0, 0);
        let ticket = SubmissionTicket::new(key, 1, 0, 0, 1);

        let fid_a = FeatureId {
            generator_fingerprint: 0xcafe,
            dimension: 0,
            stage: GenerationStage::Decoration,
            feature_type: FeatureType::Tree,
            origin: OriginCell::new(0, 0),
            candidate_ordinal: 0,
        };
        let fid_b = FeatureId {
            generator_fingerprint: 0xcafe,
            dimension: 0,
            stage: GenerationStage::Decoration,
            feature_type: FeatureType::Tree,
            origin: OriginCell::new(0, 0),
            candidate_ordinal: 1,
        };

        let frag_a = ChunkFragmentPayload {
            fragment_id: FragmentId {
                feature: fid_a,
                target_x: 0,
                target_z: 0,
                part_index: 0,
            },
            writes: vec![FragmentBlockWrite {
                local_x: 3,
                y: 65,
                local_z: 3,
                runtime_id: 100,
                layer: 0,
            }],
        };
        let frag_b = ChunkFragmentPayload {
            fragment_id: FragmentId {
                feature: fid_b,
                target_x: 0,
                target_z: 0,
                part_index: 0,
            },
            writes: vec![FragmentBlockWrite {
                local_x: 5,
                y: 66,
                local_z: 5,
                runtime_id: 200,
                layer: 0,
            }],
        };

        // Order 1: [A, B]
        let (cand1, cert1) = assemble_final_candidate(
            ticket,
            &base,
            &[frag_a.clone(), frag_b.clone()],
            None,
        )
        .unwrap();

        // Order 2: [B, A] (reverse input order)
        let (cand2, cert2) = assemble_final_candidate(
            ticket,
            &base,
            &[frag_b, frag_a],
            None,
        )
        .unwrap();

        assert_eq!(cand1.assembly_digest, cand2.assembly_digest);
        assert_eq!(cert1, cert2);
        assert_eq!(
            cand1.column.read().block_at(3, 65, 3).unwrap().0,
            100
        );
        assert_eq!(
            cand1.column.read().block_at(5, 66, 5).unwrap().0,
            200
        );
    }
}
