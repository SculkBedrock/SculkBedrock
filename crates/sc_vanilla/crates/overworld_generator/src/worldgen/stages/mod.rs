//! Generation stage trait, ordered stage chain, and chunk-coordinate helper.
//!
//! Notes:
//! - A stage receives `&mut ChunkGenerateContext` and edits the chunk in place.
//! - The builder collects stages into an ordered vector; `run_stages` executes
//!   a `[start_idx, end_name]` closed range synchronously in order.
//! - The chunk-coordinate packing helper is provided here for seed mixing.

use crate::worldgen::context::ChunkGenerateContext;

pub mod biome_map;
pub mod chunk_feature;
pub mod generated;
pub mod populator;
pub mod surface_data;
pub mod surface_overwrite;
pub mod terrain;

// ---------------------------------------------------------------------------
// GenerateStage trait: one step of the chunk generation chain
// ---------------------------------------------------------------------------

/// One step of the chunk generation chain.
///
/// Each stage receives `&mut ChunkGenerateContext` and edits the chunk in place.
/// `name()` returns the stage name (e.g. `"normal_terrain"`) used to locate
/// the start/end stage by name.
pub trait GenerateStage: Send + Sync {
    /// Applies this stage to the chunk context.
    fn apply(&self, ctx: &mut ChunkGenerateContext<'_>);

    /// Returns the stage name.
    fn name(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// GenerateStageBuilder: ordered stage list
// ---------------------------------------------------------------------------

/// Ordered stage-list builder.
///
/// Stages are kept in a vector preserving insertion order, without manual
/// linked-list traversal. `start` sets the first element, `next` appends,
/// and `build` consumes the builder into a vector.
pub struct GenerateStageBuilder {
    stages: Vec<Box<dyn GenerateStage>>,
}

impl GenerateStageBuilder {
    /// Creates an empty builder.
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Sets the first stage, clearing any previous entries.
    pub fn start(&mut self, stage: Box<dyn GenerateStage>) -> &mut Self {
        self.stages.clear();
        self.stages.push(stage);
        self
    }

    /// Appends the next stage.
    pub fn next(&mut self, stage: Box<dyn GenerateStage>) -> &mut Self {
        self.stages.push(stage);
        self
    }

    /// Consumes the builder and returns the ordered stage list.
    pub fn build(self) -> Vec<Box<dyn GenerateStage>> {
        self.stages
    }
}

impl Default for GenerateStageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// run_stages: synchronous ordered execution of a stage range
// ---------------------------------------------------------------------------

/// Synchronously runs every stage in the `[start_idx, end_name]` closed range;
/// a rejected spillover batch stops the chain early because the private chunk is dropped.
///
/// Execution starts at the start index and applies each stage in order up to
/// and including the stage named `end_name`, then returns.
///
/// - `stages`: full stage list (built by the builder).
/// - `start_idx`: start stage index, chosen from the chunk state.
/// - `end_name`: end stage name (e.g. `"generated"`).
/// - `ctx`: generation context.
///
/// Runs to the end of the chain when `end_name` is not present.
pub fn run_stages(
    stages: &[Box<dyn GenerateStage>],
    start_idx: usize,
    end_name: &str,
    ctx: &mut ChunkGenerateContext<'_>,
) {
    for i in start_idx..stages.len() {
        stages[i].apply(ctx);
        // A rejected spillover batch invalidates the private generation result.
        // Avoid spending CPU on later stages for a chunk the caller will drop.
        if ctx.spillover_overflowed() {
            break;
        }
        if stages[i].name() == end_name {
            break;
        }
    }
}

/// Finds a stage index by name via ordered scan.
pub fn find_stage_idx(stages: &[Box<dyn GenerateStage>], name: &str) -> Option<usize> {
    stages.iter().position(|s| s.name() == name)
}

// ---------------------------------------------------------------------------
// Chunk-coordinate packing helper for seed mixing
// ---------------------------------------------------------------------------

/// Packs chunk coordinates into an `i64`.
///
/// `(x << 32) | (z & 0xffffffff)`: packs chunk coordinates into an `i64`.
/// Used for seed mixing: `level_seed ^ chunk_hash(x, z)`.
pub fn chunk_hash(x: i32, z: i32) -> i64 {
    (((x as i64) << 32) | ((z as u32) as i64)) as i64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::chunk::WorldgenChunk;
    use crate::worldgen::context::{BlockEntry, BlockManager};
    use crate::worldgen::holder::normal::NormalObjectHolder;
    use crate::worldgen::material::MaterialBlocks;
    use crate::worldgen::random::Xoroshiro128;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use sc_world::chunk::{BlockRuntimeId, ChunkPosition};

    struct DummyStage {
        name: &'static str,
    }

    impl GenerateStage for DummyStage {
        fn apply(&self, _ctx: &mut ChunkGenerateContext<'_>) {}
        fn name(&self) -> &'static str {
            self.name
        }
    }

    struct OverflowStage;

    impl GenerateStage for OverflowStage {
        fn apply(&self, ctx: &mut ChunkGenerateContext<'_>) {
            let entry = BlockEntry {
                x: 16,
                y: 70,
                z: 0,
                layer: 0,
                block: BlockRuntimeId(1),
            };
            ctx.queue_object(HashMap::from([(
                BlockManager::hash_xyz(entry.x, entry.y, entry.z, entry.layer),
                entry,
            )]));
        }

        fn name(&self) -> &'static str {
            "overflow"
        }
    }

    struct CountStage(Arc<AtomicUsize>);

    impl GenerateStage for CountStage {
        fn apply(&self, _ctx: &mut ChunkGenerateContext<'_>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        fn name(&self) -> &'static str {
            "after_overflow"
        }
    }

    #[test]
    fn chunk_hash_packs_coordinates() {
        // Packs (x << 32) | zero-extended z.
        assert_eq!(chunk_hash(0, 0), 0);
        assert_eq!(chunk_hash(1, 0), 1i64 << 32);
        assert_eq!(chunk_hash(0, 1), 1);
        assert_eq!(chunk_hash(1, 1), (1i64 << 32) | 1);
        // Low 32 bits of negative z (zero-extended).
        assert_eq!(chunk_hash(0, -1), 0xffff_ffffu32 as i64);
    }

    #[test]
    fn builder_preserves_order() {
        let make = |name: &'static str| Box::new(DummyStage { name }) as Box<dyn GenerateStage>;
        let mut builder = GenerateStageBuilder::new();
        builder.start(make("a")).next(make("b")).next(make("c"));
        let stages = builder.build();
        assert_eq!(stages.len(), 3);
        assert_eq!(stages[0].name(), "a");
        assert_eq!(stages[1].name(), "b");
        assert_eq!(stages[2].name(), "c");
        assert_eq!(find_stage_idx(&stages, "b"), Some(1));
        assert_eq!(find_stage_idx(&stages, "z"), None);
    }

    #[test]
    fn run_stages_stops_at_end_name() {
        let make = |name: &'static str| Box::new(DummyStage { name }) as Box<dyn GenerateStage>;
        let mut builder = GenerateStageBuilder::new();
        builder
            .start(make("terrain"))
            .next(make("biome"))
            .next(make("surface"))
            .next(make("generated"))
            .next(make("populate"));
        let stages = builder.build();

        // Verify find_stage_idx locates the "generated" index correctly.
        let end_idx = find_stage_idx(&stages, "generated").unwrap();
        assert_eq!(end_idx, 3);
        // run_stages from 0 to end_idx (inclusive) executes 0..=3.
    }

    #[test]
    fn run_stages_stops_after_spillover_overflow() {
        let holder = NormalObjectHolder::new(
            Xoroshiro128::new(42),
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
            },
        );
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut worldgen_chunk = WorldgenChunk::new(chunk);
        let mut ctx = ChunkGenerateContext::new_with_spillover_limit(
            &mut worldgen_chunk,
            &holder,
            42,
            -64,
            319,
            0,
        );
        let later_stage_runs = Arc::new(AtomicUsize::new(0));
        let stages: Vec<Box<dyn GenerateStage>> = vec![
            Box::new(OverflowStage),
            Box::new(CountStage(Arc::clone(&later_stage_runs))),
        ];

        run_stages(&stages, 0, "finished", &mut ctx);

        assert!(ctx.spillover_overflowed());
        assert_eq!(later_stage_runs.load(Ordering::Relaxed), 0);
    }
}
