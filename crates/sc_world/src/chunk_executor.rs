//! `WorldChunkExecutor`: bounded worker pool for chunk loading/generation.
//!
//! Blocking work (LevelDB reads and CPU worldgen) runs on dedicated OS threads:
//! network tasks only `submit` a job and await a oneshot receipt,
//! never calling `load_chunk` on Tokio.
//!
//! Same-key dedup (request coalescing): when a `(world, chunk)` load is already queued or running,
//! new requests attach to the job waiter list instead of re-queuing; workers fan the result
//! out to all coalesced waiters at completion. Later requests hit the
//! `WorldChunkProvider` internal cache (fast path).
//!
//! Shutdown: `shutdown` drops the job sender first (workers exit after draining), then joins
//! threads; callers then write back dirty chunks.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use sc_ecs::resource::Resource;
use tokio::sync::oneshot;

use crate::manager::MinecraftWorldId;
use crate::storage::{ChunkColumn, ChunkKey, WorldChunkProvider, WorldStorageError};
use sc_log::t_log;

/// Result of one chunk load/generate job.
pub type ChunkLoadOutcome = Result<Option<Arc<ChunkColumn>>, WorldStorageError>;

/// Synchronous admission result for a chunk load/generation request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkSubmitError {
    /// The bounded worker queue is full or this key is in failure backoff.
    /// Retry later without blocking the caller.
    Busy,
    /// The executor is shutting down or its worker channel is closed.
    Closed,
    /// This chunk already has the maximum number of coalesced consumers.
    TooManyWaiters,
}

/// Dedup key: chunk ownership `(saved world, chunk coords)`. Different worlds never coalesce
/// even with equal dimension+coords (same semantics as the `LevelChunkCache` world-isolation key).
#[derive(Clone, Hash, Eq, PartialEq)]
struct ExecutorKey {
    world: MinecraftWorldId,
    chunk: ChunkKey,
}

/// One unit of work delivered to a worker thread.
struct ChunkJob {
    provider: WorldChunkProvider,
    key: ChunkKey,
    min_y: i32,
    max_y: i32,
    dedupe_key: ExecutorKey,
}

/// In-flight coalescing table: dedup key to receipt senders waiting on the job result.
type WaiterMap = HashMap<ExecutorKey, Vec<oneshot::Sender<Arc<ChunkLoadOutcome>>>>;

const MAX_FAILURE_BACKOFF_KEYS: usize = 4096;
const INITIAL_FAILURE_BACKOFF: Duration = Duration::from_millis(50);
const MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
struct FailureRecord {
    retry_at: Instant,
    attempts: u8,
}

/// Bounded retry cooldown for repeatedly failing `(world, chunk)` jobs.
/// The FIFO order also makes capacity eviction deterministic and cheap.
#[derive(Default)]
struct FailureBackoff {
    records: HashMap<ExecutorKey, FailureRecord>,
    order: VecDeque<ExecutorKey>,
}

impl FailureBackoff {
    fn is_blocked(&self, key: &ExecutorKey, now: Instant) -> bool {
        self.records
            .get(key)
            .is_some_and(|record| record.retry_at > now)
    }

    fn record_failure(&mut self, key: ExecutorKey, now: Instant) {
        let attempts = if let Some(record) = self.records.get_mut(&key) {
            record.attempts = record.attempts.saturating_add(1);
            record.attempts
        } else {
            while self.records.len() >= MAX_FAILURE_BACKOFF_KEYS {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                self.records.remove(&oldest);
            }
            self.order.push_back(key.clone());
            self.records.insert(
                key.clone(),
                FailureRecord {
                    retry_at: now,
                    attempts: 0,
                },
            );
            1
        };
        let shift = u32::from(attempts.saturating_sub(1).min(6));
        let delay = INITIAL_FAILURE_BACKOFF
            .checked_mul(1u32 << shift)
            .unwrap_or(MAX_FAILURE_BACKOFF)
            .min(MAX_FAILURE_BACKOFF);
        if let Some(record) = self.records.get_mut(&key) {
            record.retry_at = now + delay;
            record.attempts = attempts;
        }
    }

    fn clear(&mut self, key: &ExecutorKey) {
        if self.records.remove(key).is_some() {
            self.order.retain(|entry| entry != key);
        }
    }
}

struct ExecutorInner {
    /// Bounded job sender. Submissions use `try_send` and shed load when full, so
    /// Tokio tasks, Chunk clones, and waiters never accumulate without bound.
    jobs: Mutex<Option<mpsc::SyncSender<ChunkJob>>>,
    /// In-flight coalescing table (same-key dedup). `Arc` because workers share it before inner exists.
    inflight: Arc<Mutex<WaiterMap>>,
    /// Bounded same-key cooldown after worker/storage/generator failures.
    failures: Arc<Mutex<FailureBackoff>>,
    /// Worker thread handles (taken and joined at `shutdown`; `Option` keeps it idempotent).
    workers: Mutex<Option<Vec<thread::JoinHandle<()>>>>,
}

/// Chunk load/generate executor (Resource, accepts `chunk_pipeline` submissions).
#[derive(Resource, Clone)]
pub struct WorldChunkExecutor {
    inner: Arc<ExecutorInner>,
}

impl WorldChunkExecutor {
    const JOB_QUEUE_CAPACITY: usize = 1024;
    const MAX_WAITERS_PER_CHUNK: usize = 64;

    /// Start `worker_count` dedicated chunk worker threads.
    pub fn new(worker_count: usize) -> io::Result<Self> {
        let worker_count = worker_count.max(1);
        let (tx, rx) = mpsc::sync_channel::<ChunkJob>(Self::JOB_QUEUE_CAPACITY);
        let rx = Arc::new(Mutex::new(rx));
        let inflight: Arc<Mutex<WaiterMap>> = Arc::new(Mutex::new(HashMap::new()));
        let failures = Arc::new(Mutex::new(FailureBackoff::default()));
        let mut workers: Vec<thread::JoinHandle<()>> = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let rx = rx.clone();
            let inflight = inflight.clone();
            let failures = failures.clone();
            let handle = match thread::Builder::new()
                .name(format!("sc-chunk-worker-{index}"))
                .spawn(move || worker_loop(rx, inflight, failures))
            {
                Ok(handle) => handle,
                Err(error) => {
                    drop(tx);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(error);
                }
            };
            workers.push(handle);
        }
        Ok(Self {
            inner: Arc::new(ExecutorInner {
                jobs: Mutex::new(Some(tx)),
                inflight,
                failures,
                workers: Mutex::new(Some(workers)),
            }),
        })
    }

    /// Default worker count: half the available parallelism, clamped to [2, 8].
    pub fn default_worker_count() -> usize {
        let parallelism = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        (parallelism / 2).clamp(2, 8)
    }

    /// Compatibility wrapper retaining the original receiver-returning API.
    ///
    /// New code should use [`Self::try_submit`] to distinguish queue pressure
    /// from shutdown. This wrapper is also non-blocking; rejected admission is
    /// represented as a closed receiver for callers using the older API.
    pub fn submit(
        &self,
        world: MinecraftWorldId,
        provider: WorldChunkProvider,
        key: ChunkKey,
        min_y: i32,
        max_y: i32,
    ) -> oneshot::Receiver<Arc<ChunkLoadOutcome>> {
        match self.try_submit(world, provider, key, min_y, max_y) {
            Ok(receiver) => receiver,
            Err(_) => {
                let (sender, receiver) = oneshot::channel();
                drop(sender);
                receiver
            }
        }
    }

    /// Submit a load/generation request without blocking the caller.
    ///
    /// Concurrent requests for the same `(world, chunk)` share one job and
    /// receive its result. A new job is admitted only while holding the short
    /// in-flight lock and using `try_send`, so a full worker queue cannot block
    /// either a Tokio worker or the ECS tick thread. On failed admission the
    /// in-flight registration is removed before returning the explicit reason.
    pub fn try_submit(
        &self,
        world: MinecraftWorldId,
        provider: WorldChunkProvider,
        key: ChunkKey,
        min_y: i32,
        max_y: i32,
    ) -> Result<oneshot::Receiver<Arc<ChunkLoadOutcome>>, ChunkSubmitError> {
        let (tx, rx) = oneshot::channel();
        let dedupe_key = ExecutorKey { world, chunk: key };
        let mut inflight = self.inner.inflight.lock();
        if let Some(waiters) = inflight.get_mut(&dedupe_key) {
            // A dropped oneshot receiver releases its demand lease. Prune it
            // before enforcing the per-key consumer bound.
            waiters.retain(|waiter| !waiter.is_closed());
            if waiters.len() >= Self::MAX_WAITERS_PER_CHUNK {
                return Err(ChunkSubmitError::TooManyWaiters);
            }
            waiters.push(tx);
            return Ok(rx);
        }

        if self
            .inner
            .failures
            .lock()
            .is_blocked(&dedupe_key, Instant::now())
        {
            return Err(ChunkSubmitError::Busy);
        }

        inflight.insert(dedupe_key.clone(), vec![tx]);
        let job = ChunkJob {
            provider,
            key,
            min_y,
            max_y,
            dedupe_key: dedupe_key.clone(),
        };
        let admission = self
            .inner
            .jobs
            .lock()
            .as_ref()
            .map(|sender| sender.try_send(job));
        match admission {
            Some(Ok(())) => Ok(rx),
            Some(Err(mpsc::TrySendError::Full(_))) => {
                inflight.remove(&dedupe_key);
                Err(ChunkSubmitError::Busy)
            }
            Some(Err(mpsc::TrySendError::Disconnected(_))) | None => {
                inflight.remove(&dedupe_key);
                Err(ChunkSubmitError::Closed)
            }
        }
    }

    /// Current in-flight (queued or running) deduped job count (observability/test use).
    pub fn in_flight(&self) -> usize {
        self.inner.inflight.lock().len()
    }

    /// Shut down: drop the job sender (workers exit after draining) and join all threads. Idempotent.
    pub fn shutdown(&self) {
        // Drop the sender so the receiver sees disconnect after draining, then workers exit.
        let _ = self.inner.jobs.lock().take();
        if let Some(workers) = self.inner.workers.lock().take() {
            for handle in workers {
                let _ = handle.join();
            }
        }
    }
}

/// Worker main loop: take a job, run the blocking load/generate on this thread, fan out to coalesced waiters.
fn worker_loop(
    rx: Arc<Mutex<mpsc::Receiver<ChunkJob>>>,
    inflight: Arc<Mutex<WaiterMap>>,
    failures: Arc<Mutex<FailureBackoff>>,
) {
    loop {
        // Block for the next job; exit when the channel is disconnected after draining.
        // recv may block while holding the rx lock, so only one worker recvs at a time;
        // the rest wait on the lock while loads/generations run concurrently outside it.
        let job = {
            let guard = rx.lock();
            guard.recv()
        };
        let Ok(job) = job else {
            return;
        };
        // A queued visual demand may have been cancelled while waiting for a
        // worker (for example, its player changed hard context). Skip the load
        // if no consumer remains; running generators are not force-cancelled.
        let has_live_waiters = {
            let mut waiters_by_key = inflight.lock();
            let has_live_waiters = waiters_by_key
                .get_mut(&job.dedupe_key)
                .is_some_and(|waiters| {
                    waiters.retain(|waiter| !waiter.is_closed());
                    !waiters.is_empty()
                });
            if !has_live_waiters {
                waiters_by_key.remove(&job.dedupe_key);
            }
            has_live_waiters
        };
        if !has_live_waiters {
            continue;
        }
        // Blocking work (LevelDB I/O + CPU generation) stays on worker threads, off Tokio.
        // Generator/storage implementations are plugin-controlled boundaries;
        // isolate a panic to this job so all waiters receive a terminal error and
        // the shared worker can continue processing later requests.
        let outcome = Arc::new(
            match catch_unwind(AssertUnwindSafe(|| {
                job.provider.load_chunk(job.key, job.min_y, job.max_y)
            })) {
                Ok(outcome) => outcome,
                Err(payload) => {
                    let reason = payload
                        .downcast_ref::<&str>()
                        .map(|message| (*message).to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "non-string panic payload".to_string());
                    log::error!(
                        "{}",
                        t_log!(
                            "console.world.load_panic",
                            world = format!("{:?}", job.dedupe_key.world),
                            key = format!("{:?}", job.key),
                            reason = reason
                        )
                    );
                    Err(WorldStorageError::Backend(format!(
                        "chunk load worker panicked: {reason}"
                    )))
                }
            },
        );
        // Update retry state while still holding `inflight`: try_submit uses
        // the same lock order, so a new request cannot slip between failure
        // publication and removal of this attempt.
        let mut waiters_by_key = inflight.lock();
        {
            let mut backoff = failures.lock();
            if outcome.is_err() {
                backoff.record_failure(job.dedupe_key.clone(), Instant::now());
            } else {
                backoff.clear(&job.dedupe_key);
            }
        }
        if let Some(mut waiters) = waiters_by_key.remove(&job.dedupe_key) {
            waiters.retain(|waiter| !waiter.is_closed());
            for waiter in waiters {
                let _ = waiter.send(outcome.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPosition};
    use crate::storage::{
        ChunkGenerationRequest, InMemoryWorldStorage, WorldGenerator, WorldStorage,
    };

    /// Counting generator: counts actual generations (verifies same-key dedup).
    struct CountingGenerator {
        count: std::sync::atomic::AtomicU32,
    }
    impl CountingGenerator {
        fn new() -> Self {
            Self {
                count: std::sync::atomic::AtomicU32::new(0),
            }
        }
        fn count(&self) -> u32 {
            self.count.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
    impl WorldGenerator for CountingGenerator {
        fn generate_chunk(
            &self,
            request: ChunkGenerationRequest,
        ) -> Result<Option<Chunk>, WorldStorageError> {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Small delay to widen the window for coalescing concurrent requests into one job.
            std::thread::sleep(std::time::Duration::from_millis(20));
            Ok(Some(Chunk::empty(
                request.key.position,
                request.key.dimension,
                request.min_y,
                request.max_y,
            )))
        }
    }

    fn provider_with_generator(generator: Arc<dyn WorldGenerator>) -> WorldChunkProvider {
        WorldChunkProvider::new(Arc::new(InMemoryWorldStorage::default())).with_generator(generator)
    }

    struct PanicOnceGenerator {
        panic_once: std::sync::atomic::AtomicBool,
    }

    impl WorldGenerator for PanicOnceGenerator {
        fn generate_chunk(
            &self,
            request: ChunkGenerationRequest,
        ) -> Result<Option<Chunk>, WorldStorageError> {
            if !self
                .panic_once
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                panic!("synthetic chunk generation panic");
            }
            Ok(Some(Chunk::empty(
                request.key.position,
                request.key.dimension,
                request.min_y,
                request.max_y,
            )))
        }
    }

    struct BlockingGenerator {
        started: std::sync::mpsc::Sender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        blocked_once: std::sync::atomic::AtomicBool,
        calls: std::sync::atomic::AtomicU32,
    }

    impl WorldGenerator for BlockingGenerator {
        fn generate_chunk(
            &self,
            request: ChunkGenerationRequest,
        ) -> Result<Option<Chunk>, WorldStorageError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if !self
                .blocked_once
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                let _ = self.started.send(());
                self.release
                    .lock()
                    .recv()
                    .map_err(|error| WorldStorageError::Backend(error.to_string()))?;
            }
            Ok(Some(Chunk::empty(
                request.key.position,
                request.key.dimension,
                request.min_y,
                request.max_y,
            )))
        }
    }

    #[test]
    fn generator_panic_completes_waiters_and_worker_accepts_next_job() {
        let executor = WorldChunkExecutor::new(1).expect("create chunk executor");
        let provider = provider_with_generator(Arc::new(PanicOnceGenerator {
            panic_once: std::sync::atomic::AtomicBool::new(false),
        }));
        let world = MinecraftWorldId::random();
        let key = ChunkKey::new(0, ChunkPosition::new(12, -4));
        let timeout = std::time::Duration::from_secs(2);

        let first = executor
            .try_submit(world.clone(), provider.clone(), key, -64, 319)
            .expect("admit panic-once request");
        let outcome = futures_block_on_with_timeout(first, timeout)
            .expect("panic must not strand the waiter")
            .expect("worker must send a terminal outcome");
        assert!(outcome.is_err());
        assert_eq!(executor.in_flight(), 0);
        assert!(matches!(
            executor.try_submit(world.clone(), provider.clone(), key, -64, 319),
            Err(ChunkSubmitError::Busy)
        ));

        let second_key = ChunkKey::new(0, ChunkPosition::new(13, -4));
        let second = executor
            .try_submit(world, provider, second_key, -64, 319)
            .expect("worker must remain available after a job panic");
        let outcome = futures_block_on_with_timeout(second, timeout)
            .expect("second job must complete")
            .expect("second waiter must receive a result");
        assert!(matches!(outcome.as_ref(), Ok(Some(_))));
        executor.shutdown();
    }

    #[test]
    fn queued_job_is_skipped_after_its_only_consumer_is_dropped() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let generator = Arc::new(BlockingGenerator {
            started: started_tx,
            release: Mutex::new(release_rx),
            blocked_once: std::sync::atomic::AtomicBool::new(false),
            calls: std::sync::atomic::AtomicU32::new(0),
        });
        let executor = WorldChunkExecutor::new(1).expect("create chunk executor");
        let provider = provider_with_generator(generator.clone());
        let world = MinecraftWorldId::random();
        let first = executor
            .try_submit(
                world.clone(),
                provider.clone(),
                ChunkKey::new(0, ChunkPosition::new(0, 0)),
                -64,
                319,
            )
            .expect("admit running request");
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("first generation entered blocking section");

        let cancelled = executor
            .try_submit(
                world,
                provider,
                ChunkKey::new(0, ChunkPosition::new(1, 0)),
                -64,
                319,
            )
            .expect("admit queued request");
        drop(cancelled);
        release_tx.send(()).expect("release first generation");
        let first_result = futures_block_on_with_timeout(first, std::time::Duration::from_secs(2))
            .expect("first job completed")
            .expect("first waiter received result");
        assert!(matches!(first_result.as_ref(), Ok(Some(_))));

        executor.shutdown();
        assert_eq!(generator.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(executor.in_flight(), 0);
    }

    #[test]
    fn submit_returns_busy_without_leaking_in_flight_registration() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let generator = Arc::new(BlockingGenerator {
            started: started_tx,
            release: Mutex::new(release_rx),
            blocked_once: std::sync::atomic::AtomicBool::new(false),
            calls: std::sync::atomic::AtomicU32::new(0),
        });
        let executor = WorldChunkExecutor::new(1).expect("create chunk executor");
        let provider = provider_with_generator(generator);
        let world = MinecraftWorldId::random();

        let running_receiver = executor
            .try_submit(
                world.clone(),
                provider.clone(),
                ChunkKey::new(0, ChunkPosition::new(0, 0)),
                -64,
                319,
            )
            .expect("admit running request");
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("generator entered blocking section");
        // The job is already running; dropping its only consumer must not abort
        // a generator that has crossed the safe cancellation boundary.
        drop(running_receiver);

        for index in 0..WorldChunkExecutor::JOB_QUEUE_CAPACITY {
            executor
                .try_submit(
                    world.clone(),
                    provider.clone(),
                    ChunkKey::new(0, ChunkPosition::new(index as i32 + 1, 0)),
                    -64,
                    319,
                )
                .expect("admit request into bounded queue");
        }
        let rejected_key = ChunkKey::new(
            0,
            ChunkPosition::new(WorldChunkExecutor::JOB_QUEUE_CAPACITY as i32 + 1, 0),
        );
        assert!(matches!(
            executor.try_submit(world.clone(), provider.clone(), rejected_key, -64, 319),
            Err(ChunkSubmitError::Busy),
        ));
        let mut legacy_receiver = executor.submit(
            world,
            provider,
            ChunkKey::new(0, ChunkPosition::new(rejected_key.position.x + 1, 0)),
            -64,
            319,
        );
        assert!(matches!(
            legacy_receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed),
        ));
        assert_eq!(
            executor.in_flight(),
            WorldChunkExecutor::JOB_QUEUE_CAPACITY + 1
        );

        release_tx.send(()).expect("release generator");
        executor.shutdown();
        assert_eq!(executor.in_flight(), 0);
    }

    #[test]
    fn submit_distinguishes_waiter_limit_and_closed_executor() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let generator = Arc::new(BlockingGenerator {
            started: started_tx,
            release: Mutex::new(release_rx),
            blocked_once: std::sync::atomic::AtomicBool::new(false),
            calls: std::sync::atomic::AtomicU32::new(0),
        });
        let executor = WorldChunkExecutor::new(1).expect("create chunk executor");
        let provider = provider_with_generator(generator);
        let world = MinecraftWorldId::random();
        let key = ChunkKey::new(0, ChunkPosition::new(5, 6));
        let mut receivers = vec![executor
            .try_submit(world.clone(), provider.clone(), key, -64, 319)
            .expect("admit first waiter")];
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("generator entered blocking section");

        for _ in 1..WorldChunkExecutor::MAX_WAITERS_PER_CHUNK {
            receivers.push(
                executor
                    .try_submit(world.clone(), provider.clone(), key, -64, 319)
                    .expect("admit coalesced waiter"),
            );
        }
        assert!(matches!(
            executor.try_submit(world.clone(), provider.clone(), key, -64, 319),
            Err(ChunkSubmitError::TooManyWaiters),
        ));
        release_tx.send(()).expect("release generator");
        for receiver in receivers {
            assert!(futures_block_on(receiver).is_ok());
        }
        executor.shutdown();
        assert!(matches!(
            executor.try_submit(
                world,
                provider,
                ChunkKey::new(0, ChunkPosition::new(9, 9)),
                -64,
                319,
            ),
            Err(ChunkSubmitError::Closed),
        ));
    }

    #[test]
    fn submit_loads_and_returns_generated_chunk() {
        let executor = WorldChunkExecutor::new(2).expect("create chunk executor");
        let provider = provider_with_generator(Arc::new(CountingGenerator::new()));
        let world = MinecraftWorldId::random();
        let key = ChunkKey::new(0, ChunkPosition::new(1, 2));
        let rx = executor
            .try_submit(world, provider, key, -64, 319)
            .expect("admit chunk request");
        let outcome = futures_block_on(rx).expect("recv");
        assert!(
            matches!(outcome.as_ref(), Ok(Some(_))),
            "generator produced a chunk"
        );
        executor.shutdown();
    }

    #[test]
    fn concurrent_requests_for_same_key_coalesce_to_one_generation() {
        let executor = WorldChunkExecutor::new(4).expect("create chunk executor");
        let generator = Arc::new(CountingGenerator::new());
        let provider = provider_with_generator(generator.clone());
        let world = MinecraftWorldId::random();
        let key = ChunkKey::new(0, ChunkPosition::new(3, 4));
        // Submit concurrent requests for the same key.
        let mut receivers = Vec::new();
        for _ in 0..8 {
            receivers.push(
                executor
                    .try_submit(world.clone(), provider.clone(), key, -64, 319)
                    .expect("admit coalesced request"),
            );
        }
        for rx in receivers {
            let outcome = futures_block_on(rx);
            assert!(outcome.is_ok(), "waiter received a result");
        }
        // After dedup, the same key must generate exactly once.
        assert_eq!(generator.count(), 1, "same-key requests must coalesce");
        executor.shutdown();
    }

    /// Storage that fails the first load and succeeds the second (verifies retry-after-failure).
    struct FailOnceStorage {
        failed: std::sync::atomic::AtomicBool,
    }
    impl WorldStorage for FailOnceStorage {
        fn load_chunk(&self, key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
            if !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return Err(WorldStorageError::Backend(
                    "transient leveldb failure".into(),
                ));
            }
            Ok(Some(Chunk::empty(key.position, key.dimension, -64, 319)))
        }
    }

    #[test]
    fn failure_backoff_caps_delay_and_entry_count() {
        let mut backoff = FailureBackoff::default();
        let world = MinecraftWorldId::random();
        let now = Instant::now();
        let first = ExecutorKey {
            world: world.clone(),
            chunk: ChunkKey::new(0, ChunkPosition::new(0, 0)),
        };
        for _ in 0..32 {
            backoff.record_failure(first.clone(), now);
        }
        let record = backoff.records.get(&first).expect("failure record");
        assert!(record.retry_at.duration_since(now) <= MAX_FAILURE_BACKOFF);
        assert!(record.attempts > 1);

        let mut oldest = None;
        for index in 1..=MAX_FAILURE_BACKOFF_KEYS {
            let key = ExecutorKey {
                world: world.clone(),
                chunk: ChunkKey::new(0, ChunkPosition::new(index as i32, 0)),
            };
            if index == 1 {
                oldest = Some(key.clone());
            }
            backoff.record_failure(key, now);
        }
        assert_eq!(backoff.records.len(), MAX_FAILURE_BACKOFF_KEYS);
        assert!(!backoff.records.contains_key(&first));
        assert!(backoff.records.contains_key(&oldest.expect("oldest key")));
    }

    #[test]
    fn load_failure_propagates_and_same_key_can_retry() {
        let executor = WorldChunkExecutor::new(2).expect("create chunk executor");
        let storage = Arc::new(FailOnceStorage {
            failed: std::sync::atomic::AtomicBool::new(false),
        });
        let provider = WorldChunkProvider::new(storage);
        let world = MinecraftWorldId::random();
        let key = ChunkKey::new(0, ChunkPosition::new(7, 8));
        // First attempt: storage failure propagates to the caller.
        let outcome = futures_block_on(
            executor
                .try_submit(world.clone(), provider.clone(), key, -64, 319)
                .expect("admit first attempt"),
        )
        .expect("recv");
        assert!(
            outcome.is_err(),
            "first load must surface the storage error"
        );
        // The request entry is cleared, but the failed key gets a short bounded
        // cooldown so an invalid/transient chunk cannot spin every tick.
        assert!(matches!(
            executor.try_submit(world.clone(), provider.clone(), key, -64, 319),
            Err(ChunkSubmitError::Busy)
        ));
        std::thread::sleep(INITIAL_FAILURE_BACKOFF + Duration::from_millis(10));
        let outcome = futures_block_on(
            executor
                .try_submit(world.clone(), provider.clone(), key, -64, 319)
                .expect("admit retry after cooldown"),
        )
        .expect("recv");
        assert!(
            matches!(outcome.as_ref(), Ok(Some(_))),
            "retry after cooldown must succeed"
        );
        let outcome = futures_block_on(
            executor
                .try_submit(world, provider, key, -64, 319)
                .expect("successful retry clears failure cooldown"),
        )
        .expect("recv cached chunk");
        assert!(matches!(outcome.as_ref(), Ok(Some(_))));
        executor.shutdown();
    }

    #[test]
    fn distinct_keys_each_generate_once() {
        let executor = WorldChunkExecutor::new(2).expect("create chunk executor");
        let generator = Arc::new(CountingGenerator::new());
        let provider = provider_with_generator(generator.clone());
        let world = MinecraftWorldId::random();
        let mut receivers = Vec::new();
        for i in 0..4 {
            let key = ChunkKey::new(0, ChunkPosition::new(i, i));
            receivers.push(
                executor
                    .try_submit(world.clone(), provider.clone(), key, -64, 319)
                    .expect("admit distinct key"),
            );
        }
        for rx in receivers {
            assert!(futures_block_on(rx).is_ok());
        }
        assert_eq!(generator.count(), 4, "each distinct key generates once");
        executor.shutdown();
    }

    fn futures_block_on_with_timeout<F: std::future::Future>(
        future: F,
        timeout: std::time::Duration,
    ) -> Result<F::Output, tokio::time::error::Elapsed> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async { tokio::time::timeout(timeout, future).await })
    }

    /// Small blocking wait (test-only): await a oneshot on a temporary runtime.
    fn futures_block_on(
        rx: oneshot::Receiver<Arc<ChunkLoadOutcome>>,
    ) -> Result<Arc<ChunkLoadOutcome>, oneshot::error::RecvError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(rx)
    }
}
