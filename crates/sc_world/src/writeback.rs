//! Bounded asynchronous dirty-column writeback.
//!
//! The root-world owner admits dirty snapshots and polls terminal receipts.
//! Worker threads only call the existing storage interface; they never clear
//! dirty state or mutate a `ChunkColumn`.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use sc_ecs::resource::Resource;

use crate::chunk::Chunk;
use crate::manager::MinecraftWorldId;
use crate::storage::{
    ChunkColumn, ChunkKey, SpilloverOperationId, WorldChunkProvider, WorldStorage,
};
use sc_log::t_log;

const DEFAULT_MAX_JOBS: usize = 128;
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_DIRTY_CANDIDATES_PER_WORLD_TICK: usize = 64;
const MAX_FAILURE_KEYS: usize = 4096;
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct WritebackKey {
    world_id: MinecraftWorldId,
    chunk: ChunkKey,
}

struct WritebackJob {
    key: WritebackKey,
    attempt_id: u64,
    column: Arc<ChunkColumn>,
    generation: u64,
    block_entities_dirty: bool,
    biomes_dirty: bool,
    heightmap_dirty: bool,
    spillover_acks: Vec<SpilloverOperationId>,
    storage: Arc<dyn WorldStorage>,
    snapshot: Chunk,
}

struct WritebackAck {
    key: WritebackKey,
    attempt_id: u64,
    column: Arc<ChunkColumn>,
    generation: u64,
    spillover_acks: Vec<SpilloverOperationId>,
    result: Result<(), String>,
}

struct InFlightWriteback {
    attempt_id: u64,
    reserved_bytes: usize,
    accepted_at: Instant,
    age_warning_emitted: bool,
    /// Owning provider, so a confirmed write can release its dirty budget.
    provider: WorldChunkProvider,
    dirty_bytes_charged: usize,
}

#[derive(Default)]
pub(crate) struct WritebackAgeReport {
    pub newly_overdue: usize,
    pub overdue: usize,
    pub oldest_age: Option<Duration>,
    pub key_samples: Vec<(MinecraftWorldId, ChunkKey)>,
}

#[derive(Clone, Copy)]
struct FailureRecord {
    retry_at: Instant,
    attempts: u8,
}

#[derive(Default)]
struct WritebackState {
    in_flight: HashMap<WritebackKey, InFlightWriteback>,
    in_flight_bytes: usize,
    next_attempt_id: u64,
    retry: HashMap<WritebackKey, FailureRecord>,
    retry_order: VecDeque<WritebackKey>,
    dirty_cursor: HashMap<MinecraftWorldId, ChunkKey>,
    world_cursor: Option<MinecraftWorldId>,
}

impl WritebackState {
    fn overdue_report(
        &mut self,
        now: Instant,
        threshold: Duration,
        sample_limit: usize,
    ) -> WritebackAgeReport {
        let mut report = WritebackAgeReport::default();
        for (key, record) in &mut self.in_flight {
            let age = now.saturating_duration_since(record.accepted_at);
            if age < threshold {
                continue;
            }
            report.overdue += 1;
            report.oldest_age = Some(report.oldest_age.map_or(age, |oldest| oldest.max(age)));
            if !record.age_warning_emitted {
                record.age_warning_emitted = true;
                report.newly_overdue += 1;
                if report.key_samples.len() < sample_limit {
                    report.key_samples.push((key.world_id.clone(), key.chunk));
                }
            }
        }
        report
    }

    fn retry_blocked(&self, key: &WritebackKey, now: Instant) -> bool {
        self.retry
            .get(key)
            .is_some_and(|record| record.retry_at > now)
    }

    fn record_failure(&mut self, key: WritebackKey, now: Instant) {
        let attempts = if let Some(record) = self.retry.get_mut(&key) {
            record.attempts = record.attempts.saturating_add(1);
            record.attempts
        } else {
            while self.retry.len() >= MAX_FAILURE_KEYS {
                let Some(oldest) = self.retry_order.pop_front() else {
                    break;
                };
                self.retry.remove(&oldest);
            }
            self.retry_order.push_back(key.clone());
            self.retry.insert(
                key.clone(),
                FailureRecord {
                    retry_at: now,
                    attempts: 0,
                },
            );
            1
        };
        let shift = u32::from(attempts.saturating_sub(1).min(5));
        let delay = INITIAL_RETRY_DELAY
            .checked_mul(1u32 << shift)
            .unwrap_or(MAX_RETRY_DELAY)
            .min(MAX_RETRY_DELAY);
        if let Some(record) = self.retry.get_mut(&key) {
            record.retry_at = now + delay;
            record.attempts = attempts;
        }
    }

    fn clear_failure(&mut self, key: &WritebackKey) {
        if self.retry.remove(key).is_some() {
            self.retry_order.retain(|entry| entry != key);
        }
    }
}

struct WritebackInner {
    jobs: Mutex<Option<mpsc::SyncSender<WritebackJob>>>,
    acknowledgements: Mutex<mpsc::Receiver<WritebackAck>>,
    state: Mutex<WritebackState>,
    workers: Mutex<Option<Vec<thread::JoinHandle<()>>>>,
    max_jobs: usize,
    max_bytes: usize,
}

/// Non-blocking admission outcomes for dirty snapshot scheduling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WritebackSubmit {
    Queued,
    AlreadyInFlight,
    Clean,
    Busy,
    Oversize,
    Backoff,
    Closed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WritebackPollReport {
    pub completed: usize,
    pub saved: usize,
    pub failed: usize,
    pub superseded: usize,
}

/// Bounded shared worker pool for storage writeback. Accepted count/byte
/// permits remain reserved until the owning tick consumes each completion.
#[derive(Resource, Clone)]
pub struct ChunkWritebackExecutor {
    inner: Arc<WritebackInner>,
}

impl ChunkWritebackExecutor {
    pub fn default_worker_count() -> usize {
        let parallelism = thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(2);
        (parallelism / 4).clamp(1, 2)
    }

    pub fn new(worker_count: usize) -> io::Result<Self> {
        Self::with_limits(worker_count, DEFAULT_MAX_JOBS, DEFAULT_MAX_BYTES)
    }

    fn with_limits(worker_count: usize, max_jobs: usize, max_bytes: usize) -> io::Result<Self> {
        let max_jobs = max_jobs.max(1);
        let max_bytes = max_bytes.max(1);
        let worker_count = worker_count.clamp(1, 4);
        let (job_tx, job_rx) = mpsc::sync_channel::<WritebackJob>(max_jobs);
        let job_rx = Arc::new(Mutex::new(job_rx));
        // Completion-slot reservation invariant (§13.2): the acknowledgement
        // channel holds exactly as many slots as the executor can accept
        // outstanding jobs, and a job's byte permit is only released when the
        // owning tick polls its completion. A worker therefore never blocks on
        // `ack_tx.send`, so "drain the accepted jobs, then join" is a valid
        // shutdown order. Changing either capacity invalidates that order.
        // Completion-slot reservation invariant (§13.2): the acknowledgement
        // channel holds exactly as many slots as the executor can accept
        // outstanding jobs, and a job's byte permit is only released when the
        // owning tick polls its completion. A worker therefore never blocks on
        // `ack_tx.send`, so "drain the accepted jobs, then join" is a valid
        // shutdown order. Changing either capacity invalidates that order; the
        // `accepted_jobs_never_block_workers_on_completion_slots` test pins the
        // behaviour.
        let (ack_tx, ack_rx) = mpsc::sync_channel::<WritebackAck>(max_jobs);
        let mut workers: Vec<thread::JoinHandle<()>> = Vec::with_capacity(worker_count);

        for index in 0..worker_count {
            let job_rx = Arc::clone(&job_rx);
            let ack_tx = ack_tx.clone();
            let worker = match thread::Builder::new()
                .name(format!("sc-chunk-writeback-{index}"))
                .spawn(move || writeback_worker(job_rx, ack_tx))
            {
                Ok(worker) => worker,
                Err(error) => {
                    drop(job_tx);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(error);
                }
            };
            workers.push(worker);
        }

        Ok(Self {
            inner: Arc::new(WritebackInner {
                jobs: Mutex::new(Some(job_tx)),
                acknowledgements: Mutex::new(ack_rx),
                state: Mutex::new(WritebackState::default()),
                workers: Mutex::new(Some(workers)),
                max_jobs,
                max_bytes,
            }),
        })
    }

    pub fn try_submit(
        &self,
        world_id: MinecraftWorldId,
        provider: &WorldChunkProvider,
        key: ChunkKey,
        column: Arc<ChunkColumn>,
    ) -> WritebackSubmit {
        let key = WritebackKey {
            world_id,
            chunk: key,
        };
        if !column.is_dirty() {
            return WritebackSubmit::Clean;
        }
        {
            let state = self.inner.state.lock();
            if state.in_flight.contains_key(&key) {
                return WritebackSubmit::AlreadyInFlight;
            }
            if state.retry_blocked(&key, Instant::now()) {
                return WritebackSubmit::Backoff;
            }
        }

        let estimated_bytes = column.writeback_snapshot_estimated_bytes();
        if estimated_bytes > self.inner.max_bytes {
            self.inner.state.lock().record_failure(key, Instant::now());
            return WritebackSubmit::Oversize;
        }
        // §3.2 dirty budget: charge the snapshot once it is accepted for
        // persistence. The charge is released again when the SaveAck confirms the
        // write (or when the reservation is cancelled), so the high-water mark
        // tracks *unacknowledged* dirty data only.
        provider.note_dirty_bytes(estimated_bytes);

        let attempt_id = {
            let mut state = self.inner.state.lock();
            if state.in_flight.contains_key(&key) {
                return WritebackSubmit::AlreadyInFlight;
            }
            if state.retry_blocked(&key, Instant::now()) {
                return WritebackSubmit::Backoff;
            }
            if state.in_flight.len() >= self.inner.max_jobs
                || state.in_flight_bytes.saturating_add(estimated_bytes) > self.inner.max_bytes
            {
                return WritebackSubmit::Busy;
            }
            let attempt_id = state.next_attempt_id;
            state.next_attempt_id = state.next_attempt_id.wrapping_add(1).max(1);
            state.in_flight.insert(
                key.clone(),
                InFlightWriteback {
                    attempt_id,
                    provider: provider.clone(),
                    dirty_bytes_charged: estimated_bytes,
                    reserved_bytes: estimated_bytes,
                    accepted_at: Instant::now(),
                    age_warning_emitted: false,
                },
            );
            state.in_flight_bytes += estimated_bytes;
            attempt_id
        };

        let snapshot = {
            let chunk = column.read();
            let actual_bytes = column.snapshot_estimated_bytes(&chunk);
            let (
                generation,
                block_entities_dirty,
                biomes_dirty,
                heightmap_dirty,
                is_dirty,
                spillover_acks,
            ) = column.writeback_state_with_spillover();
            if !is_dirty || actual_bytes > estimated_bytes {
                None
            } else {
                Some((
                    generation,
                    block_entities_dirty,
                    biomes_dirty,
                    heightmap_dirty,
                    spillover_acks,
                    chunk.clone(),
                ))
            }
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
            self.cancel_reservation(&key, attempt_id);
            return if column.is_dirty() {
                WritebackSubmit::Busy
            } else {
                WritebackSubmit::Clean
            };
        };

        let job = WritebackJob {
            key: key.clone(),
            attempt_id,
            column,
            generation,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            spillover_acks,
            storage: provider.storage_handle(),
            snapshot,
        };
        let admission = self
            .inner
            .jobs
            .lock()
            .as_ref()
            .map(|sender| sender.try_send(job));
        match admission {
            Some(Ok(())) => WritebackSubmit::Queued,
            Some(Err(mpsc::TrySendError::Full(_))) => {
                self.cancel_reservation(&key, attempt_id);
                WritebackSubmit::Busy
            }
            Some(Err(mpsc::TrySendError::Disconnected(_))) | None => {
                self.cancel_reservation(&key, attempt_id);
                WritebackSubmit::Closed
            }
        }
    }

    fn cancel_reservation(&self, key: &WritebackKey, attempt_id: u64) {
        let mut state = self.inner.state.lock();
        if state
            .in_flight
            .get(key)
            .is_some_and(|record| record.attempt_id == attempt_id)
        {
            if let Some(record) = state.in_flight.remove(key) {
                state.in_flight_bytes = state.in_flight_bytes.saturating_sub(record.reserved_bytes);
                record
                    .provider
                    .release_dirty_bytes(record.dirty_bytes_charged);
            }
        }
    }

    /// Submit a bounded cyclic window from one world's dirty columns.
    pub fn submit_dirty_window(
        &self,
        world_id: &MinecraftWorldId,
        provider: &WorldChunkProvider,
        max_submits: usize,
    ) -> usize {
        if max_submits == 0 || self.in_flight_jobs() >= self.inner.max_jobs {
            return 0;
        }
        let cursor = self.inner.state.lock().dirty_cursor.get(world_id).copied();
        let candidates =
            match provider.dirty_columns_window(cursor, MAX_DIRTY_CANDIDATES_PER_WORLD_TICK) {
                Ok(candidates) => candidates,
                Err(()) => {
                    log::warn!("{}", t_log!("console.world.scan_fail"));
                    return 0;
                }
            };
        let mut submitted = 0;
        let mut last_examined = None;
        for (key, column) in candidates {
            last_examined = Some(key);
            match self.try_submit(world_id.clone(), provider, key, column) {
                WritebackSubmit::Queued => {
                    submitted += 1;
                    if submitted >= max_submits {
                        break;
                    }
                }
                WritebackSubmit::Oversize => {
                    log::error!(
                        "{}",
                        t_log!(
                            "console.world.oversize_refused",
                            world = format!("{world_id:?}"),
                            chunk = format!("{key:?}")
                        )
                    );
                }
                _ => {}
            }
        }
        if let Some(cursor) = last_examined {
            self.inner
                .state
                .lock()
                .dirty_cursor
                .insert(world_id.clone(), cursor);
        }
        submitted
    }

    /// Rotate the first-serviced world so a large world's dirty set cannot
    /// consume every per-tick admission before other worlds are visited.
    pub fn submit_worlds_round_robin(
        &self,
        worlds: &[(MinecraftWorldId, WorldChunkProvider)],
        max_submits: usize,
    ) -> usize {
        if worlds.is_empty() || max_submits == 0 {
            return 0;
        }
        let start = self
            .inner
            .state
            .lock()
            .world_cursor
            .as_ref()
            .and_then(|cursor| worlds.iter().position(|(world_id, _)| world_id == cursor))
            .map_or(0, |index| (index + 1) % worlds.len());
        let mut submitted = 0;
        for offset in 0..worlds.len() {
            if submitted >= max_submits {
                break;
            }
            let (world_id, provider) = &worlds[(start + offset) % worlds.len()];
            submitted += self.submit_dirty_window(world_id, provider, max_submits - submitted);
            self.inner.state.lock().world_cursor = Some(world_id.clone());
        }
        submitted
    }

    pub fn poll_completed(&self, max_acks: usize) -> WritebackPollReport {
        let mut report = WritebackPollReport::default();
        for _ in 0..max_acks {
            let ack = {
                let receiver = self.inner.acknowledgements.lock();
                match receiver.try_recv() {
                    Ok(ack) => ack,
                    Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
                }
            };
            let mut state = self.inner.state.lock();
            let matching_attempt = state
                .in_flight
                .get(&ack.key)
                .is_some_and(|record| record.attempt_id == ack.attempt_id);
            if !matching_attempt {
                continue;
            }
            if let Some(record) = state.in_flight.remove(&ack.key) {
                state.in_flight_bytes = state.in_flight_bytes.saturating_sub(record.reserved_bytes);
                if ack.result.is_ok() {
                    // The snapshot is persisted; its dirty budget is free again.
                    record
                        .provider
                        .release_dirty_bytes(record.dirty_bytes_charged);
                }
            }
            match ack.result {
                Ok(())
                    if ack.column.take_dirty_if_generation_and_spillover(
                        ack.generation,
                        Some(&ack.spillover_acks),
                    ) =>
                {
                    state.clear_failure(&ack.key);
                    report.saved += 1;
                }
                Ok(()) => {
                    state.record_failure(ack.key.clone(), Instant::now());
                    report.superseded += 1;
                }
                Err(error) => {
                    log::warn!(
                        "{}",
                        t_log!(
                            "console.world.async_fail",
                            world = format!("{:?}", ack.key.world_id),
                            chunk = format!("{:?}", ack.key.chunk),
                            error = error
                        )
                    );
                    state.record_failure(ack.key.clone(), Instant::now());
                    report.failed += 1;
                }
            }
            report.completed += 1;
        }
        report
    }

    pub fn in_flight_jobs(&self) -> usize {
        self.inner.state.lock().in_flight.len()
    }

    pub fn in_flight_bytes(&self) -> usize {
        self.inner.state.lock().in_flight_bytes
    }

    /// Observe old accepted writebacks without releasing their permits or
    /// attempting to cancel storage calls that may already be mutating the DB.
    pub(crate) fn overdue_report(
        &self,
        threshold: Duration,
        sample_limit: usize,
    ) -> WritebackAgeReport {
        self.inner
            .state
            .lock()
            .overdue_report(Instant::now(), threshold, sample_limit)
    }

    /// Stop admission, drain accepted writes through the DB owner, and join.
    pub fn shutdown(&self) -> WritebackShutdownReport {
        self.shutdown_with_timeout(Self::DEFAULT_SHUTDOWN_TIMEOUT)
    }

    /// Default bounded shutdown wait.
    ///
    /// A storage backend that blocks forever (hung disk, stalled network mount)
    /// must not make shutdown wait forever while reporting nothing. After the
    /// budget expires the report states exactly which writes are unconfirmed;
    /// the worker threads are deliberately **not** killed while they may still be
    /// mutating the database.
    pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

    /// Shutdown with an explicit wait budget.
    ///
    /// The invariant "ack slot count == maximum accepted jobs" keeps the drain
    /// order valid: a permit is only released when its completion is polled, so
    /// the workers can always hand every accepted write back to the owner.
    pub fn shutdown_with_timeout(&self, timeout: Duration) -> WritebackShutdownReport {
        let _ = self.inner.jobs.lock().take();
        let Some(workers) = self.inner.workers.lock().take() else {
            return WritebackShutdownReport::default();
        };
        let deadline = Instant::now() + timeout;
        let mut joined = 0usize;
        let mut worker = workers.into_iter();
        let mut timed_out = false;
        for handle in worker.by_ref() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            // `JoinHandle::is_finished` needs no timeout-aware join helper and
            // keeps the OS thread alive if the deadline expires.
            while !handle.is_finished() {
                if Instant::now() >= deadline {
                    timed_out = true;
                    break;
                }
                std::thread::sleep(
                    Duration::from_millis(5).min(remaining.max(Duration::from_millis(1))),
                );
            }
            if handle.is_finished() {
                let _ = handle.join();
                joined += 1;
            } else {
                // Detach: never force-kill a thread that may be mid-write.
                break;
            }
        }
        let mut unconfirmed_jobs = 0usize;
        let mut unconfirmed_bytes = 0usize;
        if timed_out {
            let state = self.inner.state.lock();
            unconfirmed_jobs = state.in_flight.len();
            unconfirmed_bytes = state.in_flight_bytes;
        }
        WritebackShutdownReport {
            joined_workers: joined,
            timed_out,
            unconfirmed_jobs,
            unconfirmed_bytes,
        }
    }
}

/// What the bounded shutdown wait could confirm.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WritebackShutdownReport {
    /// Workers that finished and were joined.
    pub joined_workers: usize,
    /// The wait budget expired while a worker was still running.
    pub timed_out: bool,
    /// Accepted writes whose SaveAck was never observed.
    pub unconfirmed_jobs: usize,
    /// Estimated bytes retained by those writes.
    pub unconfirmed_bytes: usize,
}

impl WritebackShutdownReport {
    /// Whether every accepted write reached a confirmed terminal state.
    pub fn is_complete(&self) -> bool {
        !self.timed_out && self.unconfirmed_jobs == 0
    }
}

fn writeback_worker(
    receiver: Arc<Mutex<mpsc::Receiver<WritebackJob>>>,
    acknowledgements: mpsc::SyncSender<WritebackAck>,
) {
    loop {
        let job = {
            let guard = receiver.lock();
            guard.recv()
        };
        let Ok(job) = job else {
            return;
        };
        let WritebackJob {
            key,
            attempt_id,
            column,
            generation,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            spillover_acks,
            storage,
            snapshot,
        } = job;
        let result = match catch_unwind(AssertUnwindSafe(|| {
            storage
                .save_chunk_owned_with_metadata_and_spillover(
                    key.chunk,
                    snapshot,
                    block_entities_dirty,
                    biomes_dirty,
                    heightmap_dirty,
                    &spillover_acks,
                )
                .map_err(|error| error.to_string())
        })) {
            Ok(result) => result,
            Err(payload) => Err(format!(
                "writeback worker panicked: {}",
                panic_message(payload.as_ref())
            )),
        };
        drop(storage);
        let ack = WritebackAck {
            key,
            attempt_id,
            column,
            generation,
            spillover_acks,
            result,
        };
        if acknowledgements.send(ack).is_err() {
            return;
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{BlockRuntimeId, ChunkPosition};
    use crate::storage::{InMemoryWorldStorage, WorldStorageError};
    use sc_nbt::NbtValue;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn writeback_age_warnings_are_once_per_attempt_and_sample_bounded() {
        let now = Instant::now();
        let mut state = WritebackState::default();
        for x in 0..3 {
            let key = WritebackKey {
                world_id: MinecraftWorldId::random(),
                chunk: ChunkKey::new(0, ChunkPosition::new(x, 0)),
            };
            state.in_flight.insert(
                key,
                InFlightWriteback {
                    attempt_id: x as u64,
                    reserved_bytes: 1,
                    accepted_at: now - Duration::from_secs(20 + x as u64),
                    age_warning_emitted: false,
                    provider: WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default())),
                    dirty_bytes_charged: 0,
                },
            );
        }

        let first = state.overdue_report(now, Duration::from_secs(10), 1);
        assert_eq!(first.newly_overdue, 3);
        assert_eq!(first.overdue, 3);
        assert_eq!(first.key_samples.len(), 1);
        assert!(first
            .oldest_age
            .is_some_and(|age| age >= Duration::from_secs(22)));

        let subsequent = state.overdue_report(now, Duration::from_secs(10), 1);
        assert_eq!(
            subsequent.newly_overdue, 0,
            "do not warn repeatedly per attempt"
        );
        assert_eq!(subsequent.overdue, 3);
        assert!(subsequent.key_samples.is_empty());
    }

    struct MetadataTrackingStorage {
        saved: Mutex<Vec<(ChunkKey, bool, bool, bool)>>,
    }

    impl WorldStorage for MetadataTrackingStorage {
        fn load_chunk(&self, _key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
            Ok(None)
        }

        fn save_chunk(&self, _key: ChunkKey, _chunk: &Chunk) -> Result<(), WorldStorageError> {
            Err(WorldStorageError::Backend(
                "writeback unexpectedly used borrowed save".into(),
            ))
        }

        fn save_chunk_owned_with_metadata(
            &self,
            key: ChunkKey,
            _chunk: Chunk,
            block_entities_dirty: bool,
            biomes_dirty: bool,
            heightmap_dirty: bool,
        ) -> Result<(), WorldStorageError> {
            self.saved
                .lock()
                .push((key, block_entities_dirty, biomes_dirty, heightmap_dirty));
            Ok(())
        }
    }

    /// A storage backend that never returns models a permanently hung disk.
    struct HangingStorage {
        released: Arc<AtomicBool>,
    }

    impl WorldStorage for HangingStorage {
        fn load_chunk(&self, _key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
            Ok(None)
        }

        fn save_chunk(&self, _key: ChunkKey, _chunk: &Chunk) -> Result<(), WorldStorageError> {
            while !self.released.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        }

        fn save_chunk_owned_with_metadata(
            &self,
            key: ChunkKey,
            chunk: Chunk,
            block_entities_dirty: bool,
            biomes_dirty: bool,
            heightmap_dirty: bool,
        ) -> Result<(), WorldStorageError> {
            self.save_chunk_owned_with_metadata_and_spillover(
                key,
                chunk,
                block_entities_dirty,
                biomes_dirty,
                heightmap_dirty,
                &[],
            )
        }

        fn save_chunk_owned_with_metadata_and_spillover(
            &self,
            key: ChunkKey,
            chunk: Chunk,
            _block_entities_dirty: bool,
            _biomes_dirty: bool,
            _heightmap_dirty: bool,
            _spillover_acks: &[SpilloverOperationId],
        ) -> Result<(), WorldStorageError> {
            self.save_chunk(key, &chunk)
        }
    }

    fn dirty_column(provider: &WorldChunkProvider, key: ChunkKey) -> Arc<ChunkColumn> {
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        {
            let mut chunk = column.write();
            assert!(chunk
                .set_block_at(0, 1, -60, 2, BlockRuntimeId(11))
                .is_some());
            column.mark_dirty_locked(&chunk, -4);
        }
        column
    }

    /// A storage backend that records how many saves it was asked to perform.
    #[derive(Default)]
    struct CountingStorage {
        saves: Arc<AtomicUsize>,
    }

    impl WorldStorage for CountingStorage {
        fn load_chunk(&self, _key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
            Ok(None)
        }

        fn save_chunk(&self, _key: ChunkKey, _chunk: &Chunk) -> Result<(), WorldStorageError> {
            self.saves.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }

        fn save_chunk_owned_with_metadata(
            &self,
            key: ChunkKey,
            chunk: Chunk,
            block_entities_dirty: bool,
            biomes_dirty: bool,
            heightmap_dirty: bool,
        ) -> Result<(), WorldStorageError> {
            self.save_chunk_owned_with_metadata_and_spillover(
                key,
                chunk,
                block_entities_dirty,
                biomes_dirty,
                heightmap_dirty,
                &[],
            )
        }

        fn save_chunk_owned_with_metadata_and_spillover(
            &self,
            key: ChunkKey,
            chunk: Chunk,
            _block_entities_dirty: bool,
            _biomes_dirty: bool,
            _heightmap_dirty: bool,
            _spillover_acks: &[SpilloverOperationId],
        ) -> Result<(), WorldStorageError> {
            self.save_chunk(key, &chunk)
        }
    }

    /// Completion slots are reserved at admission: accepting the maximum number
    /// of jobs must not block any worker on the acknowledgement channel, so the
    /// shutdown order "join, then drain" stays valid (§13.2 / §16.3).
    #[test]
    fn confirmed_writes_release_the_dirty_budget_and_cancellations_too() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let executor = ChunkWritebackExecutor::with_limits(1, 4, 8 * 1024 * 1024)
            .expect("create writeback executor");

        let key = ChunkKey::new(0, ChunkPosition::new(11, 11));
        let column = dirty_column(&provider, key);
        assert_eq!(provider.dirty_bytes(), 0);
        assert_eq!(
            executor.try_submit(MinecraftWorldId::random(), &provider, key, column.clone()),
            WritebackSubmit::Queued
        );
        let charged = provider.dirty_bytes();
        assert!(charged > 0, "admission must charge the dirty budget");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut saved = 0usize;
        while saved == 0 && Instant::now() < deadline {
            saved = executor.poll_completed(8).saved;
            if saved == 0 {
                thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(saved, 1);
        assert_eq!(
            provider.dirty_bytes(),
            0,
            "a confirmed snapshot must release its dirty budget"
        );

        // Nothing may stay charged after the confirmation loop above.
        assert_eq!(provider.dirty_bytes(), 0);
        executor.shutdown_with_timeout(Duration::from_secs(5));
    }

    #[test]
    fn accepted_jobs_never_block_workers_on_completion_slots() {
        const MAX_JOBS: usize = 4;
        let saves = Arc::new(AtomicUsize::new(0));
        let provider = WorldChunkProvider::new(Arc::new(CountingStorage {
            saves: Arc::clone(&saves),
        }));
        let executor = ChunkWritebackExecutor::with_limits(2, MAX_JOBS, 64 * 1024 * 1024)
            .expect("create writeback executor");

        for index in 0..MAX_JOBS {
            let key = ChunkKey::new(0, ChunkPosition::new(index as i32, 0));
            let column = dirty_column(&provider, key);
            assert_eq!(
                executor.try_submit(MinecraftWorldId::random(), &provider, key, column),
                WritebackSubmit::Queued,
                "job {index} must fit the admission budget"
            );
        }
        // The admission budget is now full; nothing else may be accepted.
        let overflow_key = ChunkKey::new(0, ChunkPosition::new(99, 0));
        let overflow_column = dirty_column(&provider, overflow_key);
        assert_eq!(
            executor.try_submit(
                MinecraftWorldId::random(),
                &provider,
                overflow_key,
                overflow_column
            ),
            WritebackSubmit::Busy
        );

        // Without polling acknowledgements, every worker must still be able to
        // finish its writes: the slots were reserved when the jobs were admitted.
        let deadline = Instant::now() + Duration::from_secs(5);
        while saves.load(Ordering::Acquire) < MAX_JOBS && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            saves.load(Ordering::Acquire),
            MAX_JOBS,
            "a worker blocked on the completion channel would never store its snapshot"
        );

        // And every reserved slot is still observable by the owner.
        let mut completed = 0usize;
        while completed < MAX_JOBS {
            let report = executor.poll_completed(MAX_JOBS);
            completed += report.completed;
            if report.completed == 0 {
                break;
            }
        }
        assert_eq!(completed, MAX_JOBS);
        assert_eq!(executor.in_flight_jobs(), 0);
        assert_eq!(executor.in_flight_bytes(), 0);
    }

    #[test]
    fn bounded_shutdown_reports_unconfirmed_writes_instead_of_hanging_forever() {
        let released = Arc::new(AtomicBool::new(false));
        let provider = WorldChunkProvider::new(Arc::new(HangingStorage {
            released: Arc::clone(&released),
        }));
        let key = ChunkKey::new(0, ChunkPosition::new(5, 5));
        let column = dirty_column(&provider, key);
        let executor = ChunkWritebackExecutor::with_limits(1, 4, 8 * 1024 * 1024)
            .expect("create writeback executor");
        assert_eq!(
            executor.try_submit(MinecraftWorldId::random(), &provider, key, column),
            WritebackSubmit::Queued
        );
        assert_eq!(executor.in_flight_jobs(), 1);

        let report = executor.shutdown_with_timeout(Duration::from_millis(150));
        assert!(report.timed_out, "a hung storage backend must time out");
        assert!(
            !report.is_complete(),
            "a timed-out shutdown must not report a clean stop"
        );
        assert_eq!(report.unconfirmed_jobs, 1);
        assert!(report.unconfirmed_bytes > 0);

        // The worker was detached, not killed: it still delivers its receipt once
        // storage returns, so the owner can still confirm the write.
        released.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut drained = 0usize;
        while drained == 0 && Instant::now() < deadline {
            drained = executor.poll_completed(8).completed;
            if drained == 0 {
                thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(drained, 1, "the detached worker still hands back its ack");
        assert_eq!(executor.in_flight_jobs(), 0);
    }

    #[test]
    fn bounded_shutdown_reports_complete_when_workers_drain() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let key = ChunkKey::new(0, ChunkPosition::new(6, 6));
        let column = dirty_column(&provider, key);
        let executor = ChunkWritebackExecutor::with_limits(1, 4, 8 * 1024 * 1024)
            .expect("create writeback executor");
        assert_eq!(
            executor.try_submit(MinecraftWorldId::random(), &provider, key, column),
            WritebackSubmit::Queued
        );

        let report = executor.shutdown_with_timeout(Duration::from_secs(5));
        assert!(
            !report.timed_out,
            "a healthy worker must finish inside budget"
        );
        assert_eq!(report.joined_workers, 1);
        assert!(report.is_complete());
        assert_eq!(report.unconfirmed_jobs, 0);
        // The accepted write is drained into the ack channel, not lost.
        assert_eq!(executor.poll_completed(8).completed, 1);
    }

    #[test]
    fn writeback_passes_block_entity_dirty_identity_to_storage() {
        let storage = Arc::new(MetadataTrackingStorage {
            saved: Mutex::new(Vec::new()),
        });
        let provider = WorldChunkProvider::new(storage.clone());
        let subchunk_key = ChunkKey::new(0, ChunkPosition::new(20, 2));
        let metadata_key = ChunkKey::new(0, ChunkPosition::new(21, 2));
        let biome_key = ChunkKey::new(0, ChunkPosition::new(22, 2));
        let subchunk_column = provider
            .ensure_chunk(subchunk_key, -64, 319)
            .expect("ensure subchunk column");
        let metadata_column = provider
            .ensure_chunk(metadata_key, -64, 319)
            .expect("ensure metadata column");
        let biome_column = provider
            .ensure_chunk(biome_key, -64, 319)
            .expect("ensure biome column");
        subchunk_column.mark_dirty(-4);
        metadata_column.mark_block_entities_dirty();
        biome_column.mark_biomes_dirty();

        let executor = ChunkWritebackExecutor::with_limits(1, 4, 16 * 1024 * 1024)
            .expect("create writeback executor");
        let world_id = MinecraftWorldId::random();
        assert_eq!(
            executor.try_submit(world_id.clone(), &provider, subchunk_key, subchunk_column),
            WritebackSubmit::Queued
        );
        assert_eq!(
            executor.try_submit(world_id, &provider, metadata_key, metadata_column.clone()),
            WritebackSubmit::Queued
        );
        assert_eq!(
            executor.try_submit(
                MinecraftWorldId::random(),
                &provider,
                biome_key,
                biome_column.clone(),
            ),
            WritebackSubmit::Queued
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut completed = 0;
        while completed < 3 && Instant::now() < deadline {
            completed += executor.poll_completed(4).completed;
            if completed < 3 {
                thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(completed, 3);
        let flags = storage.saved.lock().clone();
        assert!(flags.contains(&(subchunk_key, false, false, true)));
        assert!(flags.contains(&(metadata_key, true, false, false)));
        assert!(flags.contains(&(biome_key, false, true, false)));
        assert!(!metadata_column.is_dirty());
        assert!(!biome_column.is_dirty());
        executor.shutdown();
    }

    #[test]
    fn oversized_nested_nbt_snapshot_is_rejected_before_clone_and_stays_dirty() {
        let provider = WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default()));
        let key = ChunkKey::new(0, ChunkPosition::new(23, 5));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        let base_bytes = column.writeback_snapshot_estimated_bytes();
        let payload = vec![7i8; 128 * 1024];
        let payload_bytes = payload.capacity();
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

        let estimated_bytes = column.writeback_snapshot_estimated_bytes();
        assert!(estimated_bytes >= base_bytes.saturating_add(payload_bytes));
        let executor = ChunkWritebackExecutor::with_limits(1, 1, base_bytes + 64 * 1024)
            .expect("create bounded executor");
        assert_eq!(
            executor.try_submit(MinecraftWorldId::random(), &provider, key, column.clone()),
            WritebackSubmit::Oversize,
            "nested NBT clone buffers are part of the snapshot admission limit"
        );
        assert_eq!(executor.in_flight_bytes(), 0);
        assert!(
            column.is_dirty(),
            "rejected snapshots keep their dirty data"
        );
        executor.shutdown();
    }

    #[test]
    fn async_writeback_saves_snapshot_and_owner_poll_clears_dirty() {
        let storage = Arc::new(InMemoryWorldStorage::default());
        let provider = WorldChunkProvider::new(storage.clone());
        let key = ChunkKey::new(0, ChunkPosition::new(2, 3));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        {
            let mut chunk = column.write();
            assert!(chunk
                .set_block_at(0, 1, -60, 2, BlockRuntimeId(11))
                .is_some());
            column.mark_dirty_locked(&chunk, -4);
        }

        let executor = ChunkWritebackExecutor::with_limits(1, 4, 8 * 1024 * 1024)
            .expect("create writeback executor");
        assert_eq!(
            executor.try_submit(MinecraftWorldId::random(), &provider, key, column.clone()),
            WritebackSubmit::Queued
        );
        assert_eq!(executor.in_flight_jobs(), 1);
        assert!(executor.in_flight_bytes() > 0);

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut poll = WritebackPollReport::default();
        while poll.completed == 0 && Instant::now() < deadline {
            poll = executor.poll_completed(8);
            if poll.completed == 0 {
                thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(
            poll.saved, 1,
            "owner poll acknowledges the unchanged snapshot"
        );
        assert!(poll.failed == 0 && poll.superseded == 0);
        assert!(!column.is_dirty());
        assert!(provider
            .dirty_columns_window(None, 4)
            .expect("clean indexed window")
            .is_empty());
        assert_eq!(executor.in_flight_jobs(), 0);
        assert_eq!(executor.in_flight_bytes(), 0);
        let saved = storage
            .load_chunk(key)
            .expect("load persisted chunk")
            .expect("saved chunk exists");
        assert_eq!(saved.block_at_layer(0, 1, -60, 2), Some(BlockRuntimeId(11)));
        executor.shutdown();
    }

    struct OwnedOnlyStorage {
        saved: Mutex<Vec<ChunkKey>>,
    }

    impl WorldStorage for OwnedOnlyStorage {
        fn load_chunk(&self, _key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
            Ok(None)
        }

        fn save_chunk(&self, _key: ChunkKey, _chunk: &Chunk) -> Result<(), WorldStorageError> {
            Err(WorldStorageError::Backend(
                "writeback unexpectedly used the borrowed storage path".into(),
            ))
        }

        fn save_chunk_owned(&self, key: ChunkKey, _chunk: Chunk) -> Result<(), WorldStorageError> {
            self.saved.lock().push(key);
            Ok(())
        }
    }

    #[test]
    fn async_writeback_transfers_owned_snapshot_to_storage() {
        let storage = Arc::new(OwnedOnlyStorage {
            saved: Mutex::new(Vec::new()),
        });
        let provider = WorldChunkProvider::new(storage.clone());
        let key = ChunkKey::new(0, ChunkPosition::new(6, 7));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        column.mark_dirty(-4);

        let executor = ChunkWritebackExecutor::with_limits(1, 2, 8 * 1024 * 1024)
            .expect("create writeback executor");
        assert_eq!(
            executor.try_submit(MinecraftWorldId::random(), &provider, key, column.clone()),
            WritebackSubmit::Queued
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut poll = WritebackPollReport::default();
        while poll.completed == 0 && Instant::now() < deadline {
            poll = executor.poll_completed(4);
            if poll.completed == 0 {
                thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(poll.saved, 1, "owned storage call received a terminal ack");
        assert_eq!(*storage.saved.lock(), vec![key]);
        assert!(!column.is_dirty());
        executor.shutdown();
    }

    struct BlockingStorage {
        started: std::sync::mpsc::Sender<BlockRuntimeId>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl WorldStorage for BlockingStorage {
        fn load_chunk(&self, _key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
            Ok(None)
        }

        fn save_chunk(&self, _key: ChunkKey, chunk: &Chunk) -> Result<(), WorldStorageError> {
            let value = chunk
                .block_at_layer(0, 1, -60, 2)
                .ok_or_else(|| WorldStorageError::Backend("snapshot missing test block".into()))?;
            self.started
                .send(value)
                .map_err(|error| WorldStorageError::Backend(error.to_string()))?;
            self.release
                .lock()
                .recv()
                .map_err(|error| WorldStorageError::Backend(error.to_string()))?;
            Ok(())
        }
    }

    #[test]
    fn writeback_bytes_are_reserved_and_stale_ack_keeps_newer_generation_dirty() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let storage = Arc::new(BlockingStorage {
            started: started_tx,
            release: Mutex::new(release_rx),
        });
        let provider = WorldChunkProvider::new(storage);
        let first_key = ChunkKey::new(0, ChunkPosition::new(4, 4));
        let second_key = ChunkKey::new(0, ChunkPosition::new(5, 4));
        let first = provider.ensure_chunk(first_key, -64, 319).expect("first");
        let second = provider.ensure_chunk(second_key, -64, 319).expect("second");
        for column in [&first, &second] {
            {
                let mut chunk = column.write();
                assert!(chunk
                    .set_block_at(0, 1, -60, 2, BlockRuntimeId(21))
                    .is_some());
                column.mark_dirty_locked(&chunk, -4);
            }
        }
        let one_snapshot_bytes = first.writeback_snapshot_estimated_bytes();
        assert_eq!(
            one_snapshot_bytes,
            second.writeback_snapshot_estimated_bytes()
        );
        let executor = ChunkWritebackExecutor::with_limits(1, 2, one_snapshot_bytes)
            .expect("create bounded executor");
        let world_id = MinecraftWorldId::random();
        assert_eq!(
            executor.try_submit(world_id.clone(), &provider, first_key, first.clone()),
            WritebackSubmit::Queued
        );
        assert_eq!(
            started_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("worker entered storage"),
            BlockRuntimeId(21)
        );
        assert_eq!(
            executor.try_submit(world_id, &provider, second_key, second),
            WritebackSubmit::Busy,
            "snapshot byte reservation prevents over-admission"
        );
        assert_eq!(executor.in_flight_bytes(), one_snapshot_bytes);

        {
            let mut chunk = first.write();
            assert!(chunk
                .set_block_at(0, 1, -60, 2, BlockRuntimeId(22))
                .is_some());
            first.mark_dirty_locked(&chunk, -4);
        }
        release_tx.send(()).expect("release storage worker");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut poll = WritebackPollReport::default();
        while poll.completed == 0 && Instant::now() < deadline {
            poll = executor.poll_completed(4);
            if poll.completed == 0 {
                thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(
            poll.superseded, 1,
            "old snapshot cannot clear a newer generation"
        );
        assert!(first.is_dirty());
        let indexed_dirty_keys = provider
            .dirty_columns_window(None, 4)
            .expect("newer dirty generation remains indexed")
            .iter()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        assert!(indexed_dirty_keys.contains(&first_key));
        assert!(indexed_dirty_keys.contains(&second_key));
        assert_eq!(executor.in_flight_bytes(), 0);
        executor.shutdown();
    }

    #[test]
    fn shutdown_joins_accepted_write_then_owner_can_drain_reserved_ack() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let storage = Arc::new(BlockingStorage {
            started: started_tx,
            release: Mutex::new(release_rx),
        });
        let provider = WorldChunkProvider::new(storage);
        let key = ChunkKey::new(0, ChunkPosition::new(8, 9));
        let column = provider.ensure_chunk(key, -64, 319).expect("ensure column");
        column.mark_dirty(-4);
        let executor = ChunkWritebackExecutor::with_limits(1, 2, 8 * 1024 * 1024)
            .expect("create writeback executor");
        assert_eq!(
            executor.try_submit(MinecraftWorldId::random(), &provider, key, column.clone()),
            WritebackSubmit::Queued
        );
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker entered storage");

        let shutdown_executor = executor.clone();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
        let shutdown_thread = thread::spawn(move || {
            shutdown_executor.shutdown();
            let _ = shutdown_tx.send(());
        });
        assert!(
            shutdown_rx.recv_timeout(Duration::from_millis(20)).is_err(),
            "shutdown waits for the accepted storage operation"
        );
        release_tx.send(()).expect("release storage worker");
        shutdown_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("shutdown must not deadlock on the reserved ack slot");
        shutdown_thread.join().expect("shutdown thread");

        let report = executor.poll_completed(2);
        assert_eq!(report.saved, 1);
        assert!(!column.is_dirty());
        assert_eq!(executor.in_flight_jobs(), 0);
        assert_eq!(executor.in_flight_bytes(), 0);
    }
}
