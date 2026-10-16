pub mod column;
pub mod error;
pub mod key;
pub mod provider;
pub mod spillover;

pub use column::ChunkColumn;
pub use error::{DirtyFlushReport, WorldStorageError};
pub use key::{ChunkKey, SpilloverOperationId};
pub use provider::{
    EmptyWorldStorage, InMemoryWorldStorage, WorldChunkProvider, WorldGenerator, WorldStorage,
};
pub use spillover::{
    BlockSpilloverWrite, ChunkGenerationRequest, GeneratedChunk, GeneratedChunkCommit,
    SpilloverJournalEntry,
};
pub(crate) use column::{
    CacheSlotReservation, ChunkColumnPin, DirtyColumnIndex, PendingSpilloverBatch,
    SpilloverPartition, MAX_TRACKED_SPILLOVER_CELLS,
};
pub(crate) use spillover::{
    SpilloverGenerationPermit, merge_spillover_entries, report_spillover_arbitration,
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPosition};
    use sc_nbt::compound::CompoundNbt;
    use sc_nbt::NbtValue;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use std::time::Duration;
    use crate::chunk::BlockRuntimeId;
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
            provider
                .retired
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            MAX_RETIRED_COLUMN_HANDLES
        );
        assert_eq!(provider.cache_len(), 1, "resident slots stay hard-bounded");

        let rejected_key = ChunkKey::new(
            0,
            ChunkPosition::new(MAX_RETIRED_COLUMN_HANDLES as i32 + 1, 0),
        );
        assert!(matches!(
            provider.ensure_chunk(rejected_key, -64, 319),
            Err(WorldStorageError::Capacity { .. })
        ));
        assert_eq!(provider.cache_len(), 1);

        let released_key = ChunkKey::new(0, ChunkPosition::new(0, 0));
        drop(held.remove(0));
        assert!(provider.cached_chunk(released_key).is_none());
        assert_eq!(
            provider
                .retired
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
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
                        .map_err(|error| WorldStorageError::backend(error.to_string()))?
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
        *storage
            .provider
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Arc::downgrade(&provider));
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
                .ok_or_else(|| WorldStorageError::backend("test snapshot missing block".into()))?;
            self.save_started
                .send(block)
                .map_err(|error| WorldStorageError::backend(error.to_string()))?;
            self.release_save
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .recv()
                .map_err(|error| WorldStorageError::backend(error.to_string()))?;
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
            *storage
                .saved
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
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
        assert!(matches!(refused, WorldStorageError::Capacity { .. }));
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
            Err(WorldStorageError::Capacity { .. })
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

        assert!(matches!(result, Err(WorldStorageError::Backend { .. })));
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
        assert!(matches!(result, Err(WorldStorageError::Backend { .. })));
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
        assert!(matches!(result, Err(WorldStorageError::Backend { .. })));
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
        assert!(matches!(result, Err(WorldStorageError::Capacity { .. })));
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
            Err(WorldStorageError::Capacity { .. })
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
            Err(WorldStorageError::Capacity { .. })
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
            Err(WorldStorageError::Capacity { .. })
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
            Err(WorldStorageError::Capacity { .. })
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
            provider
                .retired
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1,
            "dirty data remains provider-owned and recoverable"
        );
    }
}
