//! 有界异步脏列写回。
//!
//! 主世界持有者准入脏快照并轮询终止收据。worker 线程只调用既有存储
//! 接口；它们从不清除脏状态或改动 `ChunkColumn`。
//!
//! Bounded asynchronous dirty-column writeback.
//!
//! The root-world owner admits dirty snapshots and polls terminal receipts.
//! Worker threads only call the existing storage interface; they never clear
//! dirty state or mutate a `ChunkColumn`.

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

/// 默认最大在途任务数。
///
/// Default maximum number of in-flight jobs.
const DEFAULT_MAX_JOBS: usize = 128;
/// 默认最大在途字节数（64 MiB）。
///
/// Default maximum in-flight bytes (64 MiB).
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;
/// 每世界每 tick 扫描的脏列候选上限。
///
/// Maximum dirty-column candidates scanned per world per tick.
const MAX_DIRTY_CANDIDATES_PER_WORLD_TICK: usize = 64;
/// 失败退避表的最大键数（FIFO 淘汰上限）。
///
/// Maximum number of keys in the failure-backoff table (FIFO cap).
const MAX_FAILURE_KEYS: usize = 4096;
/// 失败重试的初始延迟。
///
/// Initial failure-retry delay.
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);
/// 失败重试的最大延迟。
///
/// Maximum failure-retry delay.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

/// 写回键：世界 id + 区块坐标。
///
/// Writeback key: world id plus chunk coordinates.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct WritebackKey {
    /// 所属世界 id。
    ///
    /// Owning world id.
    world_id: MinecraftWorldId,
    /// 区块坐标键。
    ///
    /// Chunk coordinate key.
    chunk: ChunkKey,
}

/// 一个写回任务：脏快照 + 各脏位 + spillover 确认 + 存储句柄。
///
/// One writeback job: the dirty snapshot, its dirty flags, spillover acks and
/// the storage handle.
struct WritebackJob {
    /// 写回键。
    ///
    /// Writeback key.
    key: WritebackKey,
    /// 尝试号（用于识别过期 ack）。
    ///
    /// Attempt id (identifies stale acks).
    attempt_id: u64,
    /// 源列句柄（ack 回到持有者手里）。
    ///
    /// Source column handle (returned to the owner with the ack).
    column: Arc<ChunkColumn>,
    /// 快照对应的内容代际。
    ///
    /// Content generation the snapshot corresponds to.
    generation: u64,
    /// 方块实体是否脏。
    ///
    /// Whether block entities are dirty.
    block_entities_dirty: bool,
    /// 生物群系是否脏。
    ///
    /// Whether biomes are dirty.
    biomes_dirty: bool,
    /// 高度图是否脏。
    ///
    /// Whether the heightmap is dirty.
    heightmap_dirty: bool,
    /// 快照携带的 spillover 操作确认列表。
    ///
    /// Spillover operation acks carried by the snapshot.
    spillover_acks: Vec<SpilloverOperationId>,
    /// 存储后端句柄。
    ///
    /// Storage backend handle.
    storage: Arc<dyn WorldStorage>,
    /// 克隆出的区块快照（worker 独占写）。
    ///
    /// Cloned chunk snapshot (owned by the worker).
    snapshot: Chunk,
}

/// 一次写回完成的确认回执。
///
/// The acknowledgement receipt of one completed writeback.
struct WritebackAck {
    /// 写回键。
    ///
    /// Writeback key.
    key: WritebackKey,
    /// 尝试号。
    ///
    /// Attempt id.
    attempt_id: u64,
    /// 源列句柄。
    ///
    /// Source column handle.
    column: Arc<ChunkColumn>,
    /// 快照内容代际。
    ///
    /// Snapshot content generation.
    generation: u64,
    /// spillover 操作确认列表。
    ///
    /// Spillover operation acks.
    spillover_acks: Vec<SpilloverOperationId>,
    /// 写结果（`Err` 为错误信息）。
    ///
    /// Write result (`Err` carries the error message).
    result: Result<(), String>,
}

/// 在途写回记录：预留字节、受理时刻与所属 provider。
///
/// In-flight writeback record: reserved bytes, acceptance time and owning
/// provider.
struct InFlightWriteback {
    /// 尝试号。
    ///
    /// Attempt id.
    attempt_id: u64,
    /// 预留的字节数。
    ///
    /// Reserved bytes.
    reserved_bytes: usize,
    /// 受理时刻。
    ///
    /// Acceptance time.
    accepted_at: Instant,
    /// 超龄警告是否已发过（每次尝试只发一次）。
    ///
    /// Whether the overdue warning was already emitted (once per attempt).
    age_warning_emitted: bool,
    /// 所属 provider，确认写回后可释放其脏数据预算。
    ///
    /// Owning provider, so a confirmed write can release its dirty budget.
    provider: WorldChunkProvider,
    /// 已向 provider 计费的字节数。
    ///
    /// Bytes charged to the provider.
    dirty_bytes_charged: usize,
}

/// 写回超龄报告（观测用）。
///
/// Writeback overdue report (for observability).
#[derive(Default)]
pub(crate) struct WritebackAgeReport {
    /// 本次新发现的超龄任务数。
    ///
    /// Number of newly overdue jobs.
    pub newly_overdue: usize,
    /// 超龄任务总数。
    ///
    /// Total overdue jobs.
    pub overdue: usize,
    /// 最老任务的时长。
    ///
    /// Age of the oldest job.
    pub oldest_age: Option<Duration>,
    /// 采样键列表（有界）。
    ///
    /// Sampled keys (bounded).
    pub key_samples: Vec<(MinecraftWorldId, ChunkKey)>,
}

/// 单条失败记录：下次可重试时刻与尝试次数。
///
/// One failure record: the next retry time and the attempt count.
#[derive(Clone, Copy)]
struct FailureRecord {
    /// 下次允许重试的时刻。
    ///
    /// Instant when a retry is allowed again.
    retry_at: Instant,
    /// 连续失败次数。
    ///
    /// Consecutive failure count.
    attempts: u8,
}

/// 写回执行器的可变状态。
///
/// Mutable state of the writeback executor.
#[derive(Default)]
struct WritebackState {
    /// 在途写回表。
    ///
    /// In-flight writeback table.
    in_flight: HashMap<WritebackKey, InFlightWriteback>,
    /// 在途字节数。
    ///
    /// In-flight bytes.
    in_flight_bytes: usize,
    /// 下一个尝试号。
    ///
    /// Next attempt id.
    next_attempt_id: u64,
    /// 失败退避表。
    ///
    /// Failure-backoff table.
    retry: HashMap<WritebackKey, FailureRecord>,
    /// 失败退避的插入顺序（FIFO 淘汰用）。
    ///
    /// Failure-backoff insertion order (for FIFO eviction).
    retry_order: VecDeque<WritebackKey>,
    /// 每世界的脏列扫描游标。
    ///
    /// Per-world dirty-column scan cursor.
    dirty_cursor: HashMap<MinecraftWorldId, ChunkKey>,
    /// 轮转游标：下一个被服务的世界。
    ///
    /// Rotation cursor: the next world to be served.
    world_cursor: Option<MinecraftWorldId>,
}

impl WritebackState {
    /// 生成超龄报告：超过 `threshold` 的任务计数、最老时长与
    /// 新超龄任务的键采样。
    ///
    /// Builds the overdue report: counts jobs older than `threshold`, the
    /// oldest age and a sample of newly overdue keys.
    ///
    /// - `now`: 当前时刻
    /// - `threshold`: 超龄阈值
    /// - `sample_limit`: 键采样上限
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

    /// 该键当前是否处于重试退避期（未到 `retry_at`）。
    ///
    /// Whether the key is currently in retry backoff (before its `retry_at`).
    ///
    /// - `key`: 写回键
    /// - `now`: 当前时刻
    fn retry_blocked(&self, key: &WritebackKey, now: Instant) -> bool {
        self.retry
            .get(key)
            .is_some_and(|record| record.retry_at > now)
    }

    /// 记录一次失败：指数拉长退避（上限 [`MAX_RETRY_DELAY`]），
    /// 超容时按 FIFO 淘汰最旧键。
    ///
    /// Records a failure: grows the backoff exponentially (capped at
    /// [`MAX_RETRY_DELAY`]) and evicts the oldest key when full.
    ///
    /// - `key`: 写回键
    /// - `now`: 失败发生时刻
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

    /// 清除某键的失败记录（成功后的清理）。
    ///
    /// Clears a key's failure record (cleanup after success).
    ///
    /// - `key`: 写回键
    fn clear_failure(&mut self, key: &WritebackKey) {
        if self.retry.remove(key).is_some() {
            self.retry_order.retain(|entry| entry != key);
        }
    }
}

/// 执行器内部状态（worker 与持有者共享）。
///
/// Executor internal state (shared by workers and the owner).
struct WritebackInner {
    /// 任务发送端（`shutdown` 时取出以断流）。
    ///
    /// Job sender (taken at `shutdown` to close the stream).
    jobs: Mutex<Option<mpsc::SyncSender<WritebackJob>>>,
    /// 确认回执接收端。
    ///
    /// Acknowledgement receiver.
    acknowledgements: Mutex<mpsc::Receiver<WritebackAck>>,
    /// 可变状态。
    ///
    /// Mutable state.
    state: Mutex<WritebackState>,
    /// worker 线程句柄（`shutdown` 时取出并 join）。
    ///
    /// Worker thread handles (taken and joined at `shutdown`).
    workers: Mutex<Option<Vec<thread::JoinHandle<()>>>>,
    /// 最大在途任务数。
    ///
    /// Maximum in-flight jobs.
    max_jobs: usize,
    /// 最大在途字节数。
    ///
    /// Maximum in-flight bytes.
    max_bytes: usize,
}

/// 脏快照调度的非阻塞准入结果。
///
/// Non-blocking admission outcomes for dirty snapshot scheduling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WritebackSubmit {
    /// 已入队。
    ///
    /// Queued.
    Queued,
    /// 同键任务已在途。
    ///
    /// A job for the same key is already in flight.
    AlreadyInFlight,
    /// 列不脏，无需写回。
    ///
    /// The column is clean; no writeback needed.
    Clean,
    /// 队列/字节预算已满，稍后重试。
    ///
    /// The job/byte budget is full; retry later.
    Busy,
    /// 快照超过总字节预算。
    ///
    /// The snapshot exceeds the total byte budget.
    Oversize,
    /// 处于失败退避期。
    ///
    /// In failure backoff.
    Backoff,
    /// 执行器已关闭。
    ///
    /// The executor is closed.
    Closed,
}

/// 一次 `poll_completed` 的统计报告。
///
/// Statistics report of one `poll_completed` call.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WritebackPollReport {
    /// 处理完成的 ack 数。
    ///
    /// Number of processed acks.
    pub completed: usize,
    /// 确认保存数。
    ///
    /// Number of confirmed saves.
    pub saved: usize,
    /// 失败数。
    ///
    /// Number of failures.
    pub failed: usize,
    /// 被更新代际取代的数。
    ///
    /// Number superseded by a newer generation.
    pub superseded: usize,
}

/// 存储写回的有界共享 worker 池。已接受的计数/字节许可一直保留到
/// 所属 tick 消费对应完成事件为止。
///
/// Bounded shared worker pool for storage writeback. Accepted count/byte
/// permits remain reserved until the owning tick consumes each completion.
#[derive(Resource, Clone)]
pub struct ChunkWritebackExecutor {
    inner: Arc<WritebackInner>,
}

impl ChunkWritebackExecutor {
    /// 默认 worker 数：并行度的 1/4，钳制在 1..=2。
    ///
    /// Default worker count: a quarter of the parallelism, clamped to 1..=2.
    pub fn default_worker_count() -> usize {
        let parallelism = thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(2);
        (parallelism / 4).clamp(1, 2)
    }

    /// 以 worker 数与默认预算构造执行器。
    ///
    /// Builds an executor with the given worker count and default budgets.
    ///
    /// - `worker_count`: worker 线程数（钳制在 1..=4）
    pub fn new(worker_count: usize) -> io::Result<Self> {
        Self::with_limits(worker_count, DEFAULT_MAX_JOBS, DEFAULT_MAX_BYTES)
    }

    /// 以显式 worker 数与预算构造执行器。
    ///
    /// Builds an executor with explicit worker count and budgets.
    ///
    /// - `worker_count`: worker 线程数（钳制在 1..=4）
    /// - `max_jobs`: 最大在途任务数
    /// - `max_bytes`: 最大在途字节数
    fn with_limits(worker_count: usize, max_jobs: usize, max_bytes: usize) -> io::Result<Self> {
        let max_jobs = max_jobs.max(1);
        let max_bytes = max_bytes.max(1);
        let worker_count = worker_count.clamp(1, 4);
        let (job_tx, job_rx) = mpsc::sync_channel::<WritebackJob>(max_jobs);
        let job_rx = Arc::new(Mutex::new(job_rx));
        // Completion-slot reservation invariant: the acknowledgement
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

    /// 尝试准入一个脏列快照（非阻塞）。
    ///
    /// Tries to admit a dirty-column snapshot (non-blocking).
    ///
    /// 列不脏返回 [`WritebackSubmit::Clean`]；同键已在途返回
    /// [`WritebackSubmit::AlreadyInFlight`]；退避期返回
    /// [`WritebackSubmit::Backoff`]；超预算返回 [`WritebackSubmit::Busy`]
    /// 或 [`WritebackSubmit::Oversize`]；快照阶段出问题会撤销预留。
    ///
    /// Returns [`WritebackSubmit::Clean`] for a clean column,
    /// [`WritebackSubmit::AlreadyInFlight`] when the same key is in flight,
    /// [`WritebackSubmit::Backoff`] during backoff, and
    /// [`WritebackSubmit::Busy`] or [`WritebackSubmit::Oversize`] when over
    /// budget; snapshot-phase failures cancel the reservation.
    ///
    /// - `world_id`: 所属世界 id
    /// - `provider`: 区块提供器
    /// - `key`: 区块坐标键
    /// - `column`: 区块列句柄
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
        // Dirty budget: charge the snapshot once it is accepted for
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

    /// 撤销一次准入预留：摘除在途记录并释放字节预算。
    ///
    /// Cancels an admission reservation: removes the in-flight record and
    /// releases the byte budget.
    ///
    /// - `key`: 写回键
    /// - `attempt_id`: 尝试号（不匹配说明已被更新的尝试取代）
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

    /// 从一个世界的脏列中提交一个有界循环窗口。
    ///
    /// Submit a bounded cyclic window from one world's dirty columns.
    ///
    /// 扫描游标记录上次检查位置，避免每 tick 重复扫同一批脏列。
    ///
    /// The scan cursor remembers the last examined position so each tick does
    /// not rescan the same dirty columns.
    ///
    /// - `world_id`: 目标世界 id
    /// - `provider`: 区块提供器
    /// - `max_submits`: 本次最多提交数
    ///
    /// 返回实际提交数。
    ///
    /// Returns the number actually submitted.
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

    /// 轮转首个被服务的世界，使一个大世界的脏集合不会在其它世界
    /// 被访问前耗尽整 tick 的准入额度。
    ///
    /// Rotate the first-serviced world so a large world's dirty set cannot
    /// consume every per-tick admission before other worlds are visited.
    ///
    /// - `worlds`: `(世界 id, provider)` 列表
    /// - `max_submits`: 本次最多提交数
    ///
    /// 返回实际提交总数。
    ///
    /// Returns the total number actually submitted.
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

    /// 轮询至多 `max_acks` 个完成回执：释放预留、清除脏位并统计结果。
    ///
    /// Polls up to `max_acks` completion receipts: releases reservations,
    /// clears dirty flags and tallies the outcome.
    ///
    /// ack 的尝试号与在途记录不匹配（被取代）时跳过；保存成功后按
    /// 代际/spillover 确认清除脏位，被新代际取代或失败都记退避。
    ///
    /// Acks whose attempt id no longer matches the in-flight record (they
    /// were superseded) are skipped; a confirmed save clears the dirty flags
    /// by generation/spillover, while superseded or failed writes record
    /// backoff.
    ///
    /// - `max_acks`: 本次最多处理的 ack 数
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

    /// 在途任务数。
    ///
    /// Number of in-flight jobs.
    pub fn in_flight_jobs(&self) -> usize {
        self.inner.state.lock().in_flight.len()
    }

    /// 在途字节数。
    ///
    /// In-flight bytes.
    pub fn in_flight_bytes(&self) -> usize {
        self.inner.state.lock().in_flight_bytes
    }

    /// 观测超龄的已接受写回，不释放其许可，也不尝试取消可能正在
    /// 改库的存储调用。
    ///
    /// Observe old accepted writebacks without releasing their permits or
    /// attempting to cancel storage calls that may already be mutating the DB.
    ///
    /// - `threshold`: 超龄阈值
    /// - `sample_limit`: 键采样上限
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

    /// 停止准入、经 DB 持有者排空已接受的写入并 join。
    ///
    /// Stop admission, drain accepted writes through the DB owner, and join.
    pub fn shutdown(&self) -> WritebackShutdownReport {
        self.shutdown_with_timeout(Self::DEFAULT_SHUTDOWN_TIMEOUT)
    }

    /// 默认有界关闭等待时长（30s）。
    ///
    /// Default bounded shutdown wait (30s).
    ///
    /// 永久阻塞的存储后端（盘挂死、网络挂起）不能让关闭无限等待且
    /// 一声不吭。预算到期后报告明确指出哪些写未确认；worker 线程
    /// 在可能仍在写库时被有意**不**强杀。
    ///
    /// A storage backend that blocks forever (hung disk, stalled network mount)
    /// must not make shutdown wait forever while reporting nothing. After the
    /// budget expires the report states exactly which writes are unconfirmed;
    /// the worker threads are deliberately **not** killed while they may still be
    /// mutating the database.
    pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

    /// 带显式等待预算的关闭。
    ///
    /// Shutdown with an explicit wait budget.
    ///
    /// 「ack 槽位数 == 最大可接受任务数」的不变式使排空顺序成立：
    /// 许可只在其完成事件被轮询时释放，因此 worker 总能把手里的
    /// 每次已接受写回交还给持有者。
    ///
    /// The invariant "ack slot count == maximum accepted jobs" keeps the drain
    /// order valid: a permit is only released when its completion is polled, so
    /// the workers can always hand every accepted write back to the owner.
    ///
    /// - `timeout`: 等待预算
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

/// 有界关闭等待能确认什么。
///
/// What the bounded shutdown wait could confirm.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WritebackShutdownReport {
    /// 已完成并被 join 的 worker 数。
    ///
    /// Workers that finished and were joined.
    pub joined_workers: usize,
    /// 等待预算到期时仍有 worker 在运行。
    ///
    /// The wait budget expired while a worker was still running.
    pub timed_out: bool,
    /// SaveAck 从未被观测到的已接受写入数。
    ///
    /// Accepted writes whose SaveAck was never observed.
    pub unconfirmed_jobs: usize,
    /// 这些写入占用的估计字节数。
    ///
    /// Estimated bytes retained by those writes.
    pub unconfirmed_bytes: usize,
}

impl WritebackShutdownReport {
    /// 是否每次已接受写入都到达了确认的终止状态。
    ///
    /// Whether every accepted write reached a confirmed terminal state.
    pub fn is_complete(&self) -> bool {
        !self.timed_out && self.unconfirmed_jobs == 0
    }
}

/// 写回 worker 主循环：取任务、写存储、发回执。
///
/// Writeback worker main loop: take a job, write storage, send the receipt.
///
/// 存储 panic 被隔离为一条错误回执，worker 继续处理后续任务。
///
/// A storage panic is isolated into an error receipt and the worker keeps
/// processing later jobs.
///
/// - `receiver`: 任务接收端（worker 间共享）
/// - `acknowledgements`: 回执发送端
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
                sc_async::panic_message(payload.as_ref())
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
                    provider: WorldChunkProvider::new_testing(Arc::new(InMemoryWorldStorage::default())),
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
            Err(WorldStorageError::backend(
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

    /// 永不返回的存储后端，模拟永久挂死的磁盘。
    ///
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

    /// 记录被要求执行多少次保存的存储后端。
    ///
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

    /// 完成槽位在准入时即预留：受理最大数量的作业绝不能让任何 worker
    /// 阻塞在确认通道上，因此"先 join、再 drain"的关机顺序始终成立。
    ///
    /// Completion slots are reserved at admission: accepting the maximum number
    /// of jobs must not block any worker on the acknowledgement channel, so the
    /// shutdown order "join, then drain" stays valid.
    #[test]
    fn confirmed_writes_release_the_dirty_budget_and_cancellations_too() {
        let provider = WorldChunkProvider::new_testing(Arc::new(InMemoryWorldStorage::default()));
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
        let provider = WorldChunkProvider::new_testing(Arc::new(CountingStorage {
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
        let provider = WorldChunkProvider::new_testing(Arc::new(HangingStorage {
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
        let provider = WorldChunkProvider::new_testing(Arc::new(InMemoryWorldStorage::default()));
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
        let provider = WorldChunkProvider::new_testing(storage.clone());
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
        let provider = WorldChunkProvider::new_testing(Arc::new(InMemoryWorldStorage::default()));
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
        let provider = WorldChunkProvider::new_testing(storage.clone());
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
            Err(WorldStorageError::backend(
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
        let provider = WorldChunkProvider::new_testing(storage.clone());
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
                .ok_or_else(|| WorldStorageError::backend("snapshot missing test block".into()))?;
            self.started
                .send(value)
                .map_err(|error| WorldStorageError::backend(error.to_string()))?;
            self.release
                .lock()
                .recv()
                .map_err(|error| WorldStorageError::backend(error.to_string()))?;
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
        let provider = WorldChunkProvider::new_testing(storage);
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
        let provider = WorldChunkProvider::new_testing(storage);
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
