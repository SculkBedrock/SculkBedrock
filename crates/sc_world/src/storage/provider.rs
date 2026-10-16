use super::{
    CacheSlotReservation, ChunkColumn, ChunkGenerationRequest, ChunkKey,
    DirtyColumnIndex, DirtyFlushReport, GeneratedChunk, GeneratedChunkCommit,
    PendingSpilloverBatch, SpilloverGenerationPermit, SpilloverJournalEntry, SpilloverOperationId,
    SpilloverPartition, ChunkColumnPin, merge_spillover_entries, report_spillover_arbitration,
    WorldStorageError, DIRTY_CRITICAL_BYTES, DIRTY_HIGH_WATER_BYTES,
    MAX_GENERATED_SPILLOVER_WRITES, MAX_IN_FLIGHT_SPILLOVER_BYTES, MAX_PENDING_SPILLOVER_BYTES,
    MAX_PENDING_SPILLOVER_WRITES, MAX_RETIRED_COLUMN_HANDLES, MAX_STORED_SPILLOVER_WRITES,
    MAX_SYNC_FLUSH_BATCH_COLUMNS, MAX_SYNC_FLUSH_SNAPSHOT_BYTES,
    SPILLOVER_ESTIMATED_BYTES_PER_WRITE, SPILLOVER_LOCK_STRIPES,
};
use crate::chunk::{Chunk, ChunkPosition, SUBCHUNK_SIZE};
use sc_log::t_log;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, RwLock,
};
use std::time::{Duration, Instant};
#[cfg(test)]
use super::BlockSpilloverWrite;

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
            return Err(WorldStorageError::unsupported(
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
        Err(WorldStorageError::unsupported(
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
            Err(WorldStorageError::unsupported(
                "spillover journal not supported by this backend".to_string(),
            ))
        }
    }

    /// Writes a chunk (blocks/block entities) back to the archive.
    /// The default impl does not support it (memory/generator-only backends may leave it empty).
    fn save_chunk(&self, _key: ChunkKey, _chunk: &Chunk) -> Result<(), WorldStorageError> {
        Err(WorldStorageError::unsupported(
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
            return Err(WorldStorageError::capacity(format!(
                "generator spillover result exceeds the {max_spillover_writes}-write bound"
            )));
        }
        Ok(generated)
    }
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
    pub(crate) retired: Arc<RwLock<HashMap<ChunkKey, Arc<ChunkColumn>>>>,
    /// Ordered index of dirty provider-owned columns; avoids scanning every
    /// resident/retired entry on each writeback scheduling tick.
    dirty_index: Arc<DirtyColumnIndex>,
    /// Pending queue for cross-chunk spillover writes: staged while the target
    /// chunk is not cached yet, applied when that chunk first enters the cache
    /// (via generate/load/ensure).
    pending_spillover: Arc<Mutex<HashMap<ChunkKey, PendingSpilloverBatch>>>,
    pub(crate) pending_spillover_count: Arc<AtomicUsize>,
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
    pub(crate) cache_memory_estimate: Arc<AtomicUsize>,
    /// Resident slots reserved by columns being prepared for publication.
    pub(crate) pending_cache_slots: Arc<AtomicUsize>,
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

    pub(crate) fn reserve_spillover_generation(&self) -> Result<SpilloverGenerationPermit, WorldStorageError> {
        let bytes = MAX_PENDING_SPILLOVER_BYTES;
        let mut current = self
            .spillover_generation_reserved_bytes
            .load(Ordering::Acquire);
        loop {
            let next = current.saturating_add(bytes);
            if next > MAX_IN_FLIGHT_SPILLOVER_BYTES {
                return Err(WorldStorageError::capacity(format!(
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
            return Err(WorldStorageError::corrupt(format!(
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
            return Err(WorldStorageError::corrupt(
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
                Err(WorldStorageError::backend(format!(
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
                Err(WorldStorageError::backend(format!(
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
            return Err(WorldStorageError::capacity(format!(
                "dirty backlog {dirty} bytes reached the critical mark {DIRTY_CRITICAL_BYTES}; \
                 generation is suspended until writeback confirms"
            )));
        }
        if dirty >= DIRTY_HIGH_WATER_BYTES {
            self.generation_throttled.fetch_add(1, Ordering::Relaxed);
            return Err(WorldStorageError::capacity(format!(
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
    pub(crate) fn reserve_cache_slot(
        &self,
        key: ChunkKey,
        candidate: &ChunkColumn,
    ) -> Result<CacheSlotReservation, WorldStorageError> {
        let candidate_bytes = candidate.estimated_memory_bytes();
        if candidate_bytes > self.max_cache_bytes {
            return Err(WorldStorageError::capacity(format!(
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
                return Err(WorldStorageError::capacity(format!(
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

            return Err(WorldStorageError::capacity(format!(
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
    pub(crate) fn route_spillover(&self, writes: Vec<BlockSpilloverWrite>) -> Result<(), WorldStorageError> {
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
            return Err(WorldStorageError::backend(format!(
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
                    return Err(WorldStorageError::corrupt(format!(
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
            return Err(WorldStorageError::backend(format!(
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
                        Err(WorldStorageError::capacity(
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
        Err(WorldStorageError::backend(format!(
            "spillover target {key:?} was replaced after routing; refusing to write a stale column"
        )))
    }

    /// Apply a spillover batch with arbitration (used by routing and tests).
    ///
    /// Writes whose position is outside the column's bounded arbitration window
    /// are **not** resolved here; the routing path re-queues them so the owning
    /// column applies them in operation-id order when it is next materialized.
    // Spillover/journal reserved capability (see unified architecture doc §9).
    #[allow(dead_code)]
    pub(crate) fn apply_writes_to_column(column: &ChunkColumn, writes: &[SpilloverJournalEntry]) {
        Self::apply_writes_to_column_with_metadata(column, writes, false, false, false);
    }

    // Spillover/journal reserved capability (see unified architecture doc §9).
    #[allow(dead_code)]
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
    pub(crate) fn apply_pending_ordered_to_column(
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

    pub(crate) fn flush_dirty_report_with_snapshot_limit(
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
            .map_err(|_| WorldStorageError::backend("in-memory chunks poisoned".into()))?;
        Ok(chunks.get(&key).cloned())
    }

    fn load_chunk_with_spillover(
        &self,
        key: ChunkKey,
    ) -> Result<(Option<Chunk>, Vec<SpilloverJournalEntry>), WorldStorageError> {
        let chunks = self
            .chunks
            .read()
            .map_err(|_| WorldStorageError::backend("in-memory chunks poisoned".into()))?;
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
            .map_err(|_| WorldStorageError::backend("in-memory chunks poisoned".into()))?;
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
            return Err(WorldStorageError::corrupt(
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
            .map_err(|_| WorldStorageError::backend("in-memory chunks poisoned".into()))?
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
            .map_err(|_| WorldStorageError::backend("in-memory journal poisoned".into()))?;
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
            return Err(WorldStorageError::capacity(
                "spillover journal batch limit exceeded".into(),
            ));
        }
        let entries = merge_spillover_entries(entries.iter().copied())?;
        let mut journal = self
            .spillover_journal
            .write()
            .map_err(|_| WorldStorageError::backend("in-memory journal poisoned".to_string()))?;
        let mut additions: HashMap<ChunkKey, usize> = HashMap::new();
        for entry in &entries {
            if let Some(existing) = journal
                .targets
                .get(&entry.write.key)
                .and_then(|target| target.get(&entry.operation_id))
            {
                if existing != entry {
                    return Err(WorldStorageError::corrupt(
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
            return Err(WorldStorageError::capacity(
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
            .map_err(|_| WorldStorageError::backend("in-memory chunks poisoned".into()))?;
        let mut journal = self
            .spillover_journal
            .write()
            .map_err(|_| WorldStorageError::backend("in-memory journal poisoned".to_string()))?;
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
