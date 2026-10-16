use super::{ChunkKey, MAX_TRACKED_SPILLOVER_CELLS, SpilloverOperationId, WorldStorageError};
use crate::chunk::Chunk;
use sc_log::t_log;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

/// Result of `generate_chunk_with_spillover`: the chunk itself plus cross-chunk spillover writes.
pub struct GeneratedChunk {
    pub chunk: Chunk,
    /// Spillover writes (`key` is the target chunk; `x/y/z` are world coordinates).
    pub spillover: Vec<BlockSpilloverWrite>,
}

/// Count/byte admission for a worker that is about to construct one generated
/// chunk's bounded spillover result.
pub(crate) struct SpilloverGenerationPermit {
    pub(crate) reserved_bytes: Arc<AtomicUsize>,
    pub(crate) bytes: usize,
}

impl Drop for SpilloverGenerationPermit {
    fn drop(&mut self) {
        self.reserved_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// One cross-chunk spillover block write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockSpilloverWrite {
    /// Target chunk (includes dimension).
    pub key: ChunkKey,
    /// World coordinates.
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Block layer (0 = main layer, 1 = extra layer).
    pub layer: usize,
    /// Block runtime id.
    pub block: crate::chunk::BlockRuntimeId,
}

/// A spillover write together with its durable, retry-stable operation id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpilloverJournalEntry {
    pub operation_id: SpilloverOperationId,
    pub write: BlockSpilloverWrite,
}

pub struct GeneratedChunkCommit {
    pub source: Chunk,
    pub incoming: Vec<SpilloverJournalEntry>,
    pub newly_committed: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ChunkGenerationRequest {
    pub key: ChunkKey,
    pub min_y: i32,
    pub max_y: i32,
}
/// Rate-limited diagnostic for spillover arbitration outcomes.
///
/// `superseded` is a defined generation conflict policy (largest operation id
/// wins), not data loss. `deferred` means the column's bounded arbitration window
/// was full: those writes are re-queued and applied in operation-id order when
/// the column is next materialized, so no position is ever resolved by arrival
/// order.
pub(crate) fn report_spillover_arbitration(target: Option<ChunkKey>, superseded: u64, untracked: u64) {
    static TOTAL_SUPERSEDED: AtomicU64 = AtomicU64::new(0);
    static TOTAL_UNTRACKED: AtomicU64 = AtomicU64::new(0);
    static UNTRACKED_WARNED: AtomicBool = AtomicBool::new(false);
    let total_superseded = TOTAL_SUPERSEDED.fetch_add(superseded, Ordering::Relaxed) + superseded;
    let total_untracked = TOTAL_UNTRACKED.fetch_add(untracked, Ordering::Relaxed) + untracked;
    if total_untracked > 0 && !UNTRACKED_WARNED.swap(true, Ordering::AcqRel) {
        log::warn!(
            "{}",
            t_log!(
                "console.world.spillover_window",
                max = MAX_TRACKED_SPILLOVER_CELLS,
                deferred = total_untracked,
                target = format!("{target:?}")
            )
        );
    }
    if superseded > 0 {
        log::debug!(
            "[spillover] {} generation write(s) superseded by a higher-priority source \
             (target={target:?}, total_superseded={total_superseded})",
            superseded
        );
    }
}

pub(crate) fn merge_spillover_entries(
    entries: impl IntoIterator<Item = SpilloverJournalEntry>,
) -> Result<Vec<SpilloverJournalEntry>, WorldStorageError> {
    let mut by_operation = HashMap::new();
    for entry in entries {
        if let Some(existing) = by_operation.get(&entry.operation_id) {
            if existing != &entry {
                return Err(WorldStorageError::corrupt(format!(
                    "spillover operation id reused with different payload: {:?}",
                    entry.operation_id
                )));
            }
            continue;
        }
        by_operation.insert(entry.operation_id, entry);
    }
    let mut entries = by_operation.into_values().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.operation_id);
    Ok(entries)
}
