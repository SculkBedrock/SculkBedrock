//! 集成测试：特性规划、求解器、边界投射与缺失依赖强制执行。
//!
//! Integration tests: Feature Planning, Solvers, Boundary Projection,
//! and Missing-Dependency Enforcement.

use std::sync::Arc;

use sc_world::block_dictionary::air_runtime_id;
use sc_world::chunk::{BlockRuntimeId, Chunk, ChunkPosition};
use sc_world::generation::error::GenerationContractError;
use sc_world::generation::identity::{
    FeatureId, FeatureType, GenerationStage, OriginCell,
};
use sc_world::generation::view::{BaseSnapshot, GenerationView};
use sc_world::leveldb::block_hash::block_state_hash;

use sc_worldgen::feature::{
    project_feature_plan, DesertWellSolver, FeatureCandidate, TreeSolver,
};

fn make_test_snapshot(
    cx: i32,
    cz: i32,
    surface_y: i32,
    surface_block: u32,
    fill_block: u32,
) -> Arc<BaseSnapshot> {
    let mut chunk = Chunk::empty(ChunkPosition::new(cx, cz), 0, -64, 319);

    // Fill bedrock at -64 and fill_block from -63 up to surface_y - 1
    for lx in 0..16 {
        for lz in 0..16 {
            chunk.set_block_at(0, lx, -64, lz, BlockRuntimeId(block_state_hash("minecraft:bedrock", None)));
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
fn corner_tree_projects_deterministically_into_four_chunks() {
    let dirt = block_state_hash("minecraft:dirt", None);
    let stone = block_state_hash("minecraft:stone", None);
    let air = air_runtime_id();

    // Prepare a 2x2 neighborhood: (0,0), (1,0), (0,1), (1,1)
    let s00 = make_test_snapshot(0, 0, 64, dirt, stone);
    let s10 = make_test_snapshot(1, 0, 64, dirt, stone);
    let s01 = make_test_snapshot(0, 1, 64, dirt, stone);
    let s11 = make_test_snapshot(1, 1, 64, dirt, stone);

    let view = GenerationView::with_snapshots(
        0,
        1,
        0xcafe_1234,
        -64,
        319,
        BlockRuntimeId(air),
        vec![s00, s10, s01, s11],
    )
    .unwrap();

    // Candidate anchored at (15, 15) in chunk (0,0) - directly at the corner!
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
    assert!(plan.is_accepted(), "tree should be accepted on dirt ground");

    let (fragments, manifest) = project_feature_plan(&plan);

    // Tree crown with radius 2 at (15, 15) spans x in 13..=17 and z in 13..=17
    // This touches all 4 chunks: (0,0), (1,0), (0,1), (1,1)!
    assert_eq!(fragments.len(), 4);
    assert_eq!(manifest.fragment_count, 4);
    assert_eq!(
        manifest.target_chunks,
        vec![(0, 0), (0, 1), (1, 0), (1, 1)]
    );

    // Verify all fragment writes are in chunk-local [0, 16) bounds
    for frag in &fragments {
        for w in &frag.writes {
            assert!(w.local_x < 16);
            assert!(w.local_z < 16);
            assert!(w.y >= 65);
        }
    }
}

#[test]
fn obstacle_in_neighbor_chunk_rejects_tree_deterministically() {
    let dirt = block_state_hash("minecraft:dirt", None);
    let stone = block_state_hash("minecraft:stone", None);
    let air = air_runtime_id();

    let s00 = make_test_snapshot(0, 0, 64, dirt, stone);
    let s10 = make_test_snapshot(1, 0, 64, dirt, stone);
    let s01 = make_test_snapshot(0, 1, 64, dirt, stone);

    // In (1,1), place a stone cliff at world (16, 66, 16) - where leaves would grow
    let mut chunk11 = Chunk::empty(ChunkPosition::new(1, 1), 0, -64, 319);
    for lx in 0..16 {
        for lz in 0..16 {
            chunk11.set_block_at(0, lx, 64, lz, BlockRuntimeId(dirt));
        }
    }
    // Place obstacle cliff block at local (0, 67, 0) in (1, 1) -> world (16, 67, 16)
    chunk11.set_block_at(0, 0, 67, 0, BlockRuntimeId(stone));

    let s11 = Arc::new(BaseSnapshot::new(
        0,
        ChunkPosition::new(1, 1),
        1,
        0xcafe_1234,
        -64,
        319,
        Arc::new(chunk11),
        [64; 256],
    ));

    let view = GenerationView::with_snapshots(
        0,
        1,
        0xcafe_1234,
        -64,
        319,
        BlockRuntimeId(air),
        vec![s00, s10, s01, s11],
    )
    .unwrap();

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
        variant: 1,
    };

    let solver = TreeSolver::default();
    let plan = solver.solve(&candidate, &view).expect("tree solve must complete");
    assert!(!plan.is_accepted(), "tree should be rejected due to stone obstacle in neighbor chunk");

    let (fragments, manifest) = project_feature_plan(&plan);
    assert!(fragments.is_empty(), "rejected tree must produce 0 fragments");
    assert_eq!(manifest.fragment_count, 0);
}

#[test]
fn missing_neighbor_snapshot_propagates_missing_dependency_error() {
    let dirt = block_state_hash("minecraft:dirt", None);
    let stone = block_state_hash("minecraft:stone", None);
    let air = air_runtime_id();

    // Only provide chunk (0,0); neighbor chunks (1,0), (0,1), (1,1) are MISSING
    let s00 = make_test_snapshot(0, 0, 64, dirt, stone);

    let view = GenerationView::with_snapshots(
        0,
        1,
        0xcafe_1234,
        -64,
        319,
        BlockRuntimeId(air),
        vec![s00],
    )
    .unwrap();

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
        variant: 1,
    };

    let solver = TreeSolver::default();
    let err = solver.solve(&candidate, &view).unwrap_err();
    // Missing neighbor chunk must NOT be disguised as air!
    assert!(matches!(
        err,
        GenerationContractError::MissingDependency { chunk_x: 1, chunk_z: 1, stage: "base", .. }
            | GenerationContractError::MissingDependency { chunk_x: 1, chunk_z: 0, stage: "base", .. }
            | GenerationContractError::MissingDependency { chunk_x: 0, chunk_z: 1, stage: "base", .. }
    ));
}

#[test]
fn desert_well_solves_on_sand_and_rejects_on_dirt() {
    let sand = block_state_hash("minecraft:sand", None);
    let dirt = block_state_hash("minecraft:dirt", None);
    let stone = block_state_hash("minecraft:stone", None);
    let air = air_runtime_id();

    // Sand snapshot
    let s_sand = make_test_snapshot(0, 0, 64, sand, sandstone_hash());
    let view_sand = GenerationView::with_snapshots(
        0,
        1,
        0xcafe_1234,
        -64,
        319,
        BlockRuntimeId(air),
        vec![s_sand],
    )
    .unwrap();

    let candidate = FeatureCandidate {
        feature_id: FeatureId {
            generator_fingerprint: 0xcafe_1234,
            dimension: 0,
            stage: GenerationStage::Structure,
            feature_type: FeatureType::DesertWell,
            origin: OriginCell::new(0, 0),
            candidate_ordinal: 0,
        },
        feature_type: FeatureType::DesertWell,
        anchor_x: 8,
        anchor_z: 8,
        variant: 0,
    };

    let solver = DesertWellSolver::default();
    let plan = solver.solve(&candidate, &view_sand).unwrap();
    assert!(plan.is_accepted());

    // Dirt snapshot
    let s_dirt = make_test_snapshot(0, 0, 64, dirt, stone);
    let view_dirt = GenerationView::with_snapshots(
        0,
        1,
        0xcafe_1234,
        -64,
        319,
        BlockRuntimeId(air),
        vec![s_dirt],
    )
    .unwrap();

    let plan_dirt = solver.solve(&candidate, &view_dirt).unwrap();
    assert!(!plan_dirt.is_accepted());
}

fn sandstone_hash() -> u32 {
    block_state_hash("minecraft:sandstone", None)
}
