//! Bounded, per-content-version LevelChunk encoding workers.
//!
//! Encodes are coalesced by world/chunk generation and wire profile. Consumer
//! tickets are demand leases: a queued job with no consumers is skipped, and a
//! completed result remains discoverable only while a caller still owns it.

use std::any::Any;
use std::collections::HashMap;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use parking_lot::Mutex;
use sc_ecs::resource::Resource;
use sc_world::chunk_view::{CachedChunk, ChunkCacheKey};
use sc_world::storage::ChunkColumn;
use tokio::sync::watch;

use crate::protocol::server::chunk::LevelChunk;

const JOB_QUEUE_CAPACITY: usize = 128;
const MAX_WAITERS_PER_KEY: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChunkEncodeError {
    StaleGeneration,
    InvalidBlockMapping(String),
    WorkerPanicked(String),
}

impl std::fmt::Display for ChunkEncodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleGeneration => formatter.write_str("chunk changed during encoding"),
            Self::InvalidBlockMapping(message) => formatter.write_str(message),
            Self::WorkerPanicked(message) => {
                write!(formatter, "encoding worker panicked: {message}")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkEncodeSubmitError {
    Busy,
    Closed,
    TooManyWaiters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkEncodeWaitError {
    Closed,
}

pub type ChunkEncodeResult = Result<Arc<CachedChunk>, ChunkEncodeError>;
type SharedChunkEncodeResult = Arc<ChunkEncodeResult>;

#[derive(Clone, Hash, Eq, PartialEq)]
struct EncodeKey {
    cache_key: ChunkCacheKey,
    generation: u64,
    incarnation: u128,
    wire_profile: u32,
}

struct EncodeEntry {
    waiters: AtomicUsize,
    result: watch::Sender<Option<SharedChunkEncodeResult>>,
}

struct EncodeJob {
    key: EncodeKey,
    column: Arc<ChunkColumn>,
    entry: Arc<EncodeEntry>,
}

type InflightMap = HashMap<EncodeKey, std::sync::Weak<EncodeEntry>>;

struct ExecutorInner {
    jobs: Mutex<Option<mpsc::SyncSender<EncodeJob>>>,
    inflight: Arc<Mutex<InflightMap>>,
    workers: Mutex<Option<Vec<thread::JoinHandle<()>>>>,
}

/// A lease on one shared encode result. Keep the ticket alive until the caller
/// publishes or rejects the result so a concurrent consumer can join the
/// completed operation instead of starting a duplicate encode.
pub struct ChunkEncodeTicket {
    entry: Arc<EncodeEntry>,
    receiver: watch::Receiver<Option<SharedChunkEncodeResult>>,
}

impl ChunkEncodeTicket {
    pub async fn wait(&mut self) -> Result<SharedChunkEncodeResult, ChunkEncodeWaitError> {
        loop {
            let result = { self.receiver.borrow_and_update().clone() };
            if let Some(result) = result {
                return Ok(result);
            }
            self.receiver
                .changed()
                .await
                .map_err(|_| ChunkEncodeWaitError::Closed)?;
        }
    }
}

impl Drop for ChunkEncodeTicket {
    fn drop(&mut self) {
        self.entry.waiters.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Network-owned worker pool; workers receive only immutable column handles and
/// never access ECS resources, player connections, or packet hooks.
#[derive(Resource, Clone)]
pub struct ChunkEncodeExecutor {
    inner: Arc<ExecutorInner>,
    wire_profile: u32,
}

impl ChunkEncodeExecutor {
    pub fn new(worker_count: usize, wire_profile: u32) -> io::Result<Self> {
        let worker_count = worker_count.clamp(1, 4);
        let (tx, rx) = mpsc::sync_channel::<EncodeJob>(JOB_QUEUE_CAPACITY);
        let receiver = Arc::new(Mutex::new(rx));
        let inflight = Arc::new(Mutex::new(HashMap::new()));
        let mut workers: Vec<thread::JoinHandle<()>> = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let receiver = receiver.clone();
            let inflight = inflight.clone();
            let worker = match thread::Builder::new()
                .name(format!("sc-chunk-encoder-{index}"))
                .spawn(move || worker_loop(receiver, inflight))
            {
                Ok(worker) => worker,
                Err(error) => {
                    drop(tx);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(error);
                }
            };
            workers.push(worker);
        }
        Ok(Self {
            inner: Arc::new(ExecutorInner {
                jobs: Mutex::new(Some(tx)),
                inflight,
                workers: Mutex::new(Some(workers)),
            }),
            wire_profile,
        })
    }

    pub fn default_worker_count() -> usize {
        let parallelism = thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(4);
        (parallelism / 2).clamp(1, 4)
    }

    pub fn try_submit(
        &self,
        cache_key: ChunkCacheKey,
        column: Arc<ChunkColumn>,
    ) -> Result<ChunkEncodeTicket, ChunkEncodeSubmitError> {
        let key = EncodeKey {
            cache_key,
            generation: column.generation(),
            incarnation: column.incarnation(),
            wire_profile: self.wire_profile,
        };
        let mut inflight = self.inner.inflight.lock();
        inflight.retain(|_, entry| entry.strong_count() > 0);

        if let Some(entry) = inflight.get(&key).and_then(std::sync::Weak::upgrade) {
            let current = entry.waiters.fetch_add(1, Ordering::AcqRel);
            if current >= MAX_WAITERS_PER_KEY {
                entry.waiters.fetch_sub(1, Ordering::AcqRel);
                return Err(ChunkEncodeSubmitError::TooManyWaiters);
            }
            let receiver = entry.result.subscribe();
            return Ok(ChunkEncodeTicket { entry, receiver });
        }

        let (result, receiver) = watch::channel(None);
        let entry = Arc::new(EncodeEntry {
            waiters: AtomicUsize::new(1),
            result,
        });
        inflight.insert(key.clone(), Arc::downgrade(&entry));
        let job = EncodeJob {
            key: key.clone(),
            column,
            entry: entry.clone(),
        };
        let admission = self
            .inner
            .jobs
            .lock()
            .as_ref()
            .map(|sender| sender.try_send(job));
        match admission {
            Some(Ok(())) => Ok(ChunkEncodeTicket { entry, receiver }),
            Some(Err(mpsc::TrySendError::Full(_))) => {
                inflight.remove(&key);
                entry.waiters.store(0, Ordering::Release);
                Err(ChunkEncodeSubmitError::Busy)
            }
            Some(Err(mpsc::TrySendError::Disconnected(_))) | None => {
                inflight.remove(&key);
                entry.waiters.store(0, Ordering::Release);
                Err(ChunkEncodeSubmitError::Closed)
            }
        }
    }

    /// Close admission, drain accepted encodes, and join workers. Idempotent.
    pub fn shutdown(&self) {
        let _ = self.inner.jobs.lock().take();
        if let Some(workers) = self.inner.workers.lock().take() {
            for worker in workers {
                let _ = worker.join();
            }
        }
    }
}

fn worker_loop(receiver: Arc<Mutex<mpsc::Receiver<EncodeJob>>>, inflight: Arc<Mutex<InflightMap>>) {
    loop {
        let job = {
            let guard = receiver.lock();
            guard.recv()
        };
        let Ok(job) = job else {
            return;
        };

        // Serialize this check with submission: if every ticket was dropped
        // while queued, skip the encode; a racing new consumer either joins this
        // job or observes it removed and safely creates a replacement.
        let has_waiters = {
            let mut inflight = inflight.lock();
            let live = inflight
                .get(&job.key)
                .and_then(std::sync::Weak::upgrade)
                .is_some_and(|entry| {
                    Arc::ptr_eq(&entry, &job.entry) && entry.waiters.load(Ordering::Acquire) > 0
                });
            if !live {
                inflight.remove(&job.key);
            }
            live
        };
        if !has_waiters {
            continue;
        }

        let result = match catch_unwind(AssertUnwindSafe(|| encode(&job))) {
            Ok(result) => result,
            Err(payload) => Err(ChunkEncodeError::WorkerPanicked(panic_message(
                payload.as_ref(),
            ))),
        };
        job.entry.result.send_replace(Some(Arc::new(result)));
    }
}

fn encode(job: &EncodeJob) -> Result<Arc<CachedChunk>, ChunkEncodeError> {
    let chunk = job.column.read();
    let generation = job.column.generation();
    if generation != job.key.generation || job.column.incarnation() != job.key.incarnation {
        return Err(ChunkEncodeError::StaleGeneration);
    }
    let packet = LevelChunk::from_chunk(&chunk)
        .map_err(|error| ChunkEncodeError::InvalidBlockMapping(error.to_string()))?;
    if job.column.generation() != generation {
        return Err(ChunkEncodeError::StaleGeneration);
    }
    Ok(Arc::new(CachedChunk {
        payload: Arc::new(packet.payload),
        subchunk_count: packet.sub_chunk_count,
        generation,
        incarnation: job.key.incarnation,
        wire_profile: job.key.wire_profile,
    }))
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
    use sc_world::chunk::{Chunk, ChunkPosition};
    use sc_world::manager::MinecraftWorldId;
    use sc_world::storage::ChunkKey;
    use std::future::Future;

    fn block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn shared_key_consumers_receive_the_same_completed_encode() {
        let executor = ChunkEncodeExecutor::new(1, 2168).expect("create encoder");
        let world_id = MinecraftWorldId::random();
        let cache_key = ChunkCacheKey::new(world_id, ChunkKey::new(0, ChunkPosition::new(2, -3)));
        let column = Arc::new(ChunkColumn::new(Chunk::empty_overworld(
            cache_key.chunk.position,
        )));

        let mut first = executor
            .try_submit(cache_key.clone(), column.clone())
            .expect("submit first");
        let first_result = block_on(first.wait()).expect("first result channel");
        let first_payload = first_result
            .as_ref()
            .as_ref()
            .expect("encoding succeeded")
            .clone();
        let mut second = executor
            .try_submit(cache_key.clone(), column)
            .expect("join completed key");
        let second_result = block_on(second.wait()).expect("second result channel");
        let second_payload = second_result
            .as_ref()
            .as_ref()
            .expect("encoding succeeded")
            .clone();

        assert!(Arc::ptr_eq(&first_payload, &second_payload));
        let reloaded = Arc::new(ChunkColumn::new(Chunk::empty_overworld(
            cache_key.chunk.position,
        )));
        let mut replacement = executor.try_submit(cache_key, reloaded.clone()).unwrap();
        let replacement_result = block_on(replacement.wait()).unwrap();
        let replacement_payload = replacement_result.as_ref().as_ref().unwrap();
        assert!(!Arc::ptr_eq(&first_payload, replacement_payload));
        assert!(replacement_payload.matches_column(&reloaded, 2168));
        executor.shutdown();
    }

    #[test]
    fn encoder_coalesced_waiters_have_a_hard_limit() {
        let executor = ChunkEncodeExecutor::new(1, 2168).expect("create encoder");
        let key = ChunkCacheKey::new(
            MinecraftWorldId::random(),
            ChunkKey::new(0, ChunkPosition::new(0, 0)),
        );
        let column = Arc::new(ChunkColumn::new(Chunk::empty_overworld(key.chunk.position)));
        let mut first = executor
            .try_submit(key.clone(), column.clone())
            .expect("submit first");
        block_on(first.wait()).expect("first result");
        let mut tickets = vec![first];
        for _ in 1..MAX_WAITERS_PER_KEY {
            tickets.push(
                executor
                    .try_submit(key.clone(), column.clone())
                    .expect("coalesce waiter"),
            );
        }
        assert!(matches!(
            executor.try_submit(key, column),
            Err(ChunkEncodeSubmitError::TooManyWaiters),
        ));
        drop(tickets);
        executor.shutdown();
    }

    #[test]
    fn stale_generation_is_not_encoded_as_current_payload() {
        let executor = ChunkEncodeExecutor::new(1, 2168).expect("create encoder");
        let world_id = MinecraftWorldId::random();
        let cache_key = ChunkCacheKey::new(world_id, ChunkKey::new(0, ChunkPosition::new(2, -3)));
        let column = Arc::new(ChunkColumn::new(Chunk::empty_overworld(
            cache_key.chunk.position,
        )));
        let key = EncodeKey {
            cache_key,
            generation: column.generation().wrapping_sub(1),
            incarnation: column.incarnation(),
            wire_profile: executor.wire_profile,
        };
        let (result, _) = watch::channel(None);
        let job = EncodeJob {
            key,
            column: column.clone(),
            entry: Arc::new(EncodeEntry {
                waiters: AtomicUsize::new(1),
                result,
            }),
        };

        assert!(matches!(
            encode(&job),
            Err(ChunkEncodeError::StaleGeneration)
        ));
        executor.shutdown();
    }
}
