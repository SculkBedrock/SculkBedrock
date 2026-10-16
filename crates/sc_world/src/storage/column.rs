use super::{BlockSpilloverWrite, ChunkKey, SpilloverJournalEntry, SpilloverOperationId};
use crate::chunk::Chunk;
use std::collections::{BTreeSet, HashMap};
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak,
};
use std::time::Instant;

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
    pub(crate) estimated_bytes: AtomicUsize,
    memory_counter: Option<Arc<AtomicUsize>>,
    memory_accounted: AtomicBool,
    pub(crate) spillover_pins: AtomicUsize,
}

/// Per-block arbitration state owned by the authoritative column.
///
/// The tracked window is bounded: a column that receives more distinct
/// spillover targets than [`MAX_TRACKED_SPILLOVER_CELLS`] keeps applying those
/// writes but stops recording them, so the deterministic guarantee degrades to
/// arrival order for the untracked positions. That degradation is counted and
/// reported instead of being silently assumed.
pub(crate) const MAX_TRACKED_SPILLOVER_CELLS: usize = 512;

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
pub(crate) struct SpilloverPartition {
    /// Writes this column applies now.
    pub(crate) applicable: Vec<SpilloverJournalEntry>,
    /// Writes the caller must re-queue for a later, ordered application.
    pub(crate) deferred: Vec<SpilloverJournalEntry>,
    /// Writes a higher-priority generation already superseded.
    pub(crate) superseded: u64,
    /// Deferred writes caused by a full arbitration window.
    pub(crate) untracked: u64,
}

#[derive(Default)]
struct SpilloverConflictState {
    /// block position → largest `SpilloverOperationId` already applied there.
    winners: HashMap<SpilloverCell, SpilloverOperationId>,
    /// Writes skipped because a higher-priority generation already won.
    // Spillover/journal reserved capability (see unified architecture doc §9).
    #[allow(dead_code)]
    pub(crate) superseded: u64,
    /// Positions that were re-queued because the tracked window was full.
    pub(crate) untracked: u64,
}

#[derive(Default)]
pub(crate) struct DirtyColumnIndex {
    keys: RwLock<BTreeSet<ChunkKey>>,
}

impl DirtyColumnIndex {
    pub(crate) fn insert(&self, key: ChunkKey) {
        self.keys
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key);
    }

    pub(crate) fn remove(&self, key: &ChunkKey) {
        self.keys
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key);
    }

    pub(crate) fn window(&self, after: Option<ChunkKey>, limit: usize) -> Vec<ChunkKey> {
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

    pub(crate) fn keys_after(
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

    pub(crate) fn last_key(&self) -> Option<ChunkKey> {
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
pub(crate) struct CacheSlotReservation {
    pub(crate) pending: Arc<AtomicUsize>,
    pub(crate) active: bool,
}

impl CacheSlotReservation {
    pub(crate) fn commit(&mut self) {
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

    pub(crate) fn new_with_counter(chunk: Chunk, memory_counter: Option<Arc<AtomicUsize>>) -> Self {
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

    pub(crate) fn refresh_memory_estimate(&self) -> usize {
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

    pub(crate) fn unaccount_memory(&self) {
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

    pub(crate) fn pin_for_spillover(column: &Arc<Self>) -> ChunkColumnPin {
        column.spillover_pins.fetch_add(1, Ordering::AcqRel);
        ChunkColumnPin(Arc::clone(column))
    }

    pub(crate) fn is_pinned(&self) -> bool {
        self.spillover_pins.load(Ordering::Acquire) != 0
    }

    /// Number of distinct block positions currently arbitrated by this column.
    // Spillover/journal reserved capability (see unified architecture doc §9).
    #[allow(dead_code)]
    pub(crate) fn tracked_spillover_cells(&self) -> usize {
        self.spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .winners
            .len()
    }

    /// Generation writes skipped because a higher-priority source won.
    // Spillover/journal reserved capability (see unified architecture doc §9).
    #[allow(dead_code)]
    pub(crate) fn superseded_spillover_writes(&self) -> u64 {
        self.spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .superseded
    }

    /// Positions applied without arbitration because the tracked window was full.
    // Spillover/journal reserved capability (see unified architecture doc §9).
    #[allow(dead_code)]
    pub(crate) fn untracked_spillover_writes(&self) -> u64 {
        self.spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .untracked
    }

    pub(crate) fn unapplied_spillover_entries(
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
    pub(crate) fn preview_spillover_conflicts(&self, entries: &[SpilloverJournalEntry]) -> SpilloverPartition {
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
    pub(crate) fn commit_spillover_conflicts(&self, applicable: &[SpilloverJournalEntry], untracked: u64) {
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
    // Spillover/journal reserved capability (see unified architecture doc §9).
    #[allow(dead_code)]
    pub(crate) fn note_superseded(&self, superseded: u64) {
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
    pub(crate) fn attach_dirty_index(&self, key: ChunkKey, index: &Arc<DirtyColumnIndex>) {
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

    pub(crate) fn remove_stale_dirty_index_if_clean(&self) {
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
pub(crate) struct ChunkColumnPin(pub(crate) Arc<ChunkColumn>);

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
pub(crate) struct PendingSpilloverBatch {
    pub(crate) writes: Vec<SpilloverJournalEntry>,
    pub(crate) first_enqueued_at: Instant,
}
