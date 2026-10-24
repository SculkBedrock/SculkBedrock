//! 集成测试：依赖闭包、封包记账、私有最终组装与区域所有者发布。
//!
//! Integration tests: Dependency Closure, Seal Bookkeeping, Private Final
//! Assembly, and Region Owner Publication.

use std::sync::Arc;

use sc_world::block_dictionary::air_runtime_id;
use sc_world::chunk::{BlockRuntimeId, Chunk, ChunkPosition};
use sc_world::generation::error::GenerationContractError;
use sc_world::generation::extent::builtin_registry;
use sc_world::generation::{
    BaseSnapshot, ChunkFragmentPayload, FeatureId, FeatureType,
    FragmentBlockWrite, FragmentId, GenerationControlState, GenerationStage, GenerationView,
    OriginCell, RegionOwner, RegionOwnerKey, RegionPublishEvent, SubmissionTicket,
    TargetChunkTracker, WorkerSubmission, DEFAULT_REGION_SIZE_CHUNKS,
};
use sc_world::leveldb::block_hash::block_state_hash;

use sc_worldgen::feature::{project_feature_plan, FeatureCandidate, TreeSolver};

fn make_test_base(
    cx: i32,
    cz: i32,
    surface_y: i32,
    surface_block: u32,
    fill_block: u32,
) -> Arc<BaseSnapshot> {
    let mut chunk = Chunk::empty(ChunkPosition::new(cx, cz), 0, -64, 319);
    for lx in 0..16 {
        for lz in 0..16 {
            chunk.set_block_at(
                0,
                lx,
                -64,
                lz,
                BlockRuntimeId(block_state_hash("minecraft:bedrock", None)),
            );
            for y in -63..surface_y {
                chunk.set_block_at(0, lx, y, lz, BlockRuntimeId(fill_block));
            }
            chunk.set_block_at(0, lx, surface_y, lz, BlockRuntimeId(surface_block));
        }
    }
    let height_map = [surface_y; 256];
    Arc::new(BaseSnapshot::new(
        0,
        ChunkPosition::new(cx, cz),
        1,
        0xcafe_1234,
        -64,
        319,
        Arc::new(chunk),
        height_map,
    ))
}

#[test]
fn full_w5_assembly_pipeline_with_needs_closure_and_region_owner() {
    let dirt = block_state_hash("minecraft:dirt", None);
    let stone = block_state_hash("minecraft:stone", None);
    let air = air_runtime_id();

    // 1. Query Needs(c) for target chunk (0, 0)
    let registry = builtin_registry();
    let target_origin = OriginCell::new(0, 0);
    let needs = registry.needs_for(target_origin).expect("needs_for query must succeed");
    assert!(needs.contains(&target_origin));

    // 2. Prepare snapshots for target (0,0) and neighbors
    let s00 = make_test_base(0, 0, 64, dirt, stone);
    let s10 = make_test_base(1, 0, 64, dirt, stone);
    let s01 = make_test_base(0, 1, 64, dirt, stone);
    let s11 = make_test_base(1, 1, 64, dirt, stone);

    let view = GenerationView::with_snapshots(
        0,
        1,
        0xcafe_1234,
        -64,
        319,
        BlockRuntimeId(air),
        vec![s00.clone(), s10, s01, s11],
    )
    .unwrap();

    // 3. Setup RegionOwner
    let owner_key = RegionOwnerKey::region_of(0, 0, 0, DEFAULT_REGION_SIZE_CHUNKS);
    let mut owner = RegionOwner::new(owner_key, DEFAULT_REGION_SIZE_CHUNKS, 128, 64 * 1024);
    owner.register_chunk(0, 0, 0xcafe_1234).unwrap();

    // Setup TargetChunkTracker
    let mut tracker = TargetChunkTracker::new(0, 0, 0, 128, 64 * 1024);
    tracker.set_base(s00.clone());

    // Initialize record base digest and expected seals
    let needed_origins: Vec<OriginCell> = needs.iter().copied().collect();
    {
        let record = owner.get_record_mut(0, 0).unwrap();
        record.base_digest = Some(s00.digest);
        record
            .register_expected_seals(GenerationStage::Structure, needed_origins.clone())
            .unwrap();
        record
            .register_expected_seals(GenerationStage::Decoration, needed_origins.clone())
            .unwrap();
    }

    // 4. Incomplete seals block assembly
    assert!(!tracker.is_ready_for_assembly(owner.get_record(0, 0).unwrap()));

    // 5. Generate and solve a tree candidate at (15, 15) in chunk (0,0)
    let candidate = FeatureCandidate {
        feature_id: FeatureId {
            generator_fingerprint: 0xcafe_1234,
            dimension: 0,
            stage: GenerationStage::Decoration,
            feature_type: FeatureType::Tree,
            origin: OriginCell::new(0, 0),
            candidate_ordinal: 0,
        },
        feature_type: FeatureType::Tree,
        anchor_x: 15,
        anchor_z: 15,
        variant: 1, // trunk height 5
    };
    let solver = TreeSolver::default();
    let plan = solver.solve(&candidate, &view).expect("tree solve must succeed");
    assert!(plan.is_accepted());

    let (fragments, manifest) = project_feature_plan(&plan);
    assert_eq!(manifest.fragment_count, 4);

    // Extract fragment targeting (0, 0)
    let frag_target_00 = fragments
        .iter()
        .find(|f| f.fragment_id.target_x == 0 && f.fragment_id.target_z == 0)
        .expect("must have fragment targeting (0, 0)")
        .clone();

    // 6. Seal structure stage (all origins have 0 structures for target 0,0)
    for origin in &needed_origins {
        owner
            .get_record_mut(0, 0)
            .unwrap()
            .apply_seal(
                GenerationStage::Structure,
                *origin,
                (0x1000i64 + origin.x as i64) as u64,
                0,
            )
            .unwrap();
    }
    assert!(!tracker.is_ready_for_assembly(owner.get_record(0, 0).unwrap()));

    // 7. Seal decoration stage:
    // Origin (0, 0) declares 1 fragment for target (0, 0)
    // All other origins declare 0 fragments
    for origin in &needed_origins {
        let declared = if *origin == OriginCell::new(0, 0) { 1 } else { 0 };
        owner
            .get_record_mut(0, 0)
            .unwrap()
            .apply_seal(
                GenerationStage::Decoration,
                *origin,
                (0x2000i64 + origin.x as i64) as u64,
                declared,
            )
            .unwrap();
    }

    // Seals are complete, but declared fragment count (1) != accepted fragments (0) -> not ready!
    assert!(!tracker.is_ready_for_assembly(owner.get_record(0, 0).unwrap()));

    // 8. Deliver the fragment
    tracker.push_fragment(frag_target_00).unwrap();
    owner.get_record_mut(0, 0).unwrap().accept_fragment(1).unwrap();

    // Now all conditions met!
    assert!(tracker.is_ready_for_assembly(owner.get_record(0, 0).unwrap()));

    // 9. Worker executes private final assembly
    let ticket = SubmissionTicket::new(owner_key, 1, 0, 0, 1);
    let (final_candidate, cert) = tracker.assemble(ticket).unwrap();

    assert_eq!(final_candidate.chunk_x, 0);
    assert_eq!(final_candidate.chunk_z, 0);
    assert!(cert.base_valid);
    assert!(cert.structure_seals_complete);
    assert!(cert.decoration_seals_complete);
    assert!(cert.all_fragments_verified);
    assert!(cert.assembly_validated);
    assert!(cert.metadata_included);

    // Verify trunk was written at local (15, 15) on private column
    let oak_log = block_state_hash("minecraft:oak_log", None);
    let col = final_candidate.column.read();
    assert_eq!(col.block_at(15, 65, 15).unwrap().0, oak_log);
    assert_eq!(col.block_at(15, 69, 15).unwrap().0, oak_log);

    // Heightmap was updated above 64
    assert!(col.highest_block_at(15, 15, BlockRuntimeId(air)).unwrap() >= 69);
    drop(col);

    // 10. Submit candidate and certificate to RegionOwner via Mailbox
    owner
        .mailbox
        .send(WorkerSubmission::FinalCandidatePrepared {
            ticket,
            candidate: final_candidate,
            certificate: cert,
        })
        .unwrap();

    let published = owner.process_mailbox().unwrap();
    assert_eq!(published.len(), 1);
    match &published[0] {
        RegionPublishEvent::ColumnPublished { chunk_x, chunk_z, .. } => {
            assert_eq!((*chunk_x, *chunk_z), (0, 0));
        }
        _ => panic!("expected ColumnPublished event"),
    }
    assert_eq!(
        owner.get_record(0, 0).unwrap().state,
        GenerationControlState::Ready
    );

    // RegionOwner now holds the published column
    assert!(owner.published_columns.contains_key(&(0, 0)));
    let published_col = owner.published_columns.get(&(0, 0)).unwrap().read();
    assert_eq!(published_col.block_at(15, 65, 15).unwrap().0, oak_log);
}

#[test]
fn out_of_order_fragments_assemble_to_identical_digest() {
    let base = make_test_base(0, 0, 64, 10, 20);
    let owner_key = RegionOwnerKey::region_of(0, 0, 0, 4);
    let ticket = SubmissionTicket::new(owner_key, 1, 0, 0, 1);

    let fid_a = FeatureId {
        generator_fingerprint: 0xcafe,
        dimension: 0,
        stage: GenerationStage::Structure,
        feature_type: FeatureType::DesertWell,
        origin: OriginCell::new(0, 0),
        candidate_ordinal: 0,
    };
    let fid_b = FeatureId {
        generator_fingerprint: 0xcafe,
        dimension: 0,
        stage: GenerationStage::Decoration,
        feature_type: FeatureType::Tree,
        origin: OriginCell::new(0, 0),
        candidate_ordinal: 0,
    };
    let fid_c = FeatureId {
        generator_fingerprint: 0xcafe,
        dimension: 0,
        stage: GenerationStage::Decoration,
        feature_type: FeatureType::Tree,
        origin: OriginCell::new(1, 0),
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
            local_x: 2,
            y: 65,
            local_z: 2,
            runtime_id: 101,
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
            runtime_id: 202,
            layer: 0,
        }],
    };
    let frag_c = ChunkFragmentPayload {
        fragment_id: FragmentId {
            feature: fid_c,
            target_x: 0,
            target_z: 0,
            part_index: 0,
        },
        writes: vec![FragmentBlockWrite {
            local_x: 8,
            y: 67,
            local_z: 8,
            runtime_id: 303,
            layer: 0,
        }],
    };

    // Tracker 1 receives: [A, B, C]
    let mut tracker1 = TargetChunkTracker::new(0, 0, 0, 16, 4096);
    tracker1.set_base(base.clone());
    tracker1.push_fragment(frag_a.clone()).unwrap();
    tracker1.push_fragment(frag_b.clone()).unwrap();
    tracker1.push_fragment(frag_c.clone()).unwrap();
    let (cand1, cert1) = tracker1.assemble(ticket).unwrap();

    // Tracker 2 receives in reverse order: [C, B, A]
    let mut tracker2 = TargetChunkTracker::new(0, 0, 0, 16, 4096);
    tracker2.set_base(base);
    tracker2.push_fragment(frag_c).unwrap();
    tracker2.push_fragment(frag_b).unwrap();
    tracker2.push_fragment(frag_a).unwrap();
    let (cand2, cert2) = tracker2.assemble(ticket).unwrap();

    // Verifications:
    // 1. Bit-for-bit identical assembly digest
    assert_eq!(cand1.assembly_digest, cand2.assembly_digest);
    assert_eq!(cert1, cert2);

    // 2. Exact same block values
    let col1 = cand1.column.read();
    let col2 = cand2.column.read();
    assert_eq!(col1.block_at(2, 65, 2), col2.block_at(2, 65, 2));
    assert_eq!(col1.block_at(5, 66, 5), col2.block_at(5, 66, 5));
    assert_eq!(col1.block_at(8, 67, 8), col2.block_at(8, 67, 8));
}

#[test]
fn owner_fences_stale_ticket_attempt() {
    let base = make_test_base(0, 0, 64, 10, 20);
    let owner_key = RegionOwnerKey::region_of(0, 0, 0, 4);
    let mut owner = RegionOwner::new(owner_key, 4, 16, 4096);
    owner.register_chunk(0, 0, 0xcafe_1234).unwrap();

    let mut tracker = TargetChunkTracker::new(0, 0, 0, 16, 4096);
    tracker.set_base(base.clone());

    // Worker assembled ticket with attempt = 1
    let ticket = SubmissionTicket::new(owner_key, 1, 0, 0, 1);
    let (cand, cert) = tracker.assemble(ticket).unwrap();

    // But owner retry occurred and record advanced attempt to 2!
    owner.get_record_mut(0, 0).unwrap().begin_retry();

    // Worker submits stale attempt = 1
    owner
        .mailbox
        .send(WorkerSubmission::FinalCandidatePrepared {
            ticket,
            candidate: cand,
            certificate: cert,
        })
        .unwrap();

    let err = owner.process_mailbox().unwrap_err();

    assert!(matches!(
        err,
        GenerationContractError::StaleAttempt {
            key_x: 0,
            key_z: 0,
            expected: 2,
            received: 1
        }
    ));
    assert!(owner.published_columns.is_empty());
}
