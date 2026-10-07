use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak,
};
use std::time::{Duration, Instant};

use crate::chunk::{Chunk, ChunkPosition, SUBCHUNK_SIZE};
use sc_log::t_log;

const SPILLOVER_LOCK_STRIPES: usize = 64;
const MAX_PENDING_SPILLOVER_WRITES: usize = 65_536;
/// Conservative queue-accounting allowance for one fixed-size write, Vec
/// growth, per-key metadata/timestamp, and its map share. It is an admission
/// estimate, not RSS.
const SPILLOVER_ESTIMATED_BYTES_PER_WRITE: usize = 256;
const MAX_PENDING_SPILLOVER_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_STORED_SPILLOVER_WRITES: usize = 262_144;
pub const MAX_GENERATED_SPILLOVER_WRITES: usize =
    MAX_PENDING_SPILLOVER_BYTES / SPILLOVER_ESTIMATED_BYTES_PER_WRITE;
const MAX_IN_FLIGHT_SPILLOVER_BYTES: usize = 8 * 1024 * 1024;
const MAX_RETIRED_COLUMN_HANDLES: usize = 128;
const MAX_SYNC_FLUSH_BATCH_COLUMNS: usize = 32;
const MAX_SYNC_FLUSH_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
/// Estimated dirty bytes above which new generation is throttled (§3.2).
///
/// Unacknowledged dirty data is the only world state that cannot be recreated
/// from a seed, so it gets a dedicated budget: past the high-water mark the
/// provider refuses *new generation* (recoverable work) instead of accepting
/// more, while gameplay edits keep their existing admission path.
const DIRTY_HIGH_WATER_BYTES: usize = 192 * 1024 * 1024;
/// Dirty budget after which only already-resident columns may be loaded.
const DIRTY_CRITICAL_BYTES: usize = 320 * 1024 * 1024;
const MAX_DIRTY_FLUSH_KEY_SAMPLES: usize = 128;

/// A stable key shared by all world storage backends.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChunkKey {
    pub dimension: i32,
    pub position: ChunkPosition,
}

impl ChunkKey {
    pub const fn new(dimension: i32, position: ChunkPosition) -> Self {
        Self {
            dimension,
            position,
        }
    }
}

/// Stable identity for one generated spillover fact.
///
/// The source column and deterministic result ordinal remain stable when a
/// load job is retried.  The ordinal is assigned before routing and is part of
/// the persistence key, so a replay cannot create a second copy of the same
/// accepted fact.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SpilloverOperationId {
    pub source: ChunkKey,
    pub ordinal: u32,
}

impl SpilloverOperationId {
    pub const fn new(source: ChunkKey, ordinal: u32) -> Self {
        Self { source, ordinal }
    }
}

/// Outcome of a dirty-column flush, including what the shutdown path needs to
/// report when persistence is incomplete or its dirty set cannot be inspected.
#[derive(Debug, Default)]
pub struct DirtyFlushReport {
    pub attempted: usize,
    pub saved: usize,
    /// Bounded samples of failed keys; `failed_count` is the exact total.
    pub failed_keys: Vec<ChunkKey>,
    /// Bounded samples of keys changed during persistence; exact total is in
    /// `changed_during_save_count`.
    pub changed_during_save: Vec<ChunkKey>,
    pub failed_count: usize,
    /// Snapshots rejected by the single-column shutdown byte limit.
    pub oversize_count: usize,
    pub changed_during_save_count: usize,
    /// True when either key sample exceeded its fixed capacity.
    pub key_samples_truncated: bool,
    /// `None` means the provider could not inspect the remaining dirty set.
    pub remaining_dirty: Option<usize>,
    pub scan_failed: bool,
}

impl DirtyFlushReport {
    fn record_failure(&mut self, key: ChunkKey) {
        self.failed_count += 1;
        if self.failed_keys.len() < MAX_DIRTY_FLUSH_KEY_SAMPLES {
            self.failed_keys.push(key);
        } else {
            self.key_samples_truncated = true;
        }
    }

    fn record_changed(&mut self, key: ChunkKey) {
        self.changed_during_save_count += 1;
        if self.changed_during_save.len() < MAX_DIRTY_FLUSH_KEY_SAMPLES {
            self.changed_during_save.push(key);
        } else {
            self.key_samples_truncated = true;
        }
    }

    fn record_oversize(&mut self, key: ChunkKey) {
        self.oversize_count += 1;
        self.record_failure(key);
    }

    pub fn is_complete(&self) -> bool {
        !self.scan_failed
            && self.failed_count == 0
            && self.changed_during_save_count == 0
            && self.remaining_dirty == Some(0)
    }
}

#[derive(Debug)]
pub enum WorldStorageError {
    Io(std::io::Error),
    Corrupt(String),
    Unsupported(String),
    /// Provider admission was rejected without publishing or dropping dirty data.
    Capacity(String),
    Backend(String),
}

impl Display for WorldStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "world storage I/O error: {error}"),
            Self::Corrupt(message) => write!(formatter, "corrupt world storage: {message}"),
            Self::Unsupported(message) => write!(formatter, "unsupported world storage: {message}"),
            Self::Capacity(message) => {
                write!(formatter, "world capacity admission rejected: {message}")
            }
            Self::Backend(message) => write!(formatter, "world storage backend error: {message}"),
        }
    }
}

impl Error for WorldStorageError {}

impl From<std::io::Error> for WorldStorageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Reads chunks already present in a saved world.
///
/// The LevelDB implementation will be added behind this boundary. Network
/// and ECS code must not depend on LevelDB key layouts or compression details.
pub trait WorldStorage: Send + Sync {
    fn load_chunk(&self, key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError>;

    /// Journal-enabled backends must read contents and outstanding facts at
    /// one storage boundary; two independent reads can replay a saved fact.
    fn load_chunk_with_spillover(
        &self,
        key: ChunkKey,
    ) -> Result<(Option<Chunk>, Vec<SpilloverJournalEntry>), WorldStorageError> {
        if self.supports_spillover_journal() {
            return Err(WorldStorageError::Unsupported(
                "atomic chunk/journal read is required by this backend".into(),
            ));
        }
        Ok((self.load_chunk(key)?, Vec::new()))
    }

    /// Persist a previously absent generation source and all of its outgoing
    /// facts atomically. A duplicate returns the stored source without routing
    /// this attempt's outgoing facts again.
    fn commit_generated_chunk(
        &self,
        _key: ChunkKey,
        _chunk: Chunk,
        _outgoing: &[SpilloverJournalEntry],
    ) -> Result<GeneratedChunkCommit, WorldStorageError> {
        Err(WorldStorageError::Unsupported(
            "atomic generation/journal commit is required by this backend".into(),
        ))
    }

    /// Whether this backend has the versioned spillover journal namespace.
    /// Backends without it retain the bounded in-memory compatibility path.
    fn supports_spillover_journal(&self) -> bool {
        false
    }

    /// Load accepted spillover facts that have not yet been covered by a
    /// successful target-column save.
    fn load_spillover_journal(
        &self,
        _key: ChunkKey,
    ) -> Result<Vec<SpilloverJournalEntry>, WorldStorageError> {
        Ok(Vec::new())
    }

    /// Atomically append a batch of spillover facts. Implementations must be
    /// idempotent by operation id and reject a conflicting reuse of an id.
    fn append_spillover_journal(
        &self,
        entries: &[SpilloverJournalEntry],
    ) -> Result<(), WorldStorageError> {
        if entries.is_empty() {
            Ok(())
        } else {
            Err(WorldStorageError::Unsupported(
                "spillover journal not supported by this backend".to_string(),
            ))
        }
    }

    /// Writes a chunk (blocks/block entities) back to the archive.
    /// The default impl does not support it (memory/generator-only backends may leave it empty).
    fn save_chunk(&self, _key: ChunkKey, _chunk: &Chunk) -> Result<(), WorldStorageError> {
        Err(WorldStorageError::Unsupported(
            "save_chunk not supported by this backend".to_string(),
        ))
    }

    /// Persist an owned snapshot. Backends with an owned request path should
    /// override this to transfer the snapshot without cloning it again. The
    /// borrowed compatibility method remains available to existing callers
    /// and backend implementations.
    fn save_chunk_owned(&self, key: ChunkKey, chunk: Chunk) -> Result<(), WorldStorageError> {
        self.save_chunk(key, &chunk)
    }

    /// Persist a full chunk snapshot while declaring whether its block-entity
    /// record is authoritative for this save. Legacy backends that do not
    /// distinguish metadata dirty state retain their existing owned-save
    /// behavior; Bedrock LevelDB overrides this to preserve unreadable or
    /// unchanged NBT records and to delete records after explicit removal.
    fn save_chunk_owned_with_metadata(
        &self,
        key: ChunkKey,
        chunk: Chunk,
        _block_entities_dirty: bool,
        _biomes_dirty: bool,
        _heightmap_dirty: bool,
    ) -> Result<(), WorldStorageError> {
        self.save_chunk_owned(key, chunk)
    }

    /// Persist a snapshot and acknowledge the journal facts already included
    /// in that snapshot. Bedrock storage overrides this so the data save and
    /// journal cleanup share one WriteBatch.
    fn save_chunk_owned_with_metadata_and_spillover(
        &self,
        key: ChunkKey,
        chunk: Chunk,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
        _spillover_acks: &[SpilloverOperationId],
    ) -> Result<(), WorldStorageError> {
        self.save_chunk_owned_with_metadata(
            key,
            chunk,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
        )
    }
}

/// Optional plugin-owned world generation hook.
pub trait WorldGenerator: Send + Sync {
    fn generate_chunk(
        &self,
        request: ChunkGenerationRequest,
    ) -> Result<Option<Chunk>, WorldStorageError>;

    /// Generates a chunk and returns **cross-chunk spillover writes** (blocks of
    /// a feature/structure that cross the chunk border, e.g. the canopy of an
    /// edge tree).
    ///
    /// [`WorldChunkProvider`] applies spillover writes immediately to cached
    /// neighbors, or queues them in a pending queue until the target chunk is
    /// generated/loaded.
    ///
    /// The default impl wraps [`WorldGenerator::generate_chunk`] and returns
    /// empty spillover (generators without cross-chunk features need no override).
    fn generate_chunk_with_spillover(
        &self,
        request: ChunkGenerationRequest,
    ) -> Result<Option<GeneratedChunk>, WorldStorageError> {
        Ok(self.generate_chunk(request)?.map(|chunk| GeneratedChunk {
            chunk,
            spillover: Vec::new(),
        }))
    }

    /// Generate with a caller-provided spillover result limit. Legacy/plugin
    /// generators remain source-compatible through this default post-check;
    /// generators with internal spillover accumulators should override it to
    /// enforce the limit while collecting, before growing an unbounded Vec.
    fn generate_chunk_with_spillover_bounded(
        &self,
        request: ChunkGenerationRequest,
        max_spillover_writes: usize,
    ) -> Result<Option<GeneratedChunk>, WorldStorageError> {
        let generated = self.generate_chunk_with_spillover(request)?;
        if generated
            .as_ref()
            .is_some_and(|result| result.spillover.len() > max_spillover_writes)
        {
            return Err(WorldStorageError::Capacity(format!(
                "generator spillover result exceeds the {max_spillover_writes}-write bound"
            )));
        }
        Ok(generated)
    }
}

/// Result of `generate_chunk_with_spillover`: the chunk itself plus cross-chunk spillover writes.
pub struct GeneratedChunk {
    pub chunk: Chunk,
    /// Spillover writes (`key` is the target chunk; `x/y/z` are world coordinates).
    pub spillover: Vec<BlockSpilloverWrite>,
}

/// Count/byte admission for a worker that is about to construct one generated
/// chunk's bounded spillover result.
struct SpilloverGenerationPermit {
    reserved_bytes: Arc<AtomicUsize>,
    bytes: usize,
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

/// A mutable, shared chunk column: the unit the cache hands out.
///
/// Readers (network encode, get_block) take the column-level read lock;
/// the single-writer apply system takes the write lock. Per-subchunk dirty
/// indices feed incremental LevelDB write-back (design doc §4 / B7).
pub struct ChunkColumn {
    chunk: RwLock<Chunk>,
    incarnation: u128,
    dirty_subchunks: Mutex<BTreeSet<i8>>,
    applied_spillover_operations: Mutex<BTreeSet<SpilloverOperationId>>,
    /// Per-block arbitration state for cross-column generation writes.
    ///
    /// Two generators may target the same block position.
    /// `SpilloverOperationId` is a total order, so "largest id wins" makes the
    /// published result independent of which worker finished first. Entries
    /// below the recorded winner are *superseded generation results*, not
    /// dropped world facts: they are counted and diagnosed.
    spillover_conflicts: Mutex<SpilloverConflictState>,
    block_entities_dirty: AtomicBool,
    biomes_dirty: AtomicBool,
    heightmap_dirty: AtomicBool,
    dirty_registration: Mutex<Option<DirtyColumnRegistration>>,
    /// Change counter: incremented on `mark_dirty`; used to invalidate `LevelChunkCache`.
    generation: AtomicU64,
    estimated_bytes: AtomicUsize,
    memory_counter: Option<Arc<AtomicUsize>>,
    memory_accounted: AtomicBool,
    spillover_pins: AtomicUsize,
}

/// Per-block arbitration state owned by the authoritative column.
///
/// The tracked window is bounded: a column that receives more distinct
/// spillover targets than [`MAX_TRACKED_SPILLOVER_CELLS`] keeps applying those
/// writes but stops recording them, so the deterministic guarantee degrades to
/// arrival order for the untracked positions. That degradation is counted and
/// reported instead of being silently assumed.
const MAX_TRACKED_SPILLOVER_CELLS: usize = 512;

/// Global block coordinate of one spillover target.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
struct SpilloverCell {
    layer: usize,
    x: i32,
    y: i32,
    z: i32,
}

impl SpilloverCell {
    fn of(write: &BlockSpilloverWrite) -> Self {
        Self {
            layer: write.layer,
            x: write.x,
            y: write.y,
            z: write.z,
        }
    }
}

/// Result of arbitrating one spillover batch against a column's winners.
#[derive(Default)]
struct SpilloverPartition {
    /// Writes this column applies now.
    applicable: Vec<SpilloverJournalEntry>,
    /// Writes the caller must re-queue for a later, ordered application.
    deferred: Vec<SpilloverJournalEntry>,
    /// Writes a higher-priority generation already superseded.
    superseded: u64,
    /// Deferred writes caused by a full arbitration window.
    untracked: u64,
}

#[derive(Default)]
struct SpilloverConflictState {
    /// block position → largest `SpilloverOperationId` already applied there.
    winners: HashMap<SpilloverCell, SpilloverOperationId>,
    /// Writes skipped because a higher-priority generation already won.
    superseded: u64,
    /// Positions that were re-queued because the tracked window was full.
    untracked: u64,
}

#[derive(Default)]
struct DirtyColumnIndex {
    keys: RwLock<BTreeSet<ChunkKey>>,
}

impl DirtyColumnIndex {
    fn insert(&self, key: ChunkKey) {
        self.keys
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key);
    }

    fn remove(&self, key: &ChunkKey) {
        self.keys
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key);
    }

    fn window(&self, after: Option<ChunkKey>, limit: usize) -> Vec<ChunkKey> {
        if limit == 0 {
            return Vec::new();
        }
        let keys = self
            .keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut selected = Vec::with_capacity(limit);
        if let Some(cursor) = after {
            selected.extend(
                keys.range((Excluded(cursor), Unbounded))
                    .take(limit)
                    .copied(),
            );
            if selected.len() < limit {
                selected.extend(keys.range(..=cursor).take(limit - selected.len()).copied());
            }
        } else {
            selected.extend(keys.iter().take(limit).copied());
        }
        selected
    }

    fn keys_after(
        &self,
        after: Option<ChunkKey>,
        through: Option<ChunkKey>,
        limit: usize,
    ) -> Vec<ChunkKey> {
        if limit == 0 {
            return Vec::new();
        }
        if after
            .zip(through)
            .is_some_and(|(cursor, end)| cursor >= end)
        {
            return Vec::new();
        }
        let keys = self
            .keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let start = after.map_or(Unbounded, Excluded);
        let end = through.map_or(Unbounded, Included);
        keys.range((start, end)).take(limit).copied().collect()
    }

    fn last_key(&self) -> Option<ChunkKey> {
        self.keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last()
            .copied()
    }
}

struct DirtyColumnRegistration {
    key: ChunkKey,
    index: Weak<DirtyColumnIndex>,
}

/// Reserves one resident cache slot while an insertion performs private
/// preparation. Other concurrent admissions include this reservation in
/// their count check; dropping it releases the slot on every error path.
struct CacheSlotReservation {
    pending: Arc<AtomicUsize>,
    active: bool,
}

impl CacheSlotReservation {
    fn commit(&mut self) {
        if self.active {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            self.active = false;
        }
    }
}

impl Drop for CacheSlotReservation {
    fn drop(&mut self) {
        if self.active {
            self.pending.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl ChunkColumn {
    pub fn new(chunk: Chunk) -> Self {
        Self::new_with_counter(chunk, None)
    }

    fn new_with_counter(chunk: Chunk, memory_counter: Option<Arc<AtomicUsize>>) -> Self {
        // Rust plugins can each link their own constructor and statics.
        let incarnation = uuid::Uuid::new_v4().as_u128();
        let estimated_bytes = chunk.estimated_memory_bytes() + std::mem::size_of::<Self>();
        if let Some(counter) = &memory_counter {
            counter.fetch_add(estimated_bytes, Ordering::Relaxed);
        }
        Self {
            chunk: RwLock::new(chunk),
            incarnation,
            dirty_subchunks: Mutex::new(BTreeSet::new()),
            applied_spillover_operations: Mutex::new(BTreeSet::new()),
            spillover_conflicts: Mutex::new(SpilloverConflictState::default()),
            block_entities_dirty: AtomicBool::new(false),
            biomes_dirty: AtomicBool::new(false),
            heightmap_dirty: AtomicBool::new(false),
            dirty_registration: Mutex::new(None),
            generation: AtomicU64::new(0),
            estimated_bytes: AtomicUsize::new(estimated_bytes),
            memory_counter,
            memory_accounted: AtomicBool::new(true),
            spillover_pins: AtomicUsize::new(0),
        }
    }

    fn refresh_memory_estimate(&self) -> usize {
        let chunk = self.read();
        self.refresh_memory_estimate_from_chunk(&chunk)
    }

    fn refresh_memory_estimate_from_chunk(&self, chunk: &Chunk) -> usize {
        let journal_bytes = self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
            .saturating_mul(64);
        let conflict_bytes = self
            .spillover_conflicts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .winners
            .len()
            .saturating_mul(
                std::mem::size_of::<SpilloverCell>() + std::mem::size_of::<SpilloverOperationId>(),
            );
        let current = chunk
            .estimated_memory_bytes()
            .saturating_add(std::mem::size_of::<Self>())
            .saturating_add(journal_bytes)
            .saturating_add(conflict_bytes);
        let previous = self.estimated_bytes.swap(current, Ordering::Relaxed);
        if let Some(counter) = &self.memory_counter {
            if self.memory_accounted.load(Ordering::Acquire) {
                if current > previous {
                    counter.fetch_add(current - previous, Ordering::Relaxed);
                } else {
                    counter.fetch_sub(previous - current, Ordering::Relaxed);
                }
            }
        }
        current
    }

    fn unaccount_memory(&self) {
        if !self.memory_accounted.swap(false, Ordering::AcqRel) {
            return;
        }
        if let Some(counter) = &self.memory_counter {
            counter.fetch_sub(
                self.estimated_bytes.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        }
    }

    fn pin_for_spillover(column: &Arc<Self>) -> ChunkColumnPin {
        column.spillover_pins.fetch_add(1, Ordering::AcqRel);
        ChunkColumnPin(Arc::clone(column))
    }

    fn is_pinned(&self) -> bool {
        self.spillover_pins.load(Ordering::Acquire) != 0
    }

    /// Number of distinct block positions currently arbitrated by this column.
    pub(crate) fn tracked_spillover_cells(&self) -> usize {
        self.spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .winners
            .len()
    }

    /// Generation writes skipped because a higher-priority source won.
    pub(crate) fn superseded_spillover_writes(&self) -> u64 {
        self.spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .superseded
    }

    /// Positions applied without arbitration because the tracked window was full.
    pub(crate) fn untracked_spillover_writes(&self) -> u64 {
        self.spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .untracked
    }

    fn unapplied_spillover_entries(
        &self,
        entries: &[SpilloverJournalEntry],
    ) -> Vec<SpilloverJournalEntry> {
        let applied = self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries
            .iter()
            .filter(|entry| !applied.contains(&entry.operation_id))
            .copied()
            .collect()
    }

    /// Read-only arbitration preview for one spillover batch.
    ///
    /// `entries` must be ordered by `operation_id` (see `merge_spillover_entries`),
    /// which makes the intra-batch result deterministic; the recorded winners make
    /// the *cross-batch* result independent of worker completion order.
    ///
    /// This never mutates state: a batch that is later rejected must not reserve
    /// arbitration it did not publish.
    fn preview_spillover_conflicts(&self, entries: &[SpilloverJournalEntry]) -> SpilloverPartition {
        if entries.is_empty() {
            return SpilloverPartition::default();
        }
        let state = self
            .spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut applicable = Vec::with_capacity(entries.len());
        let mut superseded = 0u64;
        let mut untracked = 0u64;
        // Intra-batch winners are simulated locally so a batch that lists two
        // sources for the same position still resolves by operation id, not by
        // its internal order.
        let mut simulated: HashMap<SpilloverCell, SpilloverOperationId> = HashMap::new();
        // Positions outside the bounded arbitration window are *not* resolved by
        // arrival order: they are handed back so the caller re-queues them and the
        // owning column applies them, in operation-id order, the next time it is
        // materialized. That keeps the published result independent of which
        // worker finished first, at the cost of delayed application.
        let mut deferred = Vec::new();
        for entry in entries {
            let cell = SpilloverCell::of(&entry.write);
            let winner = simulated
                .get(&cell)
                .copied()
                .or_else(|| state.winners.get(&cell).copied());
            match winner {
                Some(winner) if winner >= entry.operation_id => {
                    // A higher-priority generation already published this block.
                    superseded = superseded.saturating_add(1);
                    continue;
                }
                Some(_) => {}
                None => {
                    if state.winners.len().saturating_add(simulated.len())
                        >= MAX_TRACKED_SPILLOVER_CELLS
                    {
                        untracked = untracked.saturating_add(1);
                        deferred.push(*entry);
                        continue;
                    }
                }
            }
            simulated.insert(cell, entry.operation_id);
            applicable.push(*entry);
        }
        SpilloverPartition {
            applicable,
            deferred,
            superseded,
            untracked,
        }
    }

    /// Record the winners of a batch that is certain to be applied.
    fn commit_spillover_conflicts(&self, applicable: &[SpilloverJournalEntry], untracked: u64) {
        if applicable.is_empty() && untracked == 0 {
            return;
        }
        let mut state = self
            .spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.untracked = state.untracked.saturating_add(untracked);
        for entry in applicable {
            state
                .winners
                .entry(SpilloverCell::of(&entry.write))
                .and_modify(|winner| {
                    if *winner < entry.operation_id {
                        *winner = entry.operation_id;
                    }
                })
                .or_insert(entry.operation_id);
        }
    }

    /// Publish the superseded counter of a batch that was applied.
    fn note_superseded(&self, superseded: u64) {
        if superseded == 0 {
            return;
        }
        let mut state = self
            .spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.superseded = state.superseded.saturating_add(superseded);
    }

    /// Register provider-owned columns in its dirty-key index. Call this only
    /// when the caller has established that this column will be published for
    /// `key`; a clean/dirty snapshot is synchronized under the dirty lock.
    fn attach_dirty_index(&self, key: ChunkKey, index: &Arc<DirtyColumnIndex>) {
        let dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut registration = self
            .dirty_registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(current) = registration.as_ref() {
            if current.key == key
                && current
                    .index
                    .upgrade()
                    .is_some_and(|existing| Arc::ptr_eq(&existing, index))
            {
                self.sync_dirty_index_locked(&dirty, &mut registration);
                return;
            }
        }

        if let Some(previous) = registration.take() {
            if let Some(previous_index) = previous.index.upgrade() {
                previous_index.remove(&previous.key);
            }
        }
        *registration = Some(DirtyColumnRegistration {
            key,
            index: Arc::downgrade(index),
        });
        self.sync_dirty_index_locked(&dirty, &mut registration);
    }

    /// Caller holds `dirty_subchunks` so dirty-state and index transitions are
    /// atomic with respect to mark/clear operations. No code may hold the index
    /// lock while acquiring a column dirty lock.
    fn sync_dirty_index_locked(
        &self,
        dirty: &BTreeSet<i8>,
        registration: &mut Option<DirtyColumnRegistration>,
    ) {
        let Some(current) = registration.as_ref() else {
            return;
        };
        let Some(index) = current.index.upgrade() else {
            *registration = None;
            return;
        };
        let has_spillover = !self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty();
        if dirty.is_empty()
            && !self.block_entities_dirty.load(Ordering::Acquire)
            && !self.biomes_dirty.load(Ordering::Acquire)
            && !self.heightmap_dirty.load(Ordering::Acquire)
            && !has_spillover
        {
            index.remove(&current.key);
        } else {
            index.insert(current.key);
        }
    }

    fn remove_dirty_registration(&mut self) {
        let registration = self
            .dirty_registration
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(registration) = registration.take() {
            if let Some(index) = registration.index.upgrade() {
                index.remove(&registration.key);
            }
        }
    }

    fn remove_stale_dirty_index_if_clean(&self) {
        let dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let has_spillover = !self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty();
        if dirty.is_empty()
            && !self.block_entities_dirty.load(Ordering::Acquire)
            && !self.biomes_dirty.load(Ordering::Acquire)
            && !self.heightmap_dirty.load(Ordering::Acquire)
            && !has_spillover
        {
            let mut registration = self
                .dirty_registration
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.sync_dirty_index_locked(&dirty, &mut registration);
        }
    }

    pub fn read(&self) -> RwLockReadGuard<'_, Chunk> {
        self.chunk
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, Chunk> {
        self.chunk
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn mark_dirty(&self, subchunk_y: i8) {
        self.mark_dirty_subchunks(&[subchunk_y]);
    }

    /// Mark a batch of subchunks dirty with one generation and memory-estimate
    /// refresh after the Chunk write guard has been released.
    ///
    /// This legacy path is retained for non-concurrent callers. Concurrent
    /// Chunk mutations should publish with `mark_dirty_locked` before dropping
    /// the guard.
    fn mark_dirty_subchunks(&self, subchunk_ys: &[i8]) {
        self.mark_dirty_subchunks_with_metadata(subchunk_ys, false, false, false);
    }

    fn mark_dirty_subchunks_with_metadata(
        &self,
        subchunk_ys: &[i8],
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
    ) {
        if self.publish_dirty_markers(
            subchunk_ys.iter().copied(),
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            &[],
        ) {
            self.refresh_memory_estimate();
        }
    }

    /// Publish a mutation while the same column's write guard is still held.
    /// This makes the changed Chunk and its generation/dirty markers one
    /// snapshot boundary; writeback can never observe new content as clean.
    /// The caller must pass a guard acquired from this `ChunkColumn`.
    pub fn mark_dirty_with_metadata_locked<I>(
        &self,
        chunk: &RwLockWriteGuard<'_, Chunk>,
        subchunk_ys: I,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
    ) where
        I: IntoIterator<Item = i8>,
    {
        self.mark_dirty_with_metadata_locked_and_spillover(
            chunk,
            subchunk_ys,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            &[],
        );
    }

    /// Publish a mutation together with the durable spillover facts included
    /// in the same column state.
    pub(crate) fn mark_dirty_with_metadata_locked_and_spillover<I>(
        &self,
        chunk: &RwLockWriteGuard<'_, Chunk>,
        subchunk_ys: I,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
        spillover_operations: &[SpilloverOperationId],
    ) where
        I: IntoIterator<Item = i8>,
    {
        if self.publish_dirty_markers(
            subchunk_ys,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            spillover_operations,
        ) {
            self.refresh_memory_estimate_from_chunk(chunk);
        }
    }

    /// Mark one changed subchunk before releasing its write guard.
    pub fn mark_dirty_locked(&self, chunk: &RwLockWriteGuard<'_, Chunk>, subchunk_y: i8) {
        self.mark_dirty_with_metadata_locked(
            chunk,
            std::iter::once(subchunk_y),
            false,
            false,
            false,
        );
    }

    fn publish_dirty_markers<I>(
        &self,
        subchunk_ys: I,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
        spillover_operations: &[SpilloverOperationId],
    ) -> bool
    where
        I: IntoIterator<Item = i8>,
    {
        let mut subchunk_ys = subchunk_ys.into_iter().peekable();
        let has_subchunks = subchunk_ys.peek().is_some();
        if !has_subchunks
            && !block_entities_dirty
            && !biomes_dirty
            && !heightmap_dirty
            && spillover_operations.is_empty()
        {
            return false;
        }

        let mut dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        dirty.extend(subchunk_ys);
        if has_subchunks || heightmap_dirty {
            self.heightmap_dirty.store(true, Ordering::Release);
        }
        if block_entities_dirty {
            self.block_entities_dirty.store(true, Ordering::Release);
        }
        if biomes_dirty {
            self.biomes_dirty.store(true, Ordering::Release);
        }
        if !spillover_operations.is_empty() {
            self.applied_spillover_operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend(spillover_operations.iter().copied());
        }
        let mut registration = self
            .dirty_registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.sync_dirty_index_locked(&dirty, &mut registration);
        true
    }

    /// Mark the block-entity NBT record as modified or deleted. Call after the
    /// column write guard has been released, like `mark_dirty`, so the estimate
    /// refresh cannot re-enter the chunk lock.
    pub fn mark_block_entities_dirty(&self) {
        self.mark_dirty_subchunks_with_metadata(&[], true, false, false);
    }

    pub fn block_entities_dirty(&self) -> bool {
        self.block_entities_dirty.load(Ordering::Acquire)
    }

    pub fn mark_biomes_dirty(&self) {
        self.mark_dirty_subchunks_with_metadata(&[], false, true, false);
    }

    pub fn mark_heightmap_dirty(&self) {
        self.mark_dirty_subchunks_with_metadata(&[], false, false, true);
    }

    pub fn biomes_dirty(&self) -> bool {
        self.biomes_dirty.load(Ordering::Acquire)
    }

    pub fn heightmap_dirty(&self) -> bool {
        self.heightmap_dirty.load(Ordering::Acquire)
    }

    /// Capture generation and dirty kinds under the same mutex used to publish
    /// them. Call while holding a chunk read guard when pairing this state with
    /// an owned content snapshot.
    pub(crate) fn writeback_state(&self) -> (u64, bool, bool, bool, bool) {
        let dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let has_spillover = !self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty();
        let block_entities_dirty = self.block_entities_dirty();
        let biomes_dirty = self.biomes_dirty();
        let heightmap_dirty = self.heightmap_dirty();
        (
            self.generation(),
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            !dirty.is_empty()
                || has_spillover
                || block_entities_dirty
                || biomes_dirty
                || heightmap_dirty,
        )
    }

    pub(crate) fn writeback_state_with_spillover(
        &self,
    ) -> (u64, bool, bool, bool, bool, Vec<SpilloverOperationId>) {
        let dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let spillover_operations = self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let block_entities_dirty = self.block_entities_dirty.load(Ordering::Acquire);
        let biomes_dirty = self.biomes_dirty.load(Ordering::Acquire);
        let heightmap_dirty = self.heightmap_dirty.load(Ordering::Acquire);
        (
            self.generation(),
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            !dirty.is_empty()
                || block_entities_dirty
                || biomes_dirty
                || heightmap_dirty
                || !spillover_operations.is_empty(),
            spillover_operations.iter().copied().collect(),
        )
    }

    /// Change counter (for LevelChunkCache generation comparison).
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn incarnation(&self) -> u128 {
        self.incarnation
    }

    pub fn is_dirty(&self) -> bool {
        self.writeback_state().4
    }

    /// Takes and clears the dirty-subchunk set (called when writing back to the archive).
    pub fn take_dirty(&self) -> Vec<i8> {
        let mut dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let taken = std::mem::take(&mut *dirty).into_iter().collect();
        let mut registration = self
            .dirty_registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.sync_dirty_index_locked(&dirty, &mut registration);
        taken
    }

    /// Clear dirty markers only when no mutation happened after the snapshot.
    pub fn estimated_memory_bytes(&self) -> usize {
        self.refresh_memory_estimate()
    }

    /// Estimate the live ChunkColumn contents plus the memory that cloning its
    /// Chunk for writeback can allocate. This includes deep-owned NBT buffers
    /// but does not claim to include LevelDB serialization or allocator/RSS
    /// overhead.
    pub fn writeback_snapshot_estimated_bytes(&self) -> usize {
        let chunk = self.read();
        self.refresh_memory_estimate_from_chunk(&chunk);
        self.snapshot_estimated_bytes(&chunk)
    }

    pub(crate) fn snapshot_estimated_bytes(&self, chunk: &Chunk) -> usize {
        let ids = self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len();
        chunk
            .estimated_writeback_snapshot_bytes()
            .saturating_add(std::mem::size_of::<Self>())
            .saturating_add(
                ids.saturating_mul(64 + 2 * std::mem::size_of::<SpilloverOperationId>()),
            )
    }

    pub fn take_dirty_if_generation(&self, expected: u64) -> bool {
        self.take_dirty_if_generation_and_spillover(expected, None)
    }

    pub(crate) fn take_dirty_if_generation_and_spillover(
        &self,
        expected: u64,
        acknowledged: Option<&[SpilloverOperationId]>,
    ) -> bool {
        let mut dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        {
            let mut spillover_operations = self
                .applied_spillover_operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(acknowledged) = acknowledged {
                for operation_id in acknowledged {
                    spillover_operations.remove(operation_id);
                }
            } else if !spillover_operations.is_empty() {
                // Legacy callers cannot acknowledge durable facts without a
                // matching storage receipt.
                return false;
            }
        }
        let unchanged = self.generation() == expected;
        if unchanged {
            dirty.clear();
            self.block_entities_dirty.store(false, Ordering::Release);
            self.biomes_dirty.store(false, Ordering::Release);
            self.heightmap_dirty.store(false, Ordering::Release);
        }
        let mut registration = self
            .dirty_registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.sync_dirty_index_locked(&dirty, &mut registration);
        unchanged
    }
}

/// Temporary cache pin held while a spillover write operates outside the
/// global cache lock. This prevents clean-target eviction during the write.
struct ChunkColumnPin(Arc<ChunkColumn>);

impl Drop for ChunkColumnPin {
    fn drop(&mut self) {
        self.0.spillover_pins.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for ChunkColumn {
    fn drop(&mut self) {
        self.remove_dirty_registration();
        if self.memory_accounted.load(Ordering::Acquire) {
            if let Some(counter) = &self.memory_counter {
                counter.fetch_sub(
                    self.estimated_bytes.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
            }
        }
    }
}

#[derive(Debug)]
struct PendingSpilloverBatch {
    writes: Vec<SpilloverJournalEntry>,
    first_enqueued_at: Instant,
}

/// Rate-limited diagnostic for spillover arbitration outcomes.
///
/// `superseded` is a defined generation conflict policy (largest operation id
/// wins), not data loss. `deferred` means the column's bounded arbitration window
/// was full: those writes are re-queued and applied in operation-id order when
/// the column is next materialized, so no position is ever resolved by arrival
/// order.
fn report_spillover_arbitration(target: Option<ChunkKey>, superseded: u64, untracked: u64) {
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
                return Err(WorldStorageError::Corrupt(format!(
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

/// Combines saved-world storage, an optional generator, and a shared cache.
#[derive(Clone)]
pub struct WorldChunkProvider {
    storage: Arc<dyn WorldStorage>,
    generator: Option<Arc<dyn WorldGenerator>>,
    cache: Arc<RwLock<HashMap<ChunkKey, Arc<ChunkColumn>>>>,
    cache_order: Arc<RwLock<VecDeque<ChunkKey>>>,
    /// Bounded strong pins for clean columns evicted while external Arc owners
    /// still exist. If one of those handles becomes dirty later, the provider
    /// can still recover/persist it after the caller releases its Arc.
    retired: Arc<RwLock<HashMap<ChunkKey, Arc<ChunkColumn>>>>,
    /// Ordered index of dirty provider-owned columns; avoids scanning every
    /// resident/retired entry on each writeback scheduling tick.
    dirty_index: Arc<DirtyColumnIndex>,
    /// Pending queue for cross-chunk spillover writes: staged while the target
    /// chunk is not cached yet, applied when that chunk first enters the cache
    /// (via generate/load/ensure).
    pending_spillover: Arc<Mutex<HashMap<ChunkKey, PendingSpilloverBatch>>>,
    pending_spillover_count: Arc<AtomicUsize>,
    pending_spillover_bytes: Arc<AtomicUsize>,
    spillover_generation_reserved_bytes: Arc<AtomicUsize>,
    /// Estimated bytes currently held by unacknowledged dirty columns.
    ///
    /// Updated when a column is marked dirty and again when a SaveAck clears it.
    /// This is a structural estimate (same accounting as `estimated_memory_bytes`),
    /// not an RSS figure.
    dirty_bytes: Arc<AtomicUsize>,
    /// Number of times the dirty backlog refused new generation.
    generation_throttled: Arc<AtomicUsize>,
    /// Targets that were replaced between routing and publication. Reachable
    /// only if publication stops sharing the routed key's stripe lock; counted
    /// so a future refactor cannot make it a silent write to a stale column.
    replaced_spillover_targets: Arc<AtomicUsize>,
    /// Fixed-size striped key locks serialize spillover admission/application
    /// with publication of the same target column without an unbounded lock map.
    spillover_locks: Arc<[Mutex<()>; SPILLOVER_LOCK_STRIPES]>,
    cache_memory_estimate: Arc<AtomicUsize>,
    /// Resident slots reserved by columns being prepared for publication.
    pending_cache_slots: Arc<AtomicUsize>,
    max_cache_entries: usize,
    max_cache_bytes: usize,
}

impl WorldChunkProvider {
    /// Resident column entry limit. At radius 10 a single square view is
    /// about 441 columns; 1024 leaves overlap headroom. Dirty/pinned columns
    /// are never evicted to satisfy admission: callers receive Capacity until
    /// writeback or released external handles make room.
    const DEFAULT_CACHE_CAPACITY: usize = 1024;

    /// Writes a chunk back: delegates to the backend storage (LevelDB/memory).
    pub fn save_chunk(&self, key: ChunkKey, chunk: &Chunk) -> Result<(), WorldStorageError> {
        self.storage.save_chunk(key, chunk)
    }

    pub(crate) fn storage_handle(&self) -> Arc<dyn WorldStorage> {
        Arc::clone(&self.storage)
    }

    /// Bounded dirty-column window ordered cyclically after the caller's key
    /// cursor. The index provides O(log n + limit) key selection; only the
    /// bounded selected set is resolved to column handles.
    pub(crate) fn dirty_columns_window(
        &self,
        after: Option<ChunkKey>,
        limit: usize,
    ) -> Result<Vec<(ChunkKey, Arc<ChunkColumn>)>, ()> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let keys = self.dirty_index.window(after, limit);
        self.resolve_dirty_keys(keys)
    }

    /// Return an ordered, non-wrapping page for one-pass shutdown flushing.
    /// `last_examined` advances even when some selected index entries prove
    /// stale during resolution, so one poisoned/stale key cannot pin the scan.
    fn dirty_columns_after(
        &self,
        after: Option<ChunkKey>,
        through: Option<ChunkKey>,
        limit: usize,
    ) -> Result<(Option<ChunkKey>, Vec<(ChunkKey, Arc<ChunkColumn>)>), ()> {
        let keys = self.dirty_index.keys_after(after, through, limit);
        let last_examined = keys.last().copied();
        let columns = self.resolve_dirty_keys(keys)?;
        Ok((last_examined, columns))
    }

    fn resolve_dirty_keys(
        &self,
        keys: Vec<ChunkKey>,
    ) -> Result<Vec<(ChunkKey, Arc<ChunkColumn>)>, ()> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let cache = self.cache.read().map_err(|_| ())?;
        let retired = self.retired.read().map_err(|_| ())?;
        let mut candidates = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(column) = cache.get(&key).or_else(|| retired.get(&key)) {
                if column.is_dirty() {
                    candidates.push((key, Arc::clone(column)));
                } else {
                    column.remove_stale_dirty_index_if_clean();
                }
            } else {
                // While both cache read guards are held the key cannot be
                // concurrently inserted into either owner map.
                self.dirty_index.remove(&key);
            }
        }
        Ok(candidates)
    }

    /// Authoritative allocation-free dirty count for shutdown reporting. The
    /// provider maps own every registered dirty column (resident or retired),
    /// so dedupe by map lookup rather than a temporary HashSet.
    fn count_dirty_columns(&self) -> Result<usize, ()> {
        let cache = self.cache.read().map_err(|_| ())?;
        let retired = self.retired.read().map_err(|_| ())?;
        let mut count = cache.values().filter(|column| column.is_dirty()).count();
        count += retired
            .iter()
            .filter(|(key, column)| !cache.contains_key(key) && column.is_dirty())
            .count();
        Ok(count)
    }

    pub fn new(storage: Arc<dyn WorldStorage>) -> Self {
        Self {
            storage,
            generator: None,
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_order: Arc::new(RwLock::new(VecDeque::new())),
            retired: Arc::new(RwLock::new(HashMap::new())),
            dirty_index: Arc::new(DirtyColumnIndex::default()),
            pending_spillover: Arc::new(Mutex::new(HashMap::new())),
            pending_spillover_count: Arc::new(AtomicUsize::new(0)),
            pending_spillover_bytes: Arc::new(AtomicUsize::new(0)),
            spillover_generation_reserved_bytes: Arc::new(AtomicUsize::new(0)),
            dirty_bytes: Arc::new(AtomicUsize::new(0)),
            generation_throttled: Arc::new(AtomicUsize::new(0)),
            replaced_spillover_targets: Arc::new(AtomicUsize::new(0)),
            spillover_locks: Arc::new(std::array::from_fn(|_| Mutex::new(()))),
            cache_memory_estimate: Arc::new(AtomicUsize::new(0)),
            pending_cache_slots: Arc::new(AtomicUsize::new(0)),
            max_cache_entries: Self::DEFAULT_CACHE_CAPACITY,
            max_cache_bytes: 384 * 1024 * 1024,
        }
    }

    pub fn with_cache_limit(mut self, max_entries: usize) -> Self {
        self.max_cache_entries = max_entries.max(1);
        self
    }

    pub fn with_cache_memory_limit(mut self, max_bytes: usize) -> Self {
        self.max_cache_bytes = max_bytes.max(1);
        self
    }

    pub fn with_generator(mut self, generator: Arc<dyn WorldGenerator>) -> Self {
        self.generator = Some(generator);
        self
    }

    fn reserve_spillover_generation(&self) -> Result<SpilloverGenerationPermit, WorldStorageError> {
        let bytes = MAX_PENDING_SPILLOVER_BYTES;
        let mut current = self
            .spillover_generation_reserved_bytes
            .load(Ordering::Acquire);
        loop {
            let next = current.saturating_add(bytes);
            if next > MAX_IN_FLIGHT_SPILLOVER_BYTES {
                return Err(WorldStorageError::Capacity(format!(
                    "spillover generation workers are at the {} byte in-flight limit",
                    MAX_IN_FLIGHT_SPILLOVER_BYTES
                )));
            }
            match self
                .spillover_generation_reserved_bytes
                .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    return Ok(SpilloverGenerationPermit {
                        reserved_bytes: Arc::clone(&self.spillover_generation_reserved_bytes),
                        bytes,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn spillover_lock_index(key: ChunkKey) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish() as usize & (SPILLOVER_LOCK_STRIPES - 1)
    }

    /// Lock all affected key stripes in a stable order. Collisions only add
    /// harmless serialization; the fixed array keeps lock metadata bounded.
    fn spillover_lock_set(keys: impl Iterator<Item = ChunkKey>) -> [bool; SPILLOVER_LOCK_STRIPES] {
        let mut selected = [false; SPILLOVER_LOCK_STRIPES];
        for key in keys {
            selected[Self::spillover_lock_index(key)] = true;
        }
        selected
    }

    fn with_spillover_key_locks<R>(
        &self,
        selected: [bool; SPILLOVER_LOCK_STRIPES],
        operation: impl FnOnce() -> R,
    ) -> R {
        let _guards = selected
            .iter()
            .enumerate()
            .filter(|(_, selected)| **selected)
            .map(|(index, _)| {
                self.spillover_locks[index]
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            })
            .collect::<Vec<_>>();
        operation()
    }

    /// Inserts an empty column into the cache without reading storage.
    ///
    /// This is used after a worker has established that a chunk is absent.
    /// Keeping the insertion separate from `ensure_chunk` prevents a second
    /// synchronous storage read on the main thread. Admission is bounded by
    /// resident-entry and estimated-live-byte limits; a rejected empty column
    /// is returned as `WorldStorageError::Capacity` for caller retry/failure.
    pub fn insert_empty_chunk(
        &self,
        key: ChunkKey,
        min_y: i32,
        max_y: i32,
    ) -> Result<Arc<ChunkColumn>, WorldStorageError> {
        if let Some(column) = self.cached_chunk(key) {
            return Ok(column);
        }
        self.insert_cached(key, Chunk::empty(key.position, key.dimension, min_y, max_y))
    }

    pub fn load_chunk(
        &self,
        key: ChunkKey,
        min_y: i32,
        max_y: i32,
    ) -> Result<Option<Arc<ChunkColumn>>, WorldStorageError> {
        if let Some(column) = self
            .cache
            .read()
            .ok()
            .and_then(|cache| cache.get(&key).cloned())
        {
            return Ok(Some(column));
        }

        let (saved_chunk, persisted_spillover) = self.storage.load_chunk_with_spillover(key)?;
        if let Some(entry) = persisted_spillover.iter().find(|entry| {
            let write = entry.write;
            write.key != key
                || write.key.position != ChunkPosition::from_world(write.x, write.z)
                || write.layer > 1
                || !(min_y..=max_y).contains(&write.y)
        }) {
            return Err(WorldStorageError::Corrupt(format!(
                "invalid persisted spillover operation {:?} for target {key:?}",
                entry.operation_id
            )));
        }
        let mut spillover = Vec::new();
        let mut generated_source = false;
        // Keep the in-flight result budget reserved until the generated
        // spillover is either routed or rejected below.
        let mut _spillover_generation_permit = None;
        let loaded = match saved_chunk {
            Some(chunk) => Some(chunk),
            None => {
                if self.generator.is_none() {
                    if !self.storage.supports_spillover_journal() {
                        return Ok(None);
                    }
                    Some(Chunk::empty(key.position, key.dimension, min_y, max_y))
                } else {
                    // §3.2: throttle *new* generation while unacknowledged
                    // dirty data accumulates. Refusal is explicit and retryable;
                    // it never converts into a silent air column.
                    self.generation_admission()?;
                    let generator = self.generator.as_ref().expect("generator checked");
                    _spillover_generation_permit = Some(self.reserve_spillover_generation()?);
                    match generator.generate_chunk_with_spillover_bounded(
                        ChunkGenerationRequest { key, min_y, max_y },
                        MAX_GENERATED_SPILLOVER_WRITES,
                    )? {
                        Some(generated) => {
                            generated_source = true;
                            spillover = generated
                                .spillover
                                .into_iter()
                                .enumerate()
                                .map(|(ordinal, write)| SpilloverJournalEntry {
                                    operation_id: SpilloverOperationId::new(key, ordinal as u32),
                                    write,
                                })
                                .collect();
                            Some(generated.chunk)
                        }
                        None => None,
                    }
                }
            }
        };

        let chunk = match loaded {
            Some(chunk) => chunk,
            None if self.storage.supports_spillover_journal() => {
                Chunk::empty(key.position, key.dimension, min_y, max_y)
            }
            None => return Ok(None),
        };
        if chunk.position != key.position
            || chunk.dimension != key.dimension
            || chunk.min_y != min_y
            || chunk.max_y != max_y
        {
            return Err(WorldStorageError::Corrupt(
                "loaded/generated source identity or bounds mismatch".into(),
            ));
        }
        let source_block_entities_dirty = generated_source && !chunk.block_entities.is_empty();
        let source_biomes_dirty = generated_source && !chunk.biomes.is_empty();
        let source_heightmap_dirty =
            generated_source && source_biomes_dirty && chunk.data_3d_heightmap.is_none();
        if spillover.len() > MAX_PENDING_SPILLOVER_WRITES {
            let lock_set = Self::spillover_lock_set(std::iter::once(key));
            return self.with_spillover_key_locks(lock_set, || {
                if let Some(existing) = self.cached_chunk(key) {
                    return Ok(Some(existing));
                }
                Err(WorldStorageError::Backend(format!(
                    "spillover generation result rejected atomically: {} writes exceed the per-result budget of {MAX_PENDING_SPILLOVER_WRITES}",
                    spillover.len()
                )))
            });
        }

        if let Some((index, entry)) = spillover.iter().enumerate().find(|(_, entry)| {
            let write = entry.write;
            write.key.dimension != key.dimension
                || write.key.position != ChunkPosition::from_world(write.x, write.z)
                || write.layer > 1
                || !(min_y..=max_y).contains(&write.y)
        }) {
            let lock_set = Self::spillover_lock_set(std::iter::once(key));
            return self.with_spillover_key_locks(lock_set, || {
                if let Some(existing) = self.cached_chunk(key) {
                    return Ok(Some(existing));
                }
                Err(WorldStorageError::Backend(format!(
                    "invalid spillover write #{index} from {key:?}: target={:?}, world=({}, {}, {}), layer={}",
                    entry.write.key,
                    entry.write.x,
                    entry.write.y,
                    entry.write.z,
                    entry.write.layer
                )))
            });
        }

        // Keep the generated source private until all of its spillover writes
        // are either applied to cached targets or admitted to the bounded
        // pending queue. If the whole batch cannot fit, publish neither a
        // partial spillover nor the source column; callers receive an explicit
        // load failure rather than a successful partial worldgen result.
        let lock_set = Self::spillover_lock_set(
            std::iter::once(key).chain(spillover.iter().map(|entry| entry.write.key)),
        );
        self.with_spillover_key_locks(lock_set, || {
            if let Some(existing) = self.cached_chunk(key) {
                // Another attempt already published this key after routing
                // its spillovers. Do not replay a stale duplicate result.
                return Ok(Some(existing));
            }

            let mut column = Arc::new(ChunkColumn::new_with_counter(
                chunk,
                Some(Arc::clone(&self.cache_memory_estimate)),
            ));
            // Admit the generated source before routing spillover to any
            // published target, so capacity rejection cannot leave partial
            // world-generation effects behind.
            let mut reservation = self.reserve_cache_slot(key, &column)?;
            let (source_writes, committed) = self.route_spillover_locked(
                spillover,
                Some(key),
                generated_source.then_some(column.as_ref()),
            )?;
            let (incoming, metadata_dirty) = if let Some(committed) = committed {
                if !committed.newly_committed {
                    column = Arc::new(ChunkColumn::new_with_counter(
                        committed.source,
                        Some(Arc::clone(&self.cache_memory_estimate)),
                    ));
                    drop(reservation);
                    reservation = self.reserve_cache_slot(key, &column)?;
                }
                (committed.incoming, false)
            } else {
                (persisted_spillover, true)
            };
            let applied_spillover =
                merge_spillover_entries(incoming.into_iter().chain(source_writes))?;
            // The source column is being materialized here, so its absorbed
            // spillover is applied in operation-id order rather than arbitrated.
            Self::apply_pending_ordered_to_column(
                &column,
                &applied_spillover,
                metadata_dirty && source_block_entities_dirty,
                metadata_dirty && source_biomes_dirty,
                metadata_dirty && source_heightmap_dirty,
            );
            Ok(Some(self.insert_column_cached_locked(
                key,
                column,
                Some(reservation),
            )?))
        })
    }

    /// Load a chunk; create and cache an all-air empty chunk when absent (/setblock into
    /// ungenerated chunks and future worldgen backfill both go through here).
    pub fn ensure_chunk(
        &self,
        key: ChunkKey,
        min_y: i32,
        max_y: i32,
    ) -> Result<Arc<ChunkColumn>, WorldStorageError> {
        if let Some(column) = self.load_chunk(key, min_y, max_y)? {
            return Ok(column);
        }
        let empty = Chunk::empty(key.position, key.dimension, min_y, max_y);
        self.insert_cached(key, empty)
    }

    /// Serialize cache publication for this key with spillover routing.
    fn insert_cached(
        &self,
        key: ChunkKey,
        chunk: Chunk,
    ) -> Result<Arc<ChunkColumn>, WorldStorageError> {
        self.with_spillover_key_locks(Self::spillover_lock_set(std::iter::once(key)), || {
            if let Some(existing) = self.pin_cached_chunk(key) {
                return Ok(Arc::clone(&existing.0));
            }
            let column = Arc::new(ChunkColumn::new_with_counter(
                chunk,
                Some(Arc::clone(&self.cache_memory_estimate)),
            ));
            self.insert_column_cached_locked(key, column, None)
        })
    }

    /// Estimated bytes currently held by unacknowledged dirty snapshots.
    pub fn dirty_bytes(&self) -> usize {
        self.dirty_bytes.load(Ordering::Acquire)
    }

    /// Account an accepted dirty snapshot against the dirty budget.
    pub(crate) fn note_dirty_bytes(&self, bytes: usize) {
        self.dirty_bytes.fetch_add(bytes, Ordering::AcqRel);
    }

    /// Release a dirty snapshot's accounting once it is confirmed persisted.
    pub(crate) fn release_dirty_bytes(&self, bytes: usize) {
        let _ = self
            .dirty_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_sub(bytes))
            });
    }

    /// Whether new generation may start under the current dirty backlog.
    ///
    /// Unacknowledged dirty data is the only world state that cannot be recreated
    /// from a seed, so past the high-water mark the provider throttles *generation*
    /// (recoverable work) instead of accepting more. Gameplay edits keep their own
    /// admission path and are never silently dropped here.
    pub fn generation_admission(&self) -> Result<(), WorldStorageError> {
        let dirty = self.dirty_bytes();
        if dirty >= DIRTY_CRITICAL_BYTES {
            return Err(WorldStorageError::Capacity(format!(
                "dirty backlog {dirty} bytes reached the critical mark {DIRTY_CRITICAL_BYTES}; \
                 generation is suspended until writeback confirms"
            )));
        }
        if dirty >= DIRTY_HIGH_WATER_BYTES {
            self.generation_throttled.fetch_add(1, Ordering::Relaxed);
            return Err(WorldStorageError::Capacity(format!(
                "dirty backlog {dirty} bytes exceeded the high-water mark \
                 {DIRTY_HIGH_WATER_BYTES}; generation retries after writeback"
            )));
        }
        Ok(())
    }

    /// How many times generation was refused by the dirty backlog.
    pub fn generation_throttled(&self) -> u64 {
        self.generation_throttled.load(Ordering::Relaxed) as u64
    }

    fn pin_cached_chunk(&self, key: ChunkKey) -> Option<ChunkColumnPin> {
        let cache = self
            .cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(column) = cache.get(&key) {
            return Some(ChunkColumn::pin_for_spillover(column));
        }

        // Keep the lock order cache → retired. A clean retired handle with no
        // external owners can be discarded; dirty or externally owned handles
        // remain canonical for this key until they are safely saved/released.
        let mut retired = self
            .retired
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let discard = retired
            .get(&key)
            .is_some_and(|column| !column.is_dirty() && Arc::strong_count(column) == 1);
        if discard {
            if let Some(column) = retired.remove(&key) {
                column.unaccount_memory();
            }
            return None;
        }
        retired.get(&key).map(ChunkColumn::pin_for_spillover)
    }

    fn take_pending_spillover(&self, key: ChunkKey) -> Vec<SpilloverJournalEntry> {
        let mut pending = self
            .pending_spillover
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let writes = pending
            .remove(&key)
            .map(|batch| batch.writes)
            .unwrap_or_default();
        self.pending_spillover_count
            .fetch_sub(writes.len(), Ordering::Relaxed);
        self.pending_spillover_bytes.fetch_sub(
            writes
                .len()
                .saturating_mul(SPILLOVER_ESTIMATED_BYTES_PER_WRITE),
            Ordering::AcqRel,
        );
        writes
    }

    /// Reserve one resident slot for an already materialized column.
    /// `cache_memory_estimate` includes that candidate (its ChunkColumn was
    /// constructed with the provider counter) and all retired live handles.
    fn reserve_cache_slot(
        &self,
        key: ChunkKey,
        candidate: &ChunkColumn,
    ) -> Result<CacheSlotReservation, WorldStorageError> {
        let candidate_bytes = candidate.estimated_memory_bytes();
        if candidate_bytes > self.max_cache_bytes {
            return Err(WorldStorageError::Capacity(format!(
                "column {key:?} estimate {candidate_bytes} exceeds cache byte limit {}",
                self.max_cache_bytes
            )));
        }
        let mut cache = self
            .cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut order = self
            .cache_order
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut retired = self
            .retired
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Dirty retired handles remain canonical in the retired map and are
        // already served by cached_chunk/writeback. Do not restore them into
        // the resident LRU, which would defeat its hard entry limit.
        retired.retain(|_, existing| {
            if !existing.is_dirty() && !existing.is_pinned() && Arc::strong_count(existing) == 1 {
                existing.unaccount_memory();
                false
            } else {
                true
            }
        });

        let live_bytes = self.cache_memory_estimate.load(Ordering::Acquire);
        if live_bytes > self.max_cache_bytes {
            let reclaimable = cache
                .values()
                .filter(|existing| {
                    !existing.is_dirty()
                        && !existing.is_pinned()
                        && Arc::strong_count(existing) == 1
                })
                .map(|existing| existing.estimated_bytes.load(Ordering::Relaxed))
                .fold(0usize, usize::saturating_add);
            if live_bytes.saturating_sub(reclaimable) > self.max_cache_bytes {
                return Err(WorldStorageError::Capacity(format!(
                    "column {key:?} rejected: pinned/dirty live bytes leave no space under the configured {} byte limit",
                    self.max_cache_bytes
                )));
            }
        }

        loop {
            let reserved_slots = self.pending_cache_slots.load(Ordering::Acquire);
            let slots_full = cache.len().saturating_add(reserved_slots) >= self.max_cache_entries;
            let bytes_full =
                self.cache_memory_estimate.load(Ordering::Acquire) > self.max_cache_bytes;
            if !slots_full && !bytes_full {
                self.pending_cache_slots.fetch_add(1, Ordering::AcqRel);
                return Ok(CacheSlotReservation {
                    pending: Arc::clone(&self.pending_cache_slots),
                    active: true,
                });
            }

            let mut removed = false;
            let attempts = order.len();
            for _ in 0..attempts {
                let Some(oldest) = order.pop_front() else {
                    break;
                };
                let Some(existing) = cache.get(&oldest) else {
                    continue;
                };
                if existing.is_dirty() || existing.is_pinned() {
                    order.push_back(oldest);
                    continue;
                }
                let external_owner = Arc::strong_count(existing) > 1;
                let bytes_full =
                    self.cache_memory_estimate.load(Ordering::Acquire) > self.max_cache_bytes;
                if bytes_full && external_owner {
                    // Moving an externally owned column to `retired` frees a
                    // resident slot but not its live bytes. Do not evict such
                    // columns while solving byte pressure alone.
                    order.push_back(oldest);
                    continue;
                }
                if external_owner
                    && (retired.len() >= MAX_RETIRED_COLUMN_HANDLES
                        || retired.contains_key(&oldest))
                {
                    order.push_back(oldest);
                    continue;
                }
                if let Some(existing) = cache.remove(&oldest) {
                    if external_owner {
                        // Keep a bounded canonical handle while external code
                        // may still mutate the column through its Arc.
                        retired.insert(oldest, existing);
                    } else {
                        existing.unaccount_memory();
                    }
                }
                removed = true;
                break;
            }
            if removed {
                continue;
            }

            return Err(WorldStorageError::Capacity(format!(
                "column {key:?} rejected: resident entries or estimated live bytes are at the configured limit; dirty, pinned, and externally owned columns were retained"
            )));
        }
    }

    /// Insert an already prepared column while the caller holds the key stripe.
    /// Pending writes are applied before publication, without nesting the
    /// pending/cache locks with the target column lock.
    fn insert_column_cached_locked(
        &self,
        key: ChunkKey,
        column: Arc<ChunkColumn>,
        reservation: Option<CacheSlotReservation>,
    ) -> Result<Arc<ChunkColumn>, WorldStorageError> {
        if let Some(pin) = self.pin_cached_chunk(key) {
            let pending = self.take_pending_spillover(key);
            Self::apply_pending_ordered_to_column(&pin.0, &pending, false, false, false);
            return Ok(Arc::clone(&pin.0));
        }

        let mut reservation = match reservation {
            Some(reservation) => reservation,
            None => self.reserve_cache_slot(key, &column)?,
        };

        // All insertion routes hold this key's spillover stripe. Recheck after
        // reserving before consuming pending writes into the private candidate.
        if let Some(pin) = self.pin_cached_chunk(key) {
            let pending = self.take_pending_spillover(key);
            Self::apply_pending_ordered_to_column(&pin.0, &pending, false, false, false);
            return Ok(Arc::clone(&pin.0));
        }

        let pending = self.take_pending_spillover(key);
        Self::apply_pending_ordered_to_column(&column, &pending, false, false, false);
        let mut cache = self
            .cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        debug_assert!(
            !cache.contains_key(&key),
            "same-key publication must be serialized by its spillover stripe"
        );

        let mut order = self
            .cache_order
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        column.attach_dirty_index(key, &self.dirty_index);
        cache.insert(key, column.clone());
        order.push_back(key);
        reservation.commit();
        Ok(column)
    }

    /// Route a spillover batch with per-key ordering and all-or-nothing
    /// admission. A batch that cannot fit the pending budget is rejected as a
    /// whole before any cached target is changed.
    #[cfg(test)]
    fn route_spillover(&self, writes: Vec<BlockSpilloverWrite>) -> Result<(), WorldStorageError> {
        if writes.is_empty() {
            return Ok(());
        }
        static NEXT_TEST_ORDINAL: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(1 << 31);
        let base = NEXT_TEST_ORDINAL.fetch_add(writes.len() as u32, Ordering::Relaxed);
        let writes = writes
            .into_iter()
            .enumerate()
            .map(|(ordinal, write)| SpilloverJournalEntry {
                operation_id: SpilloverOperationId::new(write.key, base + ordinal as u32),
                write,
            })
            .collect::<Vec<_>>();
        let lock_set = Self::spillover_lock_set(writes.iter().map(|entry| entry.write.key));
        self.with_spillover_key_locks(lock_set, || {
            self.route_spillover_locked(writes, None, None).map(|_| ())
        })
    }

    /// Caller holds the stripes for every target and optional `drain_key`.
    /// Returns the writes that must be applied to a not-yet-published source
    /// column. Lock order is key stripes → cache → retired → pending →
    /// (released) column.
    fn route_spillover_locked(
        &self,
        writes: Vec<SpilloverJournalEntry>,
        drain_key: Option<ChunkKey>,
        generated_source: Option<&ChunkColumn>,
    ) -> Result<(Vec<SpilloverJournalEntry>, Option<GeneratedChunkCommit>), WorldStorageError> {
        let writes = merge_spillover_entries(writes)?;
        if writes.is_empty() && drain_key.is_none() {
            return Ok((Vec::new(), None));
        }
        let incoming_bytes = writes
            .len()
            .saturating_mul(SPILLOVER_ESTIMATED_BYTES_PER_WRITE);
        if writes.len() > MAX_PENDING_SPILLOVER_WRITES
            || incoming_bytes > MAX_PENDING_SPILLOVER_BYTES
        {
            return Err(WorldStorageError::Backend(format!(
                "spillover batch rejected atomically: {} writes / {incoming_bytes} estimated bytes exceed the per-result limit ({MAX_PENDING_SPILLOVER_WRITES} writes / {MAX_PENDING_SPILLOVER_BYTES} bytes)",
                writes.len(),
            )));
        }

        let mut grouped: HashMap<ChunkKey, Vec<SpilloverJournalEntry>> = HashMap::new();
        for entry in &writes {
            grouped.entry(entry.write.key).or_default().push(*entry);
        }

        // Acquire short-lived target handles/pins under the cache read lock;
        // actual chunk writes happen after both global cache/pending locks drop.
        let cache = self
            .cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let retired = self
            .retired
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut direct: Vec<(ChunkKey, ChunkColumnPin, SpilloverPartition)> = Vec::new();
        let mut pending_groups = Vec::new();
        let mut source_writes = Vec::new();
        for (key, entries) in grouped {
            if let Some(column) = cache.get(&key).or_else(|| retired.get(&key)) {
                // Arbitrate before touching the column: positions inside the
                // bounded arbitration window are applied now (largest operation id
                // wins), and positions outside it are re-queued so this column
                // applies them in operation-id order when it is next materialized.
                let fresh = column.unapplied_spillover_entries(&entries);
                let partition = column.preview_spillover_conflicts(&fresh);
                if partition.superseded > 0 || partition.untracked > 0 {
                    report_spillover_arbitration(
                        Some(key),
                        partition.superseded,
                        partition.untracked,
                    );
                }
                if !partition.deferred.is_empty() {
                    pending_groups.push((key, partition.deferred));
                }
                if !partition.applicable.is_empty() {
                    direct.push((
                        key,
                        ChunkColumn::pin_for_spillover(column),
                        SpilloverPartition {
                            applicable: partition.applicable,
                            deferred: Vec::new(),
                            superseded: 0,
                            untracked: partition.untracked,
                        },
                    ));
                }
            } else if Some(key) == drain_key {
                source_writes.extend(entries);
            } else {
                pending_groups.push((key, entries));
            }
        }

        let mut pending = self
            .pending_spillover
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut existing_operations = HashMap::new();
        for batch in pending.values() {
            for entry in &batch.writes {
                existing_operations.insert(entry.operation_id, *entry);
            }
        }
        for entry in &writes {
            if let Some(existing) = existing_operations.get(&entry.operation_id) {
                if existing != entry {
                    return Err(WorldStorageError::Corrupt(format!(
                        "spillover operation id reused with different payload: {:?}",
                        entry.operation_id
                    )));
                }
            }
        }
        let mut fresh_pending_groups = Vec::new();
        for (key, entries) in pending_groups {
            let fresh = entries
                .into_iter()
                .filter(|entry| !existing_operations.contains_key(&entry.operation_id))
                .collect::<Vec<_>>();
            if !fresh.is_empty() {
                fresh_pending_groups.push((key, fresh));
            }
        }
        let pending_to_drain = drain_key
            .and_then(|key| pending.get(&key).map(|batch| batch.writes.len()))
            .unwrap_or(0);
        let current = self.pending_spillover_count.load(Ordering::Relaxed);
        let requested = fresh_pending_groups
            .iter()
            .map(|(_, entries)| entries.len())
            .sum::<usize>();
        let requested_bytes = requested.saturating_mul(SPILLOVER_ESTIMATED_BYTES_PER_WRITE);
        let resulting_count = current.saturating_add(requested);
        let current_bytes = self.pending_spillover_bytes.load(Ordering::Relaxed);
        let resulting_bytes = current_bytes.saturating_add(requested_bytes);
        let source_result_count = pending_to_drain.saturating_add(source_writes.len());
        if resulting_count > MAX_PENDING_SPILLOVER_WRITES
            || resulting_bytes > MAX_PENDING_SPILLOVER_BYTES
            || source_result_count > MAX_PENDING_SPILLOVER_WRITES
            || source_result_count.saturating_mul(SPILLOVER_ESTIMATED_BYTES_PER_WRITE)
                > MAX_PENDING_SPILLOVER_BYTES
        {
            return Err(WorldStorageError::Backend(format!(
                "spillover batch rejected atomically: pending/result budget would be exceeded (pending={resulting_count}/{resulting_bytes} bytes, source={source_result_count} writes)"
            )));
        }

        let reserved_pending = fresh_pending_groups
            .iter()
            .flat_map(|(_, entries)| entries.iter().copied())
            .collect::<Vec<_>>();
        for (key, entries) in fresh_pending_groups {
            pending
                .entry(key)
                .or_insert_with(|| PendingSpilloverBatch {
                    writes: Vec::new(),
                    first_enqueued_at: Instant::now(),
                })
                .writes
                .extend(entries);
        }
        self.pending_spillover_count
            .fetch_add(requested, Ordering::Relaxed);
        self.pending_spillover_bytes
            .fetch_add(requested_bytes, Ordering::Release);
        drop(pending);
        drop(retired);
        drop(cache);

        // Journal persistence is outside the cache and pending locks. The
        // temporary pending reservation is rolled back if the DB owner rejects
        // the append, so a failed write never becomes a visible world fact.
        let mut generation_commit = None;
        if self.storage.supports_spillover_journal() {
            let result = if let Some(column) = generated_source {
                let snapshot = {
                    let chunk = column.read();
                    if chunk.estimated_writeback_snapshot_bytes() > MAX_SYNC_FLUSH_SNAPSHOT_BYTES {
                        Err(WorldStorageError::Capacity(
                            "generated source snapshot exceeds its byte limit".into(),
                        ))
                    } else {
                        Ok(chunk.clone())
                    }
                };
                snapshot
                    .and_then(|snapshot| {
                        self.storage.commit_generated_chunk(
                            drain_key.expect("generated source key"),
                            snapshot,
                            &writes,
                        )
                    })
                    .map(|receipt| {
                        generation_commit = Some(receipt);
                    })
            } else {
                self.storage.append_spillover_journal(&writes)
            };
            let duplicate = generation_commit
                .as_ref()
                .is_some_and(|receipt| !receipt.newly_committed);
            if result.is_err() || duplicate {
                let mut pending = self
                    .pending_spillover
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for entry in &reserved_pending {
                    let key = entry.write.key;
                    if let Some(batch) = pending.get_mut(&key) {
                        batch
                            .writes
                            .retain(|current| current.operation_id != entry.operation_id);
                        if batch.writes.is_empty() {
                            pending.remove(&key);
                        }
                    }
                }
                self.pending_spillover_count
                    .fetch_sub(requested, Ordering::Relaxed);
                self.pending_spillover_bytes
                    .fetch_sub(requested_bytes, Ordering::Release);
                if let Err(error) = result {
                    return Err(error);
                }
                direct.clear();
                source_writes.clear();
            }
        }

        let mut pending = self
            .pending_spillover
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(key) = drain_key {
            if let Some(mut queued) = pending.remove(&key) {
                self.pending_spillover_count
                    .fetch_sub(queued.writes.len(), Ordering::Relaxed);
                self.pending_spillover_bytes.fetch_sub(
                    queued
                        .writes
                        .len()
                        .saturating_mul(SPILLOVER_ESTIMATED_BYTES_PER_WRITE),
                    Ordering::Release,
                );
                queued.writes.append(&mut source_writes);
                source_writes = merge_spillover_entries(queued.writes)?;
            }
        }
        drop(pending);

        // Per-key stripes plus the temporary pin preserve target ordering and
        // keep columns resident, without holding global cache/pending locks.
        //
        // Owner-side validate: the stripe lock already serializes publication
        // for every routed key, so the pinned column must still be the
        // authoritative instance. A mismatch would write a replaced column, so
        // the whole batch fails loudly instead of silently landing on a stale
        // instance (the caller retries against the new incarnation).
        for (key, column, partition) in direct {
            self.validate_spillover_target(key, &column)?;
            // Commit the arbitration winners only now: the batch is certain to be
            // applied, so a rejected batch can never reserve state it did not
            // publish.
            column
                .0
                .commit_spillover_conflicts(&partition.applicable, 0);
            Self::apply_partition_to_column_with_metadata(
                &column.0, &partition, false, false, false,
            );
        }
        Ok((source_writes, generation_commit))
    }

    /// Confirm a routed spillover target is still the published column for `key`.
    fn validate_spillover_target(
        &self,
        key: ChunkKey,
        pinned: &ChunkColumnPin,
    ) -> Result<(), WorldStorageError> {
        let cached = self
            .cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
            .is_some_and(|column| Arc::ptr_eq(column, &pinned.0));
        if cached {
            return Ok(());
        }
        let retired = self
            .retired
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
            .is_some_and(|column| Arc::ptr_eq(column, &pinned.0));
        if retired {
            return Ok(());
        }
        self.replaced_spillover_targets
            .fetch_add(1, Ordering::Relaxed);
        Err(WorldStorageError::Backend(format!(
            "spillover target {key:?} was replaced after routing; refusing to write a stale column"
        )))
    }

    /// Apply a spillover batch with arbitration (used by routing and tests).
    ///
    /// Writes whose position is outside the column's bounded arbitration window
    /// are **not** resolved here; the routing path re-queues them so the owning
    /// column applies them in operation-id order when it is next materialized.
    fn apply_writes_to_column(column: &ChunkColumn, writes: &[SpilloverJournalEntry]) {
        Self::apply_writes_to_column_with_metadata(column, writes, false, false, false);
    }

    fn apply_writes_to_column_with_metadata(
        column: &ChunkColumn,
        writes: &[SpilloverJournalEntry],
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
    ) {
        let target_key = writes.first().map(|entry| entry.write.key);
        let fresh = column.unapplied_spillover_entries(writes);
        let partition = column.preview_spillover_conflicts(&fresh);
        if partition.superseded > 0 || partition.untracked > 0 {
            report_spillover_arbitration(target_key, partition.superseded, partition.untracked);
        }
        column.commit_spillover_conflicts(&partition.applicable, partition.untracked);
        column.note_superseded(partition.superseded);
        Self::apply_partition_to_column_with_metadata(
            column,
            &partition,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
        );
    }

    /// Apply writes the owning column was re-queued for, in operation-id order.
    ///
    /// This is the owner-side absorption path: the batch was deferred precisely
    /// because it could not be arbitrated against a bounded window, so it is
    /// applied unconditionally (idempotency still comes from the recorded
    /// operation ids).
    fn apply_pending_ordered_to_column(
        column: &ChunkColumn,
        writes: &[SpilloverJournalEntry],
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
    ) {
        let pending = merge_spillover_entries(writes.iter().copied()).unwrap_or_default();
        let applicable = column.unapplied_spillover_entries(&pending);
        column.commit_spillover_conflicts(&applicable, 0);
        if applicable.is_empty() && !block_entities_dirty && !biomes_dirty && !heightmap_dirty {
            return;
        }
        Self::apply_partition_to_column_with_metadata(
            column,
            &SpilloverPartition {
                applicable,
                ..SpilloverPartition::default()
            },
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
        );
    }

    /// Apply an already arbitrated partition.
    ///
    /// `forced` skips arbitration: the owning column applies everything it was
    /// handed, which is exactly the pending-absorption path (the writes were
    /// re-queued precisely so they would be applied in operation-id order).
    fn apply_partition_to_column_with_metadata(
        column: &ChunkColumn,
        partition: &SpilloverPartition,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
    ) {
        let writes = &partition.applicable;
        if writes.is_empty() && !block_entities_dirty && !biomes_dirty && !heightmap_dirty {
            return;
        }
        let mut dirty_subchunks = BTreeSet::new();
        let mut chunk = column.write();
        let operation_ids = writes
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>();
        for entry in writes {
            let write = entry.write;
            if chunk
                .set_block_at(
                    write.layer,
                    (write.x & 0xF) as u8,
                    write.y,
                    (write.z & 0xF) as u8,
                    write.block,
                )
                .is_some()
            {
                dirty_subchunks.insert((write.y.div_euclid(SUBCHUNK_SIZE)) as i8);
            }
        }
        // Publish content generation and dirty state before releasing the
        // write guard, so a concurrent writeback snapshot cannot miss this
        // applied spillover batch.
        column.mark_dirty_with_metadata_locked_and_spillover(
            &chunk,
            dirty_subchunks,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            &operation_ids,
        );
    }

    pub fn cached_chunk(&self, key: ChunkKey) -> Option<Arc<ChunkColumn>> {
        let cache = self
            .cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(column) = cache.get(&key) {
            // A returned Arc itself prevents unsafe clean eviction: eviction
            // observes the extra owner and moves the column into `retired`.
            return Some(Arc::clone(column));
        }

        let mut retired = self
            .retired
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let discard = retired
            .get(&key)
            .is_some_and(|column| !column.is_dirty() && Arc::strong_count(column) == 1);
        if discard {
            if let Some(column) = retired.remove(&key) {
                column.unaccount_memory();
            }
            return None;
        }
        retired.get(&key).cloned()
    }

    pub fn invalidate(&self, key: ChunkKey) {
        let lock_set = Self::spillover_lock_set(std::iter::once(key));
        self.with_spillover_key_locks(lock_set, || self.invalidate_locked(key));
    }

    fn invalidate_locked(&self, key: ChunkKey) {
        let Ok(mut cache) = self.cache.write() else {
            return;
        };
        let Ok(mut order) = self.cache_order.write() else {
            return;
        };
        let Ok(mut retired) = self.retired.write() else {
            return;
        };
        let externally_owned = cache
            .get(&key)
            .or_else(|| retired.get(&key))
            .is_some_and(|column| Arc::strong_count(column) > 1);
        let protected = cache
            .get(&key)
            .or_else(|| retired.get(&key))
            .is_some_and(|column| column.is_dirty() || column.is_pinned());
        if externally_owned || protected {
            log::warn!(
                "{}",
                t_log!(
                    "console.world.invalidate_refused",
                    x = key.position.x,
                    y = key.position.z
                )
            );
            return;
        }
        cache.remove(&key);
        order.retain(|existing| existing != &key);
        if let Some(column) = retired.remove(&key) {
            column.unaccount_memory();
        }
    }

    pub fn clear_cache(&self) {
        let Ok(mut cache) = self.cache.write() else {
            return;
        };
        let Ok(mut order) = self.cache_order.write() else {
            return;
        };
        let Ok(mut retired) = self.retired.write() else {
            return;
        };
        let protected = cache
            .values()
            .filter(|column| {
                column.is_dirty() || column.is_pinned() || Arc::strong_count(column) > 1
            })
            .count();
        if protected > 0 {
            log::warn!(
                "{}",
                t_log!("console.world.cache_retain", count = protected)
            );
            cache.retain(|_, column| {
                column.is_dirty() || column.is_pinned() || Arc::strong_count(column) > 1
            });
        } else {
            cache.clear();
        }
        order.retain(|key| cache.contains_key(key));
        retired.retain(|_, column| {
            if column.is_dirty() || column.is_pinned() || Arc::strong_count(column) > 1 {
                true
            } else {
                column.unaccount_memory();
                false
            }
        });
    }

    pub fn cache_memory_bytes(&self) -> usize {
        self.cache_memory_estimate.load(Ordering::Relaxed)
    }

    /// Estimated bytes retained in pending spillover batches. This is a
    /// bounded admission estimate, not allocator/RSS accounting.
    pub fn pending_spillover_estimated_bytes(&self) -> usize {
        self.pending_spillover_bytes.load(Ordering::Acquire)
    }

    /// Age of the oldest pending target bucket, if any. This is diagnostic
    /// only: callers must not discard accepted spillover merely because it is
    /// old; recovery requires the durable journal stage.
    pub fn pending_spillover_oldest_age(&self) -> Option<Duration> {
        let now = Instant::now();
        self.pending_spillover
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .map(|batch| now.saturating_duration_since(batch.first_enqueued_at))
            .max()
    }

    /// Estimated bytes reserved by concurrent workers constructing bounded
    /// generation spillover results.
    pub fn spillover_generation_reserved_bytes(&self) -> usize {
        self.spillover_generation_reserved_bytes
            .load(Ordering::Acquire)
    }

    pub fn refresh_cache_memory_bytes(&self) -> usize {
        let Ok(cache) = self.cache.read() else {
            return self.cache_memory_estimate.load(Ordering::Relaxed);
        };
        let Ok(retired) = self.retired.read() else {
            return self.cache_memory_estimate.load(Ordering::Relaxed);
        };
        let total = cache
            .values()
            .chain(retired.values())
            .map(|column| column.refresh_memory_estimate())
            .sum();
        self.cache_memory_estimate.store(total, Ordering::Relaxed);
        total
    }

    pub fn cache_len(&self) -> usize {
        self.cache.read().map(|cache| cache.len()).unwrap_or(0)
    }

    /// Compatibility wrapper returning the number of acknowledged saves.
    pub fn flush_dirty(&self) -> usize {
        self.flush_dirty_report().saved
    }

    /// Synchronously persist snapshots of dirty resident and retired
    /// columns. This is the shutdown fallback; ordinary dirty writeback is
    /// scheduled by `ChunkWritebackExecutor`. Shutdown processes a fixed-size
    /// page at a time and retains only bounded key samples in its report.
    pub fn flush_dirty_report(&self) -> DirtyFlushReport {
        self.flush_dirty_report_with_snapshot_limit(MAX_SYNC_FLUSH_SNAPSHOT_BYTES)
    }

    fn flush_dirty_report_with_snapshot_limit(
        &self,
        max_snapshot_bytes: usize,
    ) -> DirtyFlushReport {
        let mut report = DirtyFlushReport::default();
        // Bound this flush pass to keys present when shutdown fallback starts.
        // New dirty keys created while storage I/O is blocked remain visible
        // in `remaining_dirty` instead of extending shutdown indefinitely.
        let high_water_mark = self.dirty_index.last_key();
        let mut cursor = None;
        while high_water_mark.is_some_and(|high| cursor.is_none_or(|cursor| cursor < high)) {
            let (last_examined, dirty) = match self.dirty_columns_after(
                cursor,
                high_water_mark,
                MAX_SYNC_FLUSH_BATCH_COLUMNS,
            ) {
                Ok(batch) => batch,
                Err(()) => {
                    report.scan_failed = true;
                    break;
                }
            };
            let Some(last_examined) = last_examined else {
                break;
            };
            cursor = Some(last_examined);

            for (key, column) in dirty {
                // Capture content, generation and dirty kinds while holding
                // the same content/dirty locks. Release both before storage IO.
                let snapshot = {
                    let chunk = column.read();
                    let (
                        generation,
                        block_entities_dirty,
                        biomes_dirty,
                        heightmap_dirty,
                        is_dirty,
                        spillover_acks,
                    ) = column.writeback_state_with_spillover();
                    if !is_dirty {
                        Ok(None)
                    } else if column.snapshot_estimated_bytes(&chunk) > max_snapshot_bytes {
                        Err(())
                    } else {
                        Ok(Some((
                            generation,
                            block_entities_dirty,
                            biomes_dirty,
                            heightmap_dirty,
                            spillover_acks,
                            chunk.clone(),
                        )))
                    }
                };
                let snapshot = match snapshot {
                    Err(()) => {
                        report.attempted += 1;
                        report.record_oversize(key);
                        continue;
                    }
                    Ok(snapshot) => snapshot,
                };
                let Some((
                    generation,
                    block_entities_dirty,
                    biomes_dirty,
                    heightmap_dirty,
                    spillover_acks,
                    snapshot,
                )) = snapshot
                else {
                    continue;
                };
                report.attempted += 1;
                if let Err(error) = self.storage.save_chunk_owned_with_metadata_and_spillover(
                    key,
                    snapshot,
                    block_entities_dirty,
                    biomes_dirty,
                    heightmap_dirty,
                    &spillover_acks,
                ) {
                    log::warn!(
                        "{}",
                        t_log!(
                            "console.world.dirty_save_fail",
                            pos = key.position,
                            error = error
                        )
                    );
                    report.record_failure(key);
                    continue;
                }
                if column.take_dirty_if_generation_and_spillover(generation, Some(&spillover_acks))
                {
                    report.saved += 1;
                } else {
                    log::debug!(
                        "[world] chunk changed while saving; retaining dirty markers at {}",
                        key.position
                    );
                    report.record_changed(key);
                }
            }
        }
        match self.count_dirty_columns() {
            Ok(remaining) => report.remaining_dirty = Some(remaining),
            Err(()) => {
                report.scan_failed = true;
                report.remaining_dirty = None;
            }
        }
        if report.oversize_count != 0 {
            log::warn!(
                "{}",
                t_log!(
                    "console.world.oversize_retained",
                    count = report.oversize_count,
                    max = max_snapshot_bytes
                )
            );
        }
        report
    }
}

/// Empty storage is useful for worlds that are intentionally generator-only.
#[derive(Default)]
pub struct EmptyWorldStorage;

impl WorldStorage for EmptyWorldStorage {
    fn load_chunk(&self, _key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
        Ok(None)
    }
}

/// Small in-memory backend used by plugins and by future storage tests.
#[derive(Default)]
pub struct InMemoryWorldStorage {
    chunks: RwLock<HashMap<ChunkKey, Chunk>>,
    spillover_journal: RwLock<MemorySpilloverJournal>,
}

#[derive(Default)]
struct MemorySpilloverJournal {
    targets: HashMap<ChunkKey, BTreeMap<SpilloverOperationId, SpilloverJournalEntry>>,
    total: usize,
}

impl InMemoryWorldStorage {
    pub fn insert(&self, chunk: Chunk) {
        let key = ChunkKey::new(chunk.dimension, chunk.position);
        if let Ok(mut chunks) = self.chunks.write() {
            chunks.insert(key, chunk);
        }
    }
}

impl WorldStorage for InMemoryWorldStorage {
    fn load_chunk(&self, key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
        let chunks = self
            .chunks
            .read()
            .map_err(|_| WorldStorageError::Backend("in-memory chunks poisoned".into()))?;
        Ok(chunks.get(&key).cloned())
    }

    fn load_chunk_with_spillover(
        &self,
        key: ChunkKey,
    ) -> Result<(Option<Chunk>, Vec<SpilloverJournalEntry>), WorldStorageError> {
        let chunks = self
            .chunks
            .read()
            .map_err(|_| WorldStorageError::Backend("in-memory chunks poisoned".into()))?;
        Ok((chunks.get(&key).cloned(), self.load_spillover_journal(key)?))
    }

    fn commit_generated_chunk(
        &self,
        key: ChunkKey,
        chunk: Chunk,
        outgoing: &[SpilloverJournalEntry],
    ) -> Result<GeneratedChunkCommit, WorldStorageError> {
        let mut chunks = self
            .chunks
            .write()
            .map_err(|_| WorldStorageError::Backend("in-memory chunks poisoned".into()))?;
        if let Some(existing) = chunks.get(&key) {
            return Ok(GeneratedChunkCommit {
                source: existing.clone(),
                incoming: self.load_spillover_journal(key)?,
                newly_committed: false,
            });
        }
        if outgoing
            .iter()
            .any(|entry| entry.operation_id.source != key)
        {
            return Err(WorldStorageError::Corrupt(
                "spillover source identity mismatch".into(),
            ));
        }
        self.append_spillover_journal(outgoing)?;
        chunks.insert(key, chunk.clone());
        Ok(GeneratedChunkCommit {
            source: chunk,
            incoming: self.load_spillover_journal(key)?,
            newly_committed: true,
        })
    }

    fn save_chunk(&self, key: ChunkKey, chunk: &Chunk) -> Result<(), WorldStorageError> {
        self.save_chunk_owned(key, chunk.clone())
    }

    fn save_chunk_owned(&self, key: ChunkKey, chunk: Chunk) -> Result<(), WorldStorageError> {
        self.chunks
            .write()
            .map_err(|_| WorldStorageError::Backend("in-memory chunks poisoned".into()))?
            .insert(key, chunk);
        Ok(())
    }

    fn supports_spillover_journal(&self) -> bool {
        true
    }

    fn load_spillover_journal(
        &self,
        key: ChunkKey,
    ) -> Result<Vec<SpilloverJournalEntry>, WorldStorageError> {
        let journal = self
            .spillover_journal
            .read()
            .map_err(|_| WorldStorageError::Backend("in-memory journal poisoned".into()))?;
        Ok(journal
            .targets
            .get(&key)
            .map(|entries| entries.values().copied().collect())
            .unwrap_or_default())
    }

    fn append_spillover_journal(
        &self,
        entries: &[SpilloverJournalEntry],
    ) -> Result<(), WorldStorageError> {
        if entries.len() > MAX_GENERATED_SPILLOVER_WRITES {
            return Err(WorldStorageError::Capacity(
                "spillover journal batch limit exceeded".into(),
            ));
        }
        let entries = merge_spillover_entries(entries.iter().copied())?;
        let mut journal = self
            .spillover_journal
            .write()
            .map_err(|_| WorldStorageError::Backend("in-memory journal poisoned".to_string()))?;
        let mut additions: HashMap<ChunkKey, usize> = HashMap::new();
        for entry in &entries {
            if let Some(existing) = journal
                .targets
                .get(&entry.write.key)
                .and_then(|target| target.get(&entry.operation_id))
            {
                if existing != entry {
                    return Err(WorldStorageError::Corrupt(
                        "spillover operation id reused with different payload".to_string(),
                    ));
                }
            } else {
                *additions.entry(entry.write.key).or_default() += 1;
            }
        }
        if journal
            .total
            .saturating_add(additions.values().sum::<usize>())
            > MAX_STORED_SPILLOVER_WRITES
            || additions.iter().any(|(key, count)| {
                journal
                    .targets
                    .get(key)
                    .map_or(0, |entries| entries.len())
                    .saturating_add(*count)
                    > MAX_GENERATED_SPILLOVER_WRITES
            })
        {
            return Err(WorldStorageError::Capacity(
                "spillover journal is full".into(),
            ));
        }
        journal.total += additions.values().sum::<usize>();
        for entry in entries {
            journal
                .targets
                .entry(entry.write.key)
                .or_default()
                .insert(entry.operation_id, entry);
        }
        Ok(())
    }

    fn save_chunk_owned_with_metadata_and_spillover(
        &self,
        key: ChunkKey,
        chunk: Chunk,
        _block_entities_dirty: bool,
        _biomes_dirty: bool,
        _heightmap_dirty: bool,
        spillover_acks: &[SpilloverOperationId],
    ) -> Result<(), WorldStorageError> {
        let mut chunks = self
            .chunks
            .write()
            .map_err(|_| WorldStorageError::Backend("in-memory chunks poisoned".into()))?;
        let mut journal = self
            .spillover_journal
            .write()
            .map_err(|_| WorldStorageError::Backend("in-memory journal poisoned".to_string()))?;
        let mut removed = 0;
        if let Some(target) = journal.targets.get_mut(&key) {
            for operation_id in spillover_acks {
                if target.remove(operation_id).is_some() {
                    removed += 1;
                }
            }
            if target.is_empty() {
                journal.targets.remove(&key);
            }
        }
        journal.total -= removed;
        chunks.insert(key, chunk);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::BlockRuntimeId;
    use sc_nbt::compound::CompoundNbt;
    use sc_nbt::NbtValue;

    #[test]
    fn flush_dirty_writes_back_and_clears_dirty_flag() {
        let storage = Arc::new(InMemoryWorldStorage::default());
        let provider = WorldChunkProvider::new(storage.clone());
        let key = ChunkKey::new(0, ChunkPosition::new(1, 1));
        // ensure creates an all-air chunk and caches it.
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure");
        // flush returns 0 with no dirty chunks.
        assert_eq!(provider.flush_dirty(), 0);
        // After marking dirty, flush writes back and clears the dirty flag.
        column.mark_dirty(0);
        assert!(column.is_dirty());
        let report = provider.flush_dirty_report();
        assert_eq!(report.attempted, 1);
        assert_eq!(report.saved, 1);
        assert_eq!(report.remaining_dirty, Some(0));
        assert!(report.is_complete());
        assert!(!column.is_dirty(), "flush_dirty must clear the dirty flag");
        // Persisted to the backend store.
        assert!(storage.load_chunk(key).expect("load").is_some());
        // Flushing again finds no dirty chunks.
        assert_eq!(provider.flush_dirty(), 0);
    }

    #[test]
    fn flush_dirty_skips_clean_columns() {
        let storage = Arc::new(InMemoryWorldStorage::default());
        let provider = WorldChunkProvider::new(storage.clone());
        let key_a = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let key_b = ChunkKey::new(0, ChunkPosition::new(2, 3));
        let col_a = provider.ensure_chunk(key_a, -64, 319).expect("ensure a");
        let _col_b = provider.ensure_chunk(key_b, -64, 319).expect("ensure b");
        // Only A is dirty.
        col_a.mark_dirty(-4);
        assert_eq!(provider.flush_dirty(), 1);
        assert!(storage.load_chunk(key_a).expect("load a").is_some());
    }

    #[test]
    fn mutation_generation_is_published_before_write_guard_release() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let key = ChunkKey::new(0, ChunkPosition::new(19, -6));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        let initial_generation = column.generation();
        let reader_column = Arc::clone(&column);
        let (reader_started_tx, reader_started_rx) = std::sync::mpsc::channel();

        let mut chunk = column.write();
        assert!(chunk
            .set_block_at(0, 4, 37, 9, BlockRuntimeId(55))
            .is_some());
        column.mark_dirty_locked(&chunk, 2);
        assert_eq!(column.generation(), initial_generation + 1);
        assert!(column.is_dirty(), "dirty state is visible before unlock");

        let reader = std::thread::spawn(move || {
            reader_started_tx.send(()).expect("notify snapshot attempt");
            let chunk = reader_column.read();
            let state = reader_column.writeback_state();
            (chunk.block_at(4, 37, 9), state)
        });
        reader_started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("reader started while writer owns the guard");
        drop(chunk);

        let (block, (generation, _, _, heightmap_dirty, dirty)) =
            reader.join().expect("snapshot reader");
        assert_eq!(block, Some(BlockRuntimeId(55)));
        assert_eq!(generation, initial_generation + 1);
        assert!(heightmap_dirty);
        assert!(dirty);
    }

    #[test]
    fn flush_dirty_recovers_live_dirty_column_evicted_from_cache() {
        let storage = Arc::new(InMemoryWorldStorage::default());
        let provider = WorldChunkProvider::new(storage.clone()).with_cache_limit(1);
        let evicted_key = ChunkKey::new(0, ChunkPosition::new(12, 4));
        let resident_key = ChunkKey::new(0, ChunkPosition::new(13, 4));
        let evicted = provider
            .ensure_chunk(evicted_key, -64, 319)
            .expect("first column");
        provider
            .ensure_chunk(resident_key, -64, 319)
            .expect("evict first clean column");
        assert_eq!(
            provider.cache_len(),
            1,
            "retired column is outside resident LRU"
        );
        assert!(provider.cached_chunk(evicted_key).is_some());

        evicted.mark_dirty(-4);
        assert_eq!(provider.flush_dirty(), 1);
        assert!(!evicted.is_dirty());
        assert!(storage
            .load_chunk(evicted_key)
            .expect("load saved evicted column")
            .is_some());
    }

    #[test]
    fn dirty_retired_column_survives_after_external_arc_is_dropped() {
        let storage = Arc::new(InMemoryWorldStorage::default());
        let provider = WorldChunkProvider::new(storage.clone()).with_cache_limit(1);
        let retired_key = ChunkKey::new(0, ChunkPosition::new(14, 4));
        let resident_key = ChunkKey::new(0, ChunkPosition::new(15, 4));
        let retired = provider
            .ensure_chunk(retired_key, -64, 319)
            .expect("first column");
        provider
            .ensure_chunk(resident_key, -64, 319)
            .expect("retire clean first column");
        {
            let mut chunk = retired.write();
            assert!(chunk
                .set_block_at(0, 1, -60, 2, BlockRuntimeId(77))
                .is_some());
            retired.mark_dirty_locked(&chunk, -4);
        }
        assert_eq!(
            provider
                .dirty_columns_window(None, 4)
                .expect("retired dirty window")
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            vec![retired_key]
        );
        drop(retired);

        assert_eq!(provider.flush_dirty(), 1);
        let reloaded = storage
            .load_chunk(retired_key)
            .expect("load persisted retired column")
            .expect("retired column was written");
        assert_eq!(
            reloaded.block_at_layer(0, 1, -60, 2),
            Some(BlockRuntimeId(77))
        );
    }

    #[test]
    fn spillover_targets_retired_canonical_column_instead_of_creating_pending_copy() {
        let provider =
            WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default())).with_cache_limit(1);
        let target_key = ChunkKey::new(0, ChunkPosition::new(22, 7));
        let resident_key = ChunkKey::new(0, ChunkPosition::new(23, 7));
        let target = provider
            .ensure_chunk(target_key, -64, 319)
            .expect("target column");
        provider
            .ensure_chunk(resident_key, -64, 319)
            .expect("retire target column");
        assert_eq!(provider.cache_len(), 1);

        provider
            .route_spillover(vec![spillover_write(target_key, 88)])
            .expect("route spillover to retired column");

        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 0);
        assert_eq!(target.generation(), 1);
        assert_eq!(
            target.read().block_at_layer(0, 1, -60, 2),
            Some(BlockRuntimeId(88))
        );
    }

    #[test]
    fn dirty_column_window_rotates_after_cursor_with_bounded_result_size() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let keys = [1, 3, 5, 7].map(|x| ChunkKey::new(0, ChunkPosition::new(x, 0)));
        for key in keys {
            provider
                .ensure_chunk(key, -64, 319)
                .expect("ensure dirty candidate")
                .mark_dirty(-4);
        }

        let window = provider
            .dirty_columns_window(Some(keys[1]), 3)
            .expect("dirty window");
        let actual = window
            .iter()
            .map(|(key, _)| key.position.x)
            .collect::<Vec<_>>();
        assert_eq!(actual, vec![5, 7, 1], "window wraps after its cursor");
        assert_eq!(window.len(), 3, "result stays within the requested bound");
    }

    #[test]
    fn dirty_index_tracks_ack_clear_and_redirty() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let key = ChunkKey::new(0, ChunkPosition::new(12, -3));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");

        column.mark_dirty(-4);
        assert_eq!(
            provider
                .dirty_columns_window(None, 4)
                .expect("dirty window")
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            vec![key]
        );

        assert!(column.take_dirty_if_generation(column.generation()));
        assert!(provider
            .dirty_columns_window(None, 4)
            .expect("clean window")
            .is_empty());

        column.mark_dirty(-4);
        assert_eq!(
            provider
                .dirty_columns_window(None, 4)
                .expect("redirty window")
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            vec![key]
        );
    }

    #[test]
    fn block_entity_only_dirty_state_is_indexed_and_acknowledged() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let key = ChunkKey::new(0, ChunkPosition::new(18, 5));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        column.mark_block_entities_dirty();

        assert!(column.is_dirty());
        assert!(column.block_entities_dirty());
        assert_eq!(
            provider
                .dirty_columns_window(None, 4)
                .expect("metadata dirty window")
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            vec![key]
        );

        let stale_generation = column.generation();
        column.mark_block_entities_dirty();
        assert!(
            !column.take_dirty_if_generation(stale_generation),
            "an older save acknowledgement must not clear newer metadata dirty state"
        );
        assert!(column.is_dirty());
        assert!(column.take_dirty_if_generation(column.generation()));
        assert!(!column.is_dirty());
        assert!(!column.block_entities_dirty());
        assert!(provider
            .dirty_columns_window(None, 4)
            .expect("clean metadata window")
            .is_empty());
    }

    struct GeneratedBlockEntity;

    impl WorldGenerator for GeneratedBlockEntity {
        fn generate_chunk(
            &self,
            request: ChunkGenerationRequest,
        ) -> Result<Option<Chunk>, WorldStorageError> {
            let mut chunk = Chunk::empty(
                request.key.position,
                request.key.dimension,
                request.min_y,
                request.max_y,
            );
            let mut entity = CompoundNbt::new(None);
            entity.insert("id", NbtValue::String("minecraft:chest".into()));
            chunk.block_entities.push(NbtValue::Compound(entity));
            Ok(Some(chunk))
        }
    }

    #[test]
    fn generated_block_entity_metadata_starts_dirty_for_persistence() {
        let provider = WorldChunkProvider::new(Arc::new(EmptyWorldStorage))
            .with_generator(Arc::new(GeneratedBlockEntity));
        let key = ChunkKey::new(0, ChunkPosition::new(25, -8));
        let column = provider
            .load_chunk(key, -64, 319)
            .expect("load generated chunk")
            .expect("generated chunk exists");

        assert_eq!(column.read().block_entities.len(), 1);
        assert!(column.block_entities_dirty());
        assert!(column.is_dirty());
        assert!(provider
            .dirty_columns_window(None, 4)
            .expect("generated metadata dirty index")
            .iter()
            .any(|(candidate, _)| *candidate == key));
    }

    #[test]
    fn biome_only_dirty_state_is_indexed_and_acknowledged() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let key = ChunkKey::new(0, ChunkPosition::new(27, -4));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        column.mark_biomes_dirty();

        let (generation, block_entities_dirty, biomes_dirty, heightmap_dirty, is_dirty) =
            column.writeback_state();
        assert!(is_dirty);
        assert!(!block_entities_dirty);
        assert!(biomes_dirty);
        assert!(!heightmap_dirty);
        assert!(provider
            .dirty_columns_window(None, 4)
            .expect("biome dirty index")
            .iter()
            .any(|(candidate, _)| *candidate == key));
        assert!(column.take_dirty_if_generation(generation));
        assert!(!column.is_dirty());
        assert!(!column.biomes_dirty());
    }

    #[test]
    fn dirty_index_remains_consistent_under_concurrent_mark_and_ack() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let key = ChunkKey::new(0, ChunkPosition::new(-9, 14));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        let writer_column = Arc::clone(&column);
        let writer = std::thread::spawn(move || {
            for _ in 0..2_000 {
                writer_column.mark_dirty(-4);
            }
        });
        let ack_column = Arc::clone(&column);
        let acknowledger = std::thread::spawn(move || {
            for _ in 0..2_000 {
                let generation = ack_column.generation();
                ack_column.take_dirty_if_generation(generation);
            }
        });
        writer.join().expect("writer thread");
        acknowledger.join().expect("acknowledger thread");

        let indexed = provider
            .dirty_columns_window(None, 4)
            .expect("dirty window");
        assert_eq!(
            indexed.iter().any(|(candidate, _)| *candidate == key),
            column.is_dirty(),
            "dirty markers and the provider index agree after concurrent transitions"
        );
    }

    #[test]
    fn retired_clean_handle_budget_is_bounded_and_reclaims_released_columns() {
        let provider =
            WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default())).with_cache_limit(1);
        let mut held = Vec::new();
        for index in 0..=MAX_RETIRED_COLUMN_HANDLES as i32 {
            let key = ChunkKey::new(0, ChunkPosition::new(index, 0));
            held.push(
                provider
                    .ensure_chunk(key, -64, 319)
                    .expect("ensure")
                    .clone(),
            );
        }

        assert_eq!(
            provider.retired.read().expect("retired lock").len(),
            MAX_RETIRED_COLUMN_HANDLES
        );
        assert_eq!(provider.cache_len(), 1, "resident slots stay hard-bounded");

        let rejected_key = ChunkKey::new(
            0,
            ChunkPosition::new(MAX_RETIRED_COLUMN_HANDLES as i32 + 1, 0),
        );
        assert!(matches!(
            provider.ensure_chunk(rejected_key, -64, 319),
            Err(WorldStorageError::Capacity(_))
        ));
        assert_eq!(provider.cache_len(), 1);

        let released_key = ChunkKey::new(0, ChunkPosition::new(0, 0));
        drop(held.remove(0));
        assert!(provider.cached_chunk(released_key).is_none());
        assert_eq!(
            provider.retired.read().expect("retired lock").len(),
            MAX_RETIRED_COLUMN_HANDLES - 1
        );
        provider
            .ensure_chunk(rejected_key, -64, 319)
            .expect("admission succeeds after a retired handle is released");
        assert_eq!(provider.cache_len(), 1);
    }

    #[test]
    fn flush_dirty_keeps_markers_when_storage_rejects_snapshot() {
        let provider = WorldChunkProvider::new(Arc::new(EmptyWorldStorage));
        let key = ChunkKey::new(0, ChunkPosition::new(4, 5));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure chunk");
        column.mark_dirty(-4);

        let report = provider.flush_dirty_report();
        assert_eq!(report.attempted, 1);
        assert_eq!(report.failed_keys, vec![key]);
        assert_eq!(report.remaining_dirty, Some(1));
        assert!(!report.is_complete());
        assert!(
            column.is_dirty(),
            "failed persistence must retain dirty data"
        );
    }

    #[test]
    fn shutdown_fallback_rejects_oversized_snapshot_without_clearing_dirty() {
        let storage = Arc::new(InMemoryWorldStorage::default());
        let provider = WorldChunkProvider::new(storage.clone());
        let key = ChunkKey::new(0, ChunkPosition::new(31, -8));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        let base_bytes = column.writeback_snapshot_estimated_bytes();
        let payload = vec![4i8; 128 * 1024];
        {
            let mut chunk = column.write();
            chunk
                .block_entities
                .push(NbtValue::List(vec![NbtValue::ByteArray(payload)]));
            column.mark_dirty_with_metadata_locked(
                &chunk,
                std::iter::empty::<i8>(),
                true,
                false,
                false,
            );
        }

        let report = provider.flush_dirty_report_with_snapshot_limit(base_bytes + 64 * 1024);
        assert_eq!(report.attempted, 1);
        assert_eq!(report.oversize_count, 1);
        assert_eq!(report.failed_count, 1);
        assert_eq!(report.remaining_dirty, Some(1));
        assert!(!report.is_complete());
        assert!(column.is_dirty(), "oversize rejection preserves dirty data");
        assert!(storage.load_chunk(key).expect("load").is_none());
    }

    #[test]
    fn shutdown_flush_pages_dirty_columns_and_caps_failure_key_samples() {
        let provider = WorldChunkProvider::new(Arc::new(EmptyWorldStorage));
        let total_dirty = MAX_DIRTY_FLUSH_KEY_SAMPLES + 17;
        for index in 0..total_dirty as i32 {
            let key = ChunkKey::new(0, ChunkPosition::new(index, -2));
            provider
                .ensure_chunk(key, -64, 319)
                .expect("ensure column")
                .mark_dirty(0);
        }

        let report = provider.flush_dirty_report();
        assert_eq!(report.attempted, total_dirty);
        assert_eq!(report.saved, 0);
        assert_eq!(report.failed_count, total_dirty);
        assert_eq!(report.failed_keys.len(), MAX_DIRTY_FLUSH_KEY_SAMPLES);
        assert!(report.key_samples_truncated);
        assert_eq!(report.remaining_dirty, Some(total_dirty));
        assert!(!report.is_complete());
    }

    struct AppendDirtyColumnOnceStorage {
        provider: Mutex<Option<std::sync::Weak<WorldChunkProvider>>>,
        inserted: AtomicBool,
    }

    impl WorldStorage for AppendDirtyColumnOnceStorage {
        fn load_chunk(&self, _key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
            Ok(None)
        }

        fn save_chunk_owned(&self, key: ChunkKey, _chunk: Chunk) -> Result<(), WorldStorageError> {
            if !self.inserted.swap(true, Ordering::AcqRel) {
                if let Some(provider) = self
                    .provider
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                    .and_then(std::sync::Weak::upgrade)
                {
                    let next_key = ChunkKey::new(
                        key.dimension,
                        ChunkPosition::new(key.position.x + 1, key.position.z),
                    );
                    provider
                        .ensure_chunk(next_key, -64, 319)
                        .map_err(|error| WorldStorageError::Backend(error.to_string()))?
                        .mark_dirty(0);
                }
            }
            Ok(())
        }
    }

    #[test]
    fn shutdown_flush_does_not_chase_new_dirty_keys_past_its_high_water_mark() {
        let storage = Arc::new(AppendDirtyColumnOnceStorage {
            provider: Mutex::new(None),
            inserted: AtomicBool::new(false),
        });
        let provider = Arc::new(WorldChunkProvider::new(storage.clone()));
        *storage.provider.lock().expect("provider weak lock") = Some(Arc::downgrade(&provider));
        let key = ChunkKey::new(0, ChunkPosition::new(30, 2));
        provider
            .ensure_chunk(key, -64, 319)
            .expect("ensure initial column")
            .mark_dirty(0);

        let report = provider.flush_dirty_report();
        assert_eq!(report.attempted, 1);
        assert_eq!(report.saved, 1);
        assert_eq!(report.remaining_dirty, Some(1));
        assert!(!report.is_complete());
    }

    struct BlockingSaveStorage {
        save_started: std::sync::mpsc::Sender<BlockRuntimeId>,
        release_save: Mutex<std::sync::mpsc::Receiver<()>>,
        saved: Mutex<Vec<BlockRuntimeId>>,
    }

    impl WorldStorage for BlockingSaveStorage {
        fn load_chunk(&self, _key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
            Ok(None)
        }

        fn save_chunk(&self, _key: ChunkKey, chunk: &Chunk) -> Result<(), WorldStorageError> {
            let block = chunk
                .block_at_layer(0, 1, -60, 2)
                .ok_or_else(|| WorldStorageError::Backend("test snapshot missing block".into()))?;
            self.save_started
                .send(block)
                .map_err(|error| WorldStorageError::Backend(error.to_string()))?;
            self.release_save
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .recv()
                .map_err(|error| WorldStorageError::Backend(error.to_string()))?;
            self.saved
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(block);
            Ok(())
        }
    }

    #[test]
    fn flush_dirty_releases_column_lock_during_io_and_keeps_newer_dirty_generation() {
        let (save_started_tx, save_started_rx) = std::sync::mpsc::channel();
        let (release_save_tx, release_save_rx) = std::sync::mpsc::channel();
        let storage = Arc::new(BlockingSaveStorage {
            save_started: save_started_tx,
            release_save: Mutex::new(release_save_rx),
            saved: Mutex::new(Vec::new()),
        });
        let provider = WorldChunkProvider::new(storage.clone());
        let key = ChunkKey::new(0, ChunkPosition::new(8, 9));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure chunk");

        {
            let mut chunk = column.write();
            assert!(chunk
                .set_block_at(0, 1, -60, 2, BlockRuntimeId(10))
                .is_some());
            column.mark_dirty_locked(&chunk, -4);
        }
        let flushing_provider = provider.clone();
        let flush = std::thread::spawn(move || flushing_provider.flush_dirty_report());

        let saved_snapshot = save_started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("writeback reached storage");
        assert_eq!(saved_snapshot, BlockRuntimeId(10));

        let writer_column = Arc::clone(&column);
        let (write_done_tx, write_done_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            {
                let mut chunk = writer_column.write();
                assert!(chunk
                    .set_block_at(0, 1, -60, 2, BlockRuntimeId(20))
                    .is_some());
                writer_column.mark_dirty_locked(&chunk, -4);
            }
            let _ = write_done_tx.send(());
        });

        // The DB owner is deliberately blocked. The chunk writer must still
        // acquire the column lock and publish a newer generation meanwhile.
        let write_completed_during_io = write_done_rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_ok();
        release_save_tx.send(()).expect("release storage write");
        let report = flush.join().expect("flush worker");
        writer.join().expect("chunk writer");

        assert!(
            write_completed_during_io,
            "DB I/O must not hold the chunk read lock"
        );
        assert_eq!(report.saved, 0, "stale snapshot is not acknowledged");
        assert_eq!(report.changed_during_save, vec![key]);
        assert_eq!(report.remaining_dirty, Some(1));
        assert!(!report.is_complete());
        assert!(column.is_dirty(), "newer mutation must remain dirty");
        assert_eq!(
            column.read().block_at_layer(0, 1, -60, 2),
            Some(BlockRuntimeId(20))
        );
        assert_eq!(
            *storage.saved.lock().expect("saved snapshot list"),
            vec![BlockRuntimeId(10)],
            "storage receives the stable pre-mutation snapshot"
        );
    }

    #[test]
    fn pending_spillover_is_applied_after_chunk_cache_insertion() {
        let storage = Arc::new(InMemoryWorldStorage::default());
        let provider = WorldChunkProvider::new(storage);
        let key = ChunkKey::new(0, ChunkPosition::new(3, -2));
        let block = BlockRuntimeId(1);

        provider
            .route_spillover(vec![BlockSpilloverWrite {
                key,
                x: 3 * 16 + 1,
                y: -60,
                z: -2 * 16 + 2,
                layer: 0,
                block,
            }])
            .expect("admit spillover write");
        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 1);

        let column = provider
            .insert_empty_chunk(key, -64, 319)
            .expect("admit empty chunk");
        assert_eq!(column.read().block_at_layer(0, 1, -60, 2), Some(block));
        assert!(column.is_dirty());
        assert_eq!(column.generation(), 1);
        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn pending_spillover_oldest_age_survives_append_and_clears_on_drain() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let oldest_key = ChunkKey::new(0, ChunkPosition::new(3, -2));
        let later_key = ChunkKey::new(0, ChunkPosition::new(4, -2));

        provider
            .route_spillover(vec![spillover_write(oldest_key, 1)])
            .expect("admit first target write");
        std::thread::sleep(Duration::from_millis(20));
        provider
            .route_spillover(vec![spillover_write(oldest_key, 2)])
            .expect("append to first target bucket");
        provider
            .route_spillover(vec![spillover_write(later_key, 3)])
            .expect("admit second target bucket");

        let oldest_age = provider
            .pending_spillover_oldest_age()
            .expect("pending batches have an age");
        assert!(oldest_age >= Duration::from_millis(15));
        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 3);

        provider
            .insert_empty_chunk(oldest_key, -64, 319)
            .expect("drain oldest target bucket");
        let next_oldest_age = provider
            .pending_spillover_oldest_age()
            .expect("second target remains pending");
        assert!(next_oldest_age < oldest_age);

        provider
            .insert_empty_chunk(later_key, -64, 319)
            .expect("drain remaining target bucket");
        assert_eq!(provider.pending_spillover_oldest_age(), None);
    }

    #[test]
    fn dirty_high_water_suspends_new_generation_but_not_cached_reads() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        assert_eq!(provider.dirty_bytes(), 0);
        assert!(provider.generation_admission().is_ok());

        // Charge more than the high-water mark, as an accepted writeback batch
        // would.
        provider.note_dirty_bytes(DIRTY_HIGH_WATER_BYTES);
        let refused = provider
            .generation_admission()
            .expect_err("generation must be throttled past the high-water mark");
        assert!(matches!(refused, WorldStorageError::Capacity(_)));
        assert_eq!(provider.generation_throttled(), 1);

        // Confirming (or cancelling) the snapshot releases the budget again.
        provider.release_dirty_bytes(DIRTY_HIGH_WATER_BYTES);
        assert_eq!(provider.dirty_bytes(), 0);
        assert!(provider.generation_admission().is_ok());
    }

    #[test]
    fn critical_dirty_backlog_suspends_generation_outright() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        provider.note_dirty_bytes(DIRTY_CRITICAL_BYTES);
        let refused = provider
            .generation_admission()
            .expect_err("critical backlog must suspend generation");
        assert!(refused.to_string().contains("critical"));
        // Release can never underflow the accounting.
        provider.release_dirty_bytes(DIRTY_CRITICAL_BYTES * 2);
        assert_eq!(provider.dirty_bytes(), 0);
    }

    #[test]
    fn releasing_more_than_charged_never_underflows() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        provider.note_dirty_bytes(64);
        provider.release_dirty_bytes(128);
        assert_eq!(provider.dirty_bytes(), 0);
    }

    #[test]
    fn an_overflow_position_is_requeued_and_applied_when_the_column_rematerializes() {
        let target_key = ChunkKey::new(0, ChunkPosition::new(4, 4));
        let source_key = ChunkKey::new(0, ChunkPosition::new(3, 4));
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));

        // Fill the target's bounded arbitration window with distinct positions.
        let target = provider
            .insert_empty_chunk(target_key, -64, 319)
            .expect("materialize target");
        let filler: Vec<SpilloverJournalEntry> = (0..MAX_TRACKED_SPILLOVER_CELLS)
            .map(|index| {
                let x = (index / 2) as i32;
                let y = -64 + (index % 2) as i32;
                SpilloverJournalEntry {
                    operation_id: SpilloverOperationId::new(
                        ChunkKey::new(0, ChunkPosition::new(index as i32 + 100, 0)),
                        0,
                    ),
                    write: BlockSpilloverWrite {
                        key: target_key,
                        x,
                        y,
                        z: 0,
                        layer: 0,
                        block: BlockRuntimeId(index as u32 + 1),
                    },
                }
            })
            .collect();
        WorldChunkProvider::apply_writes_to_column(&target, &filler);
        assert_eq!(
            target.tracked_spillover_cells(),
            MAX_TRACKED_SPILLOVER_CELLS
        );

        // A routed write for one more position cannot be arbitrated, so it must be
        // re-queued instead of applied out of order.
        let overflow = SpilloverJournalEntry {
            operation_id: SpilloverOperationId::new(source_key, 0),
            write: BlockSpilloverWrite {
                key: target_key,
                x: target_key.position.x * 16 + 5,
                y: 100,
                z: target_key.position.z * 16 + 6,
                layer: 0,
                block: BlockRuntimeId(4242),
            },
        };
        let partition = target.preview_spillover_conflicts(&[overflow]);
        assert_eq!(partition.deferred.len(), 1);
        assert!(
            target
                .read()
                .block_at_layer(0, 5, 100, 6)
                .is_none_or(|block| block != BlockRuntimeId(4242)),
            "an unarbitrated position must not be published out of order"
        );

        // Re-queued writes are applied, in operation-id order, once the owning
        // column absorbs its pending batch.
        WorldChunkProvider::apply_pending_ordered_to_column(
            &target,
            &partition.deferred,
            false,
            false,
            false,
        );
        assert_eq!(
            target.read().block_at_layer(0, 5, 100, 6),
            Some(BlockRuntimeId(4242)),
            "a re-queued generation write is never dropped"
        );
    }

    struct FixedSpilloverGenerator(Vec<BlockSpilloverWrite>);

    impl WorldGenerator for FixedSpilloverGenerator {
        fn generate_chunk(
            &self,
            request: ChunkGenerationRequest,
        ) -> Result<Option<Chunk>, WorldStorageError> {
            Ok(Some(Chunk::empty(
                request.key.position,
                request.key.dimension,
                request.min_y,
                request.max_y,
            )))
        }

        fn generate_chunk_with_spillover(
            &self,
            request: ChunkGenerationRequest,
        ) -> Result<Option<GeneratedChunk>, WorldStorageError> {
            Ok(Some(GeneratedChunk {
                chunk: Chunk::empty(
                    request.key.position,
                    request.key.dimension,
                    request.min_y,
                    request.max_y,
                ),
                spillover: self.0.clone(),
            }))
        }
    }

    fn spillover_write(key: ChunkKey, block: u32) -> BlockSpilloverWrite {
        BlockSpilloverWrite {
            key,
            x: key.position.x * 16 + 1,
            y: -60,
            z: key.position.z * 16 + 2,
            layer: 0,
            block: BlockRuntimeId(block),
        }
    }

    struct ConcurrentSpilloverGenerator {
        barrier: std::sync::Barrier,
        spillover: BlockSpilloverWrite,
    }

    impl WorldGenerator for ConcurrentSpilloverGenerator {
        fn generate_chunk(
            &self,
            request: ChunkGenerationRequest,
        ) -> Result<Option<Chunk>, WorldStorageError> {
            Ok(Some(Chunk::empty(
                request.key.position,
                request.key.dimension,
                request.min_y,
                request.max_y,
            )))
        }

        fn generate_chunk_with_spillover(
            &self,
            request: ChunkGenerationRequest,
        ) -> Result<Option<GeneratedChunk>, WorldStorageError> {
            self.barrier.wait();
            Ok(Some(GeneratedChunk {
                chunk: Chunk::empty(
                    request.key.position,
                    request.key.dimension,
                    request.min_y,
                    request.max_y,
                ),
                spillover: vec![self.spillover],
            }))
        }
    }

    #[test]
    fn concurrent_duplicate_generation_commits_spillover_once() {
        let source_key = ChunkKey::new(0, ChunkPosition::new(15, 15));
        let target_key = ChunkKey::new(0, ChunkPosition::new(16, 15));
        let generator = Arc::new(ConcurrentSpilloverGenerator {
            barrier: std::sync::Barrier::new(2),
            spillover: spillover_write(target_key, 4),
        });
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()))
            .with_generator(generator);
        let first_provider = provider.clone();
        let second_provider = provider.clone();
        let first = std::thread::spawn(move || first_provider.load_chunk(source_key, -64, 319));
        let second = std::thread::spawn(move || second_provider.load_chunk(source_key, -64, 319));

        let first = first
            .join()
            .expect("first loader")
            .expect("first load")
            .expect("first generated source");
        let second = second
            .join()
            .expect("second loader")
            .expect("second load")
            .expect("second generated source");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 1);
        assert_eq!(
            provider.pending_spillover_estimated_bytes(),
            SPILLOVER_ESTIMATED_BYTES_PER_WRITE
        );
        let target = provider
            .insert_empty_chunk(target_key, -64, 319)
            .expect("admit empty chunk");
        assert_eq!(target.generation(), 1);
        assert_eq!(
            target.read().block_at_layer(0, 1, -60, 2),
            Some(BlockRuntimeId(4))
        );
        assert_eq!(provider.pending_spillover_estimated_bytes(), 0);
    }

    /// Two generation sources target the same block position. The published
    /// result must be the higher `SpilloverOperationId` regardless of the order
    /// in which the two routes reach the column (T24).
    #[test]
    fn spillover_conflicts_resolve_by_operation_id_not_arrival_order() {
        let target_key = ChunkKey::new(0, ChunkPosition::new(16, 15));
        let cell = (
            target_key.position.x * 16 + 1,
            -60,
            target_key.position.z * 16 + 2,
        );
        let write = |block| BlockSpilloverWrite {
            key: target_key,
            x: cell.0,
            y: cell.1,
            z: cell.2,
            layer: 0,
            block: BlockRuntimeId(block),
        };
        let entry = |source: ChunkKey, ordinal: u32, block| SpilloverJournalEntry {
            operation_id: SpilloverOperationId::new(source, ordinal),
            write: write(block),
        };
        let low = entry(ChunkKey::new(0, ChunkPosition::new(14, 15)), 0, 4);
        let high = entry(ChunkKey::new(0, ChunkPosition::new(18, 15)), 0, 9);

        // Lower id first, then the higher id: the higher id wins.
        let forward = ChunkColumn::new(Chunk::empty(
            target_key.position,
            target_key.dimension,
            -64,
            319,
        ));
        WorldChunkProvider::apply_writes_to_column(&forward, &[low, high]);
        assert_eq!(
            forward.read().block_at_layer(0, 1, -60, 2),
            Some(BlockRuntimeId(9))
        );

        // Higher id first, then the lower id: the lower id is superseded, so the
        // published block is identical in both arrival orders.
        let reverse = ChunkColumn::new(Chunk::empty(
            target_key.position,
            target_key.dimension,
            -64,
            319,
        ));
        WorldChunkProvider::apply_writes_to_column(&reverse, &[high, low]);
        assert_eq!(
            reverse.read().block_at_layer(0, 1, -60, 2),
            Some(BlockRuntimeId(9))
        );
    }

    #[test]
    fn superseded_spillover_is_counted_and_replaying_it_is_idempotent() {
        let target_key = ChunkKey::new(0, ChunkPosition::new(16, 15));
        let low = SpilloverJournalEntry {
            operation_id: SpilloverOperationId::new(
                ChunkKey::new(0, ChunkPosition::new(14, 15)),
                0,
            ),
            write: BlockSpilloverWrite {
                key: target_key,
                x: target_key.position.x * 16 + 1,
                y: -60,
                z: target_key.position.z * 16 + 2,
                layer: 0,
                block: BlockRuntimeId(4),
            },
        };
        let high = SpilloverJournalEntry {
            operation_id: SpilloverOperationId::new(
                ChunkKey::new(0, ChunkPosition::new(18, 15)),
                0,
            ),
            write: BlockSpilloverWrite {
                block: BlockRuntimeId(9),
                ..low.write
            },
        };
        let column = ChunkColumn::new(Chunk::empty(
            target_key.position,
            target_key.dimension,
            -64,
            319,
        ));
        WorldChunkProvider::apply_writes_to_column(&column, &[high.clone()]);
        let generation_after_high = column.generation();

        // A replay of the winning write is idempotent: no new dirty generation.
        WorldChunkProvider::apply_writes_to_column(&column, &[high]);
        assert_eq!(column.generation(), generation_after_high);

        // The losing write cannot regress the published block.
        WorldChunkProvider::apply_writes_to_column(&column, &[low]);
        assert_eq!(
            column.read().block_at_layer(0, 1, -60, 2),
            Some(BlockRuntimeId(9))
        );
        assert_eq!(column.generation(), generation_after_high);
        assert_eq!(column.superseded_spillover_writes(), 1);
    }

    #[test]
    fn spillover_arbitration_window_is_bounded_and_reported() {
        let target_key = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let column = ChunkColumn::new(Chunk::empty(target_key.position, 0, -64, 319));
        // Distinct, in-bounds target positions: two x values per y band.
        let entries: Vec<SpilloverJournalEntry> = (0..MAX_TRACKED_SPILLOVER_CELLS)
            .map(|index| {
                let x = (index / 2) as i32;
                let y = -64 + (index % 2) as i32;
                SpilloverJournalEntry {
                    operation_id: SpilloverOperationId::new(
                        ChunkKey::new(0, ChunkPosition::new(index as i32, 0)),
                        0,
                    ),
                    write: BlockSpilloverWrite {
                        key: target_key,
                        x,
                        y,
                        z: 0,
                        layer: 0,
                        block: BlockRuntimeId(index as u32 + 1),
                    },
                }
            })
            .collect();
        WorldChunkProvider::apply_writes_to_column(&column, &entries);
        assert_eq!(
            column.tracked_spillover_cells(),
            MAX_TRACKED_SPILLOVER_CELLS
        );
        assert_eq!(column.untracked_spillover_writes(), 0);

        // One more distinct position is applied but no longer arbitrated.
        let overflow = SpilloverJournalEntry {
            operation_id: SpilloverOperationId::new(
                ChunkKey::new(0, ChunkPosition::new(9999, 0)),
                0,
            ),
            write: BlockSpilloverWrite {
                key: target_key,
                x: 255,
                y: 64,
                z: 0,
                layer: 0,
                block: BlockRuntimeId(7),
            },
        };
        // The extra position is *not* resolved by arrival order: it is handed back
        // so the owning column can apply it in operation-id order later.
        let partition = column.preview_spillover_conflicts(&[overflow]);
        assert_eq!(partition.applicable.len(), 0);
        assert_eq!(partition.deferred.len(), 1);
        assert_eq!(partition.untracked, 1);
        assert_eq!(partition.deferred[0].write.block, BlockRuntimeId(7));
        assert_eq!(
            column.tracked_spillover_cells(),
            MAX_TRACKED_SPILLOVER_CELLS
        );

        // The forced owner-absorption path applies it in operation-id order.
        WorldChunkProvider::apply_pending_ordered_to_column(
            &column,
            &partition.deferred,
            false,
            false,
            false,
        );
        assert_eq!(
            column.read().block_at_layer(0, 15, 64, 0),
            Some(BlockRuntimeId(7)),
            "a re-queued write is never dropped"
        );
        assert_eq!(column.untracked_spillover_writes(), 0);
    }

    #[test]
    fn spillover_generation_permits_bound_and_release_result_working_bytes() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let first = provider
            .reserve_spillover_generation()
            .expect("reserve first result buffer");
        let second = provider
            .reserve_spillover_generation()
            .expect("reserve second result buffer");
        assert_eq!(
            provider.spillover_generation_reserved_bytes(),
            MAX_IN_FLIGHT_SPILLOVER_BYTES
        );
        assert!(matches!(
            provider.reserve_spillover_generation(),
            Err(WorldStorageError::Capacity(_))
        ));

        drop(first);
        assert_eq!(
            provider.spillover_generation_reserved_bytes(),
            MAX_PENDING_SPILLOVER_BYTES
        );
        drop(second);
        assert_eq!(provider.spillover_generation_reserved_bytes(), 0);
    }

    #[test]
    fn spillover_byte_budget_rejects_the_whole_batch_without_partial_mutation() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let queued_key = ChunkKey::new(0, ChunkPosition::new(10, 10));
        let pending_at_byte_limit =
            MAX_PENDING_SPILLOVER_BYTES / SPILLOVER_ESTIMATED_BYTES_PER_WRITE;
        provider
            .route_spillover(
                (0..pending_at_byte_limit)
                    .map(|_| spillover_write(queued_key, 1))
                    .collect(),
            )
            .expect("fill pending spillover byte budget");
        assert_eq!(
            provider.pending_spillover_count.load(Ordering::Relaxed),
            pending_at_byte_limit
        );
        assert_eq!(
            provider.pending_spillover_estimated_bytes(),
            pending_at_byte_limit * SPILLOVER_ESTIMATED_BYTES_PER_WRITE
        );

        let cached_key = ChunkKey::new(0, ChunkPosition::new(11, 10));
        let missing_key = ChunkKey::new(0, ChunkPosition::new(12, 10));
        let cached = provider
            .insert_empty_chunk(cached_key, -64, 319)
            .expect("admit empty chunk");
        let before = cached.read().block_at_layer(0, 1, -60, 2);
        let result = provider.route_spillover(vec![
            spillover_write(cached_key, 7),
            spillover_write(missing_key, 8),
        ]);

        assert!(matches!(result, Err(WorldStorageError::Backend(_))));
        assert_eq!(cached.read().block_at_layer(0, 1, -60, 2), before);
        assert_eq!(
            provider.pending_spillover_count.load(Ordering::Relaxed),
            pending_at_byte_limit
        );
        assert_eq!(
            provider.pending_spillover_estimated_bytes(),
            pending_at_byte_limit * SPILLOVER_ESTIMATED_BYTES_PER_WRITE
        );
    }

    #[test]
    fn spillover_bytes_reject_before_count_limit_and_without_queue_mutation() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let key = ChunkKey::new(0, ChunkPosition::new(13, 10));
        let count = MAX_PENDING_SPILLOVER_BYTES / SPILLOVER_ESTIMATED_BYTES_PER_WRITE + 1;
        assert!(count < MAX_PENDING_SPILLOVER_WRITES);

        let result =
            provider.route_spillover((0..count).map(|_| spillover_write(key, 3)).collect());
        assert!(matches!(result, Err(WorldStorageError::Backend(_))));
        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 0);
        assert_eq!(provider.pending_spillover_estimated_bytes(), 0);
        assert!(provider.cached_chunk(key).is_none());
    }

    #[test]
    fn invalid_spillover_rejects_the_whole_generation_result() {
        let source_key = ChunkKey::new(0, ChunkPosition::new(20, 20));
        let mut invalid = spillover_write(ChunkKey::new(0, ChunkPosition::new(21, 20)), 9);
        invalid.x += 16; // World coordinates no longer belong to the declared target key.
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()))
            .with_generator(Arc::new(FixedSpilloverGenerator(vec![invalid])));

        let result = provider.load_chunk(source_key, -64, 319);
        assert!(matches!(result, Err(WorldStorageError::Backend(_))));
        assert!(provider.cached_chunk(source_key).is_none());
        assert!(provider.cached_chunk(invalid.key).is_none());
        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn over_budget_generation_does_not_publish_source_or_part_of_its_spillover() {
        let source_key = ChunkKey::new(0, ChunkPosition::new(20, 20));
        let target_key = ChunkKey::new(0, ChunkPosition::new(21, 20));
        let generator = Arc::new(FixedSpilloverGenerator(
            (0..=MAX_GENERATED_SPILLOVER_WRITES)
                .map(|_| spillover_write(target_key, 9))
                .collect(),
        ));
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()))
            .with_generator(generator);

        let result = provider.load_chunk(source_key, -64, 319);
        assert!(matches!(result, Err(WorldStorageError::Capacity(_))));
        assert!(provider.cached_chunk(source_key).is_none());
        assert!(provider.cached_chunk(target_key).is_none());
        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cache_admission_rejects_generated_source_before_spillover_side_effects() {
        let existing_key = ChunkKey::new(0, ChunkPosition::new(24, 16));
        let source_key = ChunkKey::new(0, ChunkPosition::new(25, 16));
        let target_key = ChunkKey::new(0, ChunkPosition::new(26, 16));
        let provider =
            WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default())).with_cache_limit(1);
        let existing = provider
            .ensure_chunk(existing_key, -64, 319)
            .expect("admit existing column");
        existing.mark_dirty(-4);
        let provider =
            provider.with_generator(Arc::new(FixedSpilloverGenerator(vec![spillover_write(
                target_key, 91,
            )])));

        assert!(matches!(
            provider.load_chunk(source_key, -64, 319),
            Err(WorldStorageError::Capacity(_))
        ));
        assert!(existing.is_dirty());
        assert!(provider.cached_chunk(source_key).is_none());
        assert!(provider.cached_chunk(target_key).is_none());
        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn generated_source_stays_hidden_until_cached_spillover_target_is_updated() {
        let source_key = ChunkKey::new(0, ChunkPosition::new(40, 40));
        let target_key = ChunkKey::new(0, ChunkPosition::new(41, 40));
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let target = provider
            .insert_empty_chunk(target_key, -64, 319)
            .expect("admit empty chunk");
        let target_guard = target.write();
        let provider =
            provider.with_generator(Arc::new(FixedSpilloverGenerator(vec![spillover_write(
                target_key, 9,
            )])));
        let worker_provider = provider.clone();
        let worker = std::thread::spawn(move || worker_provider.load_chunk(source_key, -64, 319));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while target.spillover_pins.load(Ordering::Acquire) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(
            target.spillover_pins.load(Ordering::Acquire),
            1,
            "loader should pin the cached target before applying the spillover"
        );
        assert!(
            provider.cached_chunk(source_key).is_none(),
            "source column must not be visible while its outgoing spillover is unapplied"
        );

        drop(target_guard);
        let source = worker
            .join()
            .expect("loader thread")
            .expect("load source")
            .expect("generated source");
        assert_eq!(
            target.read().block_at_layer(0, 1, -60, 2),
            Some(BlockRuntimeId(9))
        );
        assert!(provider.cached_chunk(source_key).is_some());
        assert_eq!(source.generation(), 0);
    }

    #[test]
    fn generated_source_applies_incoming_spillover_before_cache_publication() {
        let source_key = ChunkKey::new(0, ChunkPosition::new(30, 30));
        let neighbor_key = ChunkKey::new(0, ChunkPosition::new(31, 30));
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        provider
            .route_spillover(vec![spillover_write(source_key, 1)])
            .expect("queue incoming write");
        let provider =
            provider.with_generator(Arc::new(FixedSpilloverGenerator(vec![spillover_write(
                neighbor_key,
                2,
            )])));

        let source = provider
            .load_chunk(source_key, -64, 319)
            .expect("load source")
            .expect("generated source");

        assert_eq!(
            source.read().block_at_layer(0, 1, -60, 2),
            Some(BlockRuntimeId(1))
        );
        assert_eq!(source.generation(), 1);
        assert_eq!(provider.pending_spillover_count.load(Ordering::Relaxed), 1);
        assert!(provider.cached_chunk(neighbor_key).is_none());
    }

    #[test]
    fn cache_capacity_rejection_preserves_dirty_column_and_can_retry_after_save() {
        let storage = Arc::new(InMemoryWorldStorage::default());
        let provider = WorldChunkProvider::new(storage.clone()).with_cache_limit(1);
        let dirty_key = ChunkKey::new(0, ChunkPosition::new(50, 3));
        let requested_key = ChunkKey::new(0, ChunkPosition::new(51, 3));
        let dirty = provider
            .ensure_chunk(dirty_key, -64, 319)
            .expect("admit first column");
        dirty.mark_dirty(-4);

        assert!(matches!(
            provider.ensure_chunk(requested_key, -64, 319),
            Err(WorldStorageError::Capacity(_))
        ));
        assert_eq!(provider.cache_len(), 1);
        assert!(dirty.is_dirty(), "capacity refusal cannot evict dirty data");
        assert!(provider.cached_chunk(requested_key).is_none());

        assert_eq!(provider.flush_dirty(), 1);
        provider
            .ensure_chunk(requested_key, -64, 319)
            .expect("retry after the dirty resident is saved");
        assert_eq!(provider.cache_len(), 1);
        assert!(storage
            .load_chunk(dirty_key)
            .expect("load saved dirty column")
            .is_some());
    }

    #[test]
    fn cache_byte_capacity_rejects_an_oversize_column_without_accounting_leak() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()))
            .with_cache_memory_limit(1);
        let key = ChunkKey::new(0, ChunkPosition::new(52, 3));
        assert!(matches!(
            provider.ensure_chunk(key, -64, 319),
            Err(WorldStorageError::Capacity(_))
        ));
        assert_eq!(provider.cache_len(), 0);
        assert_eq!(provider.cache_memory_bytes(), 0);
        assert!(provider.cached_chunk(key).is_none());
    }

    #[test]
    fn cache_byte_admission_counts_a_candidate_once_and_keeps_both_that_fit() {
        let sample = ChunkColumn::new(Chunk::empty(ChunkPosition::new(0, 0), 0, -64, 319));
        let one_column_bytes = sample.estimated_memory_bytes();
        drop(sample);
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()))
            .with_cache_memory_limit(one_column_bytes.saturating_mul(2));

        let first = provider
            .ensure_chunk(ChunkKey::new(0, ChunkPosition::new(53, 3)), -64, 319)
            .expect("admit first column");
        drop(first);
        provider
            .ensure_chunk(ChunkKey::new(0, ChunkPosition::new(54, 3)), -64, 319)
            .expect("two estimated columns fit the configured budget");

        assert_eq!(provider.cache_len(), 2);
        assert!(provider.cache_memory_bytes() <= one_column_bytes * 2);
    }

    #[test]
    fn pending_cache_slot_reservations_prevent_overbooking() {
        let provider =
            WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default())).with_cache_limit(1);
        let make_candidate = |x| {
            Arc::new(ChunkColumn::new_with_counter(
                Chunk::empty(ChunkPosition::new(x, 0), 0, -64, 319),
                Some(Arc::clone(&provider.cache_memory_estimate)),
            ))
        };
        let first_key = ChunkKey::new(0, ChunkPosition::new(55, 0));
        let second_key = ChunkKey::new(0, ChunkPosition::new(56, 0));
        let first = make_candidate(first_key.position.x);
        let second = make_candidate(second_key.position.x);
        let first_reservation = provider
            .reserve_cache_slot(first_key, &first)
            .expect("reserve the only resident slot");
        assert_eq!(provider.pending_cache_slots.load(Ordering::Acquire), 1);
        assert!(matches!(
            provider.reserve_cache_slot(second_key, &second),
            Err(WorldStorageError::Capacity(_))
        ));

        drop(first_reservation);
        assert_eq!(provider.pending_cache_slots.load(Ordering::Acquire), 0);
        drop(first);
        drop(second);
        assert_eq!(provider.cache_memory_bytes(), 0);
    }

    #[test]
    fn dirty_retired_handles_remain_canonical_outside_the_resident_limit() {
        let storage = Arc::new(InMemoryWorldStorage::default());
        let provider = WorldChunkProvider::new(storage).with_cache_limit(1);
        let first = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let second = ChunkKey::new(0, ChunkPosition::new(1, 0));
        let first_column = provider.ensure_chunk(first, -64, 319).expect("first");
        provider.ensure_chunk(second, -64, 319).expect("second");
        assert_eq!(
            provider.cache_len(),
            1,
            "first column left the resident cache"
        );
        assert!(
            provider.cached_chunk(first).is_some(),
            "retired handle stays canonical"
        );
        first_column.mark_dirty(0);
        let third = ChunkKey::new(0, ChunkPosition::new(2, 0));
        provider.ensure_chunk(third, -64, 319).expect("third");
        assert!(provider.cached_chunk(first).is_some());
        assert_eq!(
            provider.cache_len(),
            1,
            "dirty retired handles use no resident slot"
        );
        assert_eq!(
            provider.retired.read().expect("retired lock").len(),
            1,
            "dirty data remains provider-owned and recoverable"
        );
    }
}
