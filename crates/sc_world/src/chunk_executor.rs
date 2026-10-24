//! `WorldChunkExecutor`：区块加载/生成的有界 worker 池。
//!
//! 阻塞工作（LevelDB 读与 CPU 世界生成）跑在专用 OS 线程上：
//! 网络任务只 `submit` 任务并 await oneshot 收据，从不在 Tokio 上
//! 调用 `load_chunk`。
//!
//! 同键去重（请求合并）：某个 `(world, chunk)` 加载已排队或运行时，
//! 新请求挂到该任务的等待者列表而不是重新排队；worker 完成时把结果
//! 分发给全部合并等待者。后续请求命中 `WorldChunkProvider` 内部缓存
//! （快路径）。
//!
//! 关闭：`shutdown` 先丢弃任务发送端（worker 排空后退出），再 join
//! 线程；调用方随后写回脏区块。
//!
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
use std::fmt::Display;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Unix 下把 worker 线程设为后台优先级，避免与游戏逻辑抢 CPU。
///
/// On Unix, demotes the worker thread to background priority so it does not
/// compete with game logic for CPU.
#[cfg(unix)]
fn set_thread_background_priority() {
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

/// 非 Unix 平台为空操作。
///
/// No-op on non-Unix platforms.
#[cfg(not(unix))]
fn set_thread_background_priority() {}

use parking_lot::Mutex;
use sc_ecs::resource::Resource;
use tokio::sync::oneshot;

use crate::manager::MinecraftWorldId;
use crate::storage::{ChunkColumn, ChunkKey, WorldChunkProvider, WorldStorageError};
use sc_error::{ScError, ScSite, capture_site};
use sc_log::t_log;

/// 一次区块加载/生成任务的结果。
///
/// Result of one chunk load/generate job.
pub type ChunkLoadOutcome = Result<Option<Arc<ChunkColumn>>, WorldStorageError>;

/// 区块加载/生成请求的同步准入结果。
///
/// Synchronous admission result for a chunk load/generation request.
#[derive(Clone, Debug, Eq, PartialEq, ScError)]
pub enum ChunkSubmitError {
    /// 有界 worker 队列已满或该键处于失败退避；稍后重试。
    ///
    /// Bounded worker queue full or key in failure backoff; retry later.
    #[sc_error(code = "world.executor.busy")]
    Busy {
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 执行器正在关闭或其 worker 通道已断。
    ///
    /// Executor is shutting down or its worker channel is closed.
    #[sc_error(code = "world.executor.closed")]
    Closed {
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 该区块的合并消费者已达上限。
    ///
    /// Chunk already has the maximum number of coalesced consumers.
    #[sc_error(code = "world.executor.too_many_waiters")]
    TooManyWaiters {
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
}

impl ChunkSubmitError {
    /// 构造队列压力拒绝，并捕获调用位置。
    ///
    /// Build a queue-pressure rejection, capturing the call site.
    ///
    /// - `detail`: 说明原因的英文细节
    #[track_caller]
    pub fn busy(detail: String) -> Self {
        Self::Busy {
            detail,
            site: capture_site(),
        }
    }

    /// 构造关闭拒绝，并捕获调用位置。
    ///
    /// Build a shutdown rejection, capturing the call site.
    ///
    /// - `detail`: 说明原因的英文细节
    #[track_caller]
    pub fn closed(detail: String) -> Self {
        Self::Closed {
            detail,
            site: capture_site(),
        }
    }

    /// 构造等待者超限拒绝，并捕获调用位置。
    ///
    /// Build a waiter-limit rejection, capturing the call site.
    ///
    /// - `detail`: 说明原因的英文细节
    #[track_caller]
    pub fn too_many_waiters(detail: String) -> Self {
        Self::TooManyWaiters {
            detail,
            site: capture_site(),
        }
    }
}

impl Display for ChunkSubmitError {
    /// 按错误码 + 细节 + 位置的格式格式化错误。
    ///
    /// Formats the error as code + detail + site.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { detail, site } => {
                write!(formatter, "[world.executor.busy] {detail} (at {site})")
            }
            Self::Closed { detail, site } => {
                write!(formatter, "[world.executor.closed] {detail} (at {site})")
            }
            Self::TooManyWaiters { detail, site } => {
                write!(
                    formatter,
                    "[world.executor.too_many_waiters] {detail} (at {site})"
                )
            }
        }
    }
}

/// `std::error::Error` 实现（错误源即自身字段，无嵌套源）。
///
/// `std::error::Error` implementation (no nested source).
impl std::error::Error for ChunkSubmitError {}

/// 去重键：区块归属 `(所属世界, 区块坐标)`。不同世界即使维度+坐标
/// 相同也绝不合并（与 `LevelChunkCache` 的世界隔离键同语义）。
///
/// Dedup key: chunk ownership `(saved world, chunk coords)`. Different worlds never coalesce
/// even with equal dimension+coords (same semantics as the `LevelChunkCache` world-isolation key).
#[derive(Clone, Hash, Eq, PartialEq)]
struct ExecutorKey {
    /// 所属世界 id。
    ///
    /// Owning world id.
    world: MinecraftWorldId,
    /// 区块坐标键。
    ///
    /// Chunk coordinate key.
    chunk: ChunkKey,
}

/// 交付给 worker 线程的一个工作单元。
///
/// One unit of work delivered to a worker thread.
struct ChunkJob {
    /// 区块提供器（持存储与生成器句柄）。
    ///
    /// Chunk provider (holds storage and generator handles).
    provider: WorldChunkProvider,
    /// 区块坐标键。
    ///
    /// Chunk coordinate key.
    key: ChunkKey,
    /// 世界底部 y。
    ///
    /// World bottom y.
    min_y: i32,
    /// 世界顶部 y。
    ///
    /// World top y.
    max_y: i32,
    /// 去重键（与 `key` 对应的归属键）。
    ///
    /// Dedup key (ownership key matching `key`).
    dedupe_key: ExecutorKey,
}

/// 在飞行合并表：去重键 → 等待任务结果的收据发送端列表。
///
/// In-flight coalescing table: dedup key to receipt senders waiting on the job result.
type WaiterMap = HashMap<ExecutorKey, Vec<oneshot::Sender<Arc<ChunkLoadOutcome>>>>;

/// 失败退避表的最大键数（FIFO 淘汰上限）。
///
/// Maximum number of keys kept in the failure-backoff table (FIFO cap).
const MAX_FAILURE_BACKOFF_KEYS: usize = 4096;
/// 失败退避的初始延迟。
///
/// Initial failure-backoff delay.
const INITIAL_FAILURE_BACKOFF: Duration = Duration::from_millis(50);
/// 失败退避的最大延迟。
///
/// Maximum failure-backoff delay.
const MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(2);

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

/// 反复失败的 `(world, chunk)` 任务的有界重试冷却。
/// FIFO 顺序同时让容量淘汰确定且廉价。
///
/// Bounded retry cooldown for repeatedly failing `(world, chunk)` jobs.
/// The FIFO order also makes capacity eviction deterministic and cheap.
#[derive(Default)]
struct FailureBackoff {
    /// 去重键 → 失败记录。
    ///
    /// Dedup key to failure record.
    records: HashMap<ExecutorKey, FailureRecord>,
    /// 插入顺序（用于 FIFO 淘汰）。
    ///
    /// Insertion order (used for FIFO eviction).
    order: VecDeque<ExecutorKey>,
}

impl FailureBackoff {
    /// 该键当前是否处于退避期（未到 `retry_at`）。
    ///
    /// Whether the key is currently in backoff (before its `retry_at`).
    ///
    /// - `key`: 去重键
    /// - `now`: 当前时刻
    fn is_blocked(&self, key: &ExecutorKey, now: Instant) -> bool {
        self.records
            .get(key)
            .is_some_and(|record| record.retry_at > now)
    }

    /// 记录一次失败：指数拉长退避（上限 [`MAX_FAILURE_BACKOFF`]），
    /// 超容时按 FIFO 淘汰最旧键。
    ///
    /// Records a failure: grows the backoff exponentially (capped at
    /// [`MAX_FAILURE_BACKOFF`]) and evicts the oldest key when full.
    ///
    /// - `key`: 去重键
    /// - `now`: 失败发生时刻
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

    /// 清除某键的失败记录（成功后的清理）。
    ///
    /// Clears a key's failure record (cleanup after success).
    ///
    /// - `key`: 去重键
    fn clear(&mut self, key: &ExecutorKey) {
        if self.records.remove(key).is_some() {
            self.order.retain(|entry| entry != key);
        }
    }
}

/// 服务端 tick 健康评级。按主 tick 时长的 EWMA 评估。
///
/// Server tick health rating. Evaluated by EWMA over main tick durations.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TickHealth {
    /// TPS ≥ 18（Tick Time ≤ 55.5ms）：全容量突发。
    ///
    /// TPS >= 18 (Tick Time <= 55.5ms): Full capacity bursting.
    Healthy = 0,
    /// TPS 15 ~ 18（Tick Time 55.5ms ~ 66.7ms）：中度收缩。
    ///
    /// TPS 15 ~ 18 (Tick Time 55.5ms ~ 66.7ms): Moderate scale down.
    Pressured = 1,
    /// TPS < 15（Tick Time > 66.7ms）：紧急让渡主循环，收缩到最小 worker 数。
    ///
    /// TPS < 15 (Tick Time > 66.7ms): Emergency concession to main loop, retreat to min workers.
    Critical = 2,
}

impl TickHealth {
    /// 从 u8 恢复（未知值按 [`TickHealth::Critical`]）。
    ///
    /// Restores from a u8 (unknown values map to [`TickHealth::Critical`]).
    ///
    /// - `val`: 存储的整数值
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => Self::Healthy,
            1 => Self::Pressured,
            _ => Self::Critical,
        }
    }
}

/// 执行器内部状态（worker 共享）。
///
/// Executor internal state (shared by workers).
struct ExecutorInner {
    /// 有界任务发送端。提交用 `try_send`，队列满即卸负载，因此
    /// Tokio 任务、Chunk 克隆与等待者都不会无界堆积。
    ///
    /// Bounded job sender. Submissions use `try_send` and shed load when full, so
    /// Tokio tasks, Chunk clones, and waiters never accumulate without bound.
    jobs: Mutex<Option<flume::Sender<ChunkJob>>>,
    /// MPMC 队列接收端，worker 间共享。
    ///
    /// MPMC queue receiver, shared across workers.
    rx: flume::Receiver<ChunkJob>,
    /// 在飞行合并表（同键去重）。用 `Arc` 因为 worker 在 inner 存在前就共享它。
    ///
    /// In-flight coalescing table (same-key dedup). `Arc` because workers share it before inner exists.
    inflight: Arc<Mutex<WaiterMap>>,
    /// worker/存储/生成器失败后的同键有界冷却。
    ///
    /// Bounded same-key cooldown after worker/storage/generator failures.
    failures: Arc<Mutex<FailureBackoff>>,
    /// 当前存活 worker 线程数（核心 + 活动弹性）。
    ///
    /// Current number of live worker threads (core + active elastic).
    current_workers: AtomicUsize,
    /// 正在执行 load_chunk 的 worker 数。
    ///
    /// Active worker count currently executing load_chunk.
    active_workers: AtomicUsize,
    /// 常驻最小 worker 数（超时也不退出）。
    ///
    /// Minimum persistent workers (never terminate on timeout).
    min_workers: usize,
    /// 最大 worker 并发（物理上限）。
    ///
    /// Maximum worker concurrency (physical limit).
    max_workers: usize,
    /// 当前 tick 健康状态。
    ///
    /// Current tick health status.
    tick_health: AtomicU8,
    /// worker 线程句柄（`shutdown` 时取出并 join；`Option` 保证幂等）。
    ///
    /// Worker thread handles (taken and joined at `shutdown`; `Option` keeps it idempotent).
    workers: Mutex<Option<Vec<thread::JoinHandle<()>>>>,
}

impl ExecutorInner {
    /// 按当前 tick 健康度计算目标 worker 上限。
    ///
    /// Computes the target worker limit from the current tick health.
    fn target_worker_limit(&self) -> usize {
        let health = TickHealth::from_u8(self.tick_health.load(Ordering::Relaxed));
        match health {
            TickHealth::Healthy => self.max_workers,
            TickHealth::Pressured => (self.max_workers * 2 / 3).max(self.min_workers).max(1),
            TickHealth::Critical => self.min_workers.max(1),
        }
    }
}

/// 区块加载/生成执行器（资源，接受 `chunk_pipeline` 提交）。
///
/// Chunk load/generate executor (Resource, accepts `chunk_pipeline` submissions).
#[derive(Resource, Clone)]
pub struct WorldChunkExecutor {
    inner: Arc<ExecutorInner>,
}

impl WorldChunkExecutor {
    /// 任务队列容量（有界，满即 `Busy`）。
    ///
    /// Bounded job-queue capacity (a full queue yields `Busy`).
    const JOB_QUEUE_CAPACITY: usize = 1024;
    /// 单区块最大合并等待者数。
    ///
    /// Maximum coalesced waiters per chunk.
    const MAX_WAITERS_PER_CHUNK: usize = 64;

    /// 以最大 worker 数启动区块执行器。
    /// `max_worker_count > 1` 时最小 worker 为 0，无区块生成任务时
    /// worker 完全让出系统资源给游戏逻辑。
    ///
    /// Start a chunk executor with max worker count.
    /// When max_worker_count > 1, min_workers is 0 so that workers completely yield
    /// system resources to game logic when there are no chunk generation tasks.
    ///
    /// - `max_worker_count`: 最大 worker 数（至少 1）
    pub fn new(max_worker_count: usize) -> io::Result<Self> {
        let max_workers = max_worker_count.max(1);
        let min_workers = if max_workers == 1 { 1 } else { 0 };
        Self::with_limits(min_workers, max_workers)
    }

    /// 以显式的最小常驻/最大弹性 worker 边界启动执行器。
    ///
    /// Start a chunk executor with explicit minimum persistent and maximum elastic worker bounds.
    ///
    /// - `min_workers`: 常驻最小 worker 数（被 `max_workers` 压制）
    /// - `max_workers`: 最大弹性 worker 数（至少 1）
    pub fn with_limits(min_workers: usize, max_workers: usize) -> io::Result<Self> {
        let max_workers = max_workers.max(1);
        let min_workers = min_workers.min(max_workers);
        let (tx, rx) = flume::bounded::<ChunkJob>(Self::JOB_QUEUE_CAPACITY);
        let inflight: Arc<Mutex<WaiterMap>> = Arc::new(Mutex::new(HashMap::new()));
        let failures = Arc::new(Mutex::new(FailureBackoff::default()));

        let inner = Arc::new(ExecutorInner {
            jobs: Mutex::new(Some(tx)),
            rx,
            inflight,
            failures,
            current_workers: AtomicUsize::new(min_workers),
            active_workers: AtomicUsize::new(0),
            min_workers,
            max_workers,
            tick_health: AtomicU8::new(TickHealth::Healthy as u8),
            workers: Mutex::new(Some(Vec::with_capacity(max_workers))),
        });

        for index in 0..min_workers {
            let handle = spawn_worker(&inner, true, &format!("sc-chunk-worker-core-{index}"))?;
            let mut workers = inner.workers.lock();
            if let Some(ref mut vec) = *workers {
                vec.push(handle);
            }
        }

        Ok(Self { inner })
    }

    /// 默认最大 worker 数：可用并行度减 1（给游戏 tick/网络留核），
    /// 下限 2。
    ///
    /// Default max worker count: available parallelism minus 1 core reserved for game tick / network,
    /// clamped to at least 2.
    pub fn default_max_workers() -> usize {
        let parallelism = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        parallelism.saturating_sub(1).max(2)
    }

    /// 默认 worker 数：`default_max_workers` 的别名。
    ///
    /// Default worker count: alias for default_max_workers.
    pub fn default_worker_count() -> usize {
        Self::default_max_workers()
    }

    /// 设置当前 tick 健康度；健康度改善时尝试扩容。
    ///
    /// Sets the current tick health; tries to scale up when health improves.
    ///
    /// - `health`: 新的健康评级
    pub fn set_tick_health(&self, health: TickHealth) {
        let prev = self.inner.tick_health.swap(health as u8, Ordering::Relaxed);
        if (health as u8) < prev {
            self.maybe_scale_up();
        }
    }

    /// 当前 tick 健康度。
    ///
    /// Current tick health.
    pub fn tick_health(&self) -> TickHealth {
        TickHealth::from_u8(self.inner.tick_health.load(Ordering::Relaxed))
    }

    /// 常驻最小 worker 数。
    ///
    /// Minimum persistent worker count.
    pub fn min_workers(&self) -> usize {
        self.inner.min_workers
    }

    /// 最大 worker 数。
    ///
    /// Maximum worker count.
    pub fn max_workers(&self) -> usize {
        self.inner.max_workers
    }

    /// 当前存活 worker 数。
    ///
    /// Current live worker count.
    pub fn current_workers(&self) -> usize {
        self.inner.current_workers.load(Ordering::Relaxed)
    }

    /// 正在执行 `load_chunk` 的 worker 数。
    ///
    /// Number of workers currently executing `load_chunk`.
    pub fn active_workers(&self) -> usize {
        self.inner.active_workers.load(Ordering::Relaxed)
    }

    /// 当前目标并发上限（由 tick 健康度决定）。
    ///
    /// Current target concurrency limit (determined by tick health).
    pub fn target_concurrency(&self) -> usize {
        self.inner.target_worker_limit()
    }

    /// 队列有积压且低于目标上限时 CAS 扩容 worker。
    ///
    /// CAS-scales workers up while there is queued backlog and room below the
    /// target limit.
    fn maybe_scale_up(&self) {
        let queued = self.inner.rx.len();
        if queued == 0 {
            return;
        }
        let needed = self
            .inner
            .active_workers
            .load(Ordering::Relaxed)
            .saturating_add(queued);
        let target_limit = self.inner.target_worker_limit();

        let mut current = self.inner.current_workers.load(Ordering::Acquire);
        while current < target_limit && current < needed {
            match self.inner.current_workers.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    let index = current;
                    let is_core = current < self.inner.min_workers;
                    let handle = spawn_worker(
                        &self.inner,
                        is_core,
                        &format!("sc-chunk-worker-{index}"),
                    );
                    if let Ok(handle) = handle {
                        let mut workers = self.inner.workers.lock();
                        if let Some(ref mut vec) = *workers {
                            vec.retain(|h| !h.is_finished());
                            vec.push(handle);
                        }
                    } else {
                        self.inner.current_workers.fetch_sub(1, Ordering::SeqCst);
                        break;
                    }
                    current += 1;
                }
                Err(actual) => current = actual,
            }
        }
    }

    /// 兼容包装：保留原「返回接收端」的 API。
    ///
    /// 新代码应使用 [`Self::try_submit`] 以区分队列压力与关闭。
    /// 本包装同样是非阻塞的；对被拒的准入，用旧 API 的调用方会看到
    /// 一个已关闭的接收端。
    ///
    /// Compatibility wrapper retaining the original receiver-returning API.
    ///
    /// New code should use [`Self::try_submit`] to distinguish queue pressure
    /// from shutdown. This wrapper is also non-blocking; rejected admission is
    /// represented as a closed receiver for callers using the older API.
    ///
    /// - `world`: 所属世界 id
    /// - `provider`: 区块提供器
    /// - `key`: 区块坐标键
    /// - `min_y` / `max_y`: 世界竖直范围
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

    /// 非阻塞地提交一个加载/生成请求。
    ///
    /// Submit a load/generation request without blocking the caller.
    ///
    /// 同一 `(world, chunk)` 的并发请求共享一个任务并收到同一结果。
    /// 新任务只在持有短在飞行锁并用 `try_send` 时准入，因此满队列既不会
    /// 阻塞 Tokio worker 也不会阻塞 ECS tick 线程。准入失败时先在返回
    /// 显式原因前摘除在飞行注册。
    ///
    /// Concurrent requests for the same `(world, chunk)` share one job and
    /// receive its result. A new job is admitted only while holding the short
    /// in-flight lock and using `try_send`, so a full worker queue cannot block
    /// either a Tokio worker or the ECS tick thread. On failed admission the
    /// in-flight registration is removed before returning the explicit reason.
    ///
    /// - `world`: 所属世界 id
    /// - `provider`: 区块提供器
    /// - `key`: 区块坐标键
    /// - `min_y` / `max_y`: 世界竖直范围
    ///
    /// 错误：队列满/失败退避返回 [`ChunkSubmitError::Busy`]，关闭返回
    /// [`ChunkSubmitError::Closed`]，等待者超限返回
    /// [`ChunkSubmitError::TooManyWaiters`]。
    ///
    /// Errors: [`ChunkSubmitError::Busy`] on a full queue or failure backoff,
    /// [`ChunkSubmitError::Closed`] when shutting down, and
    /// [`ChunkSubmitError::TooManyWaiters`] beyond the waiter bound.
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
                return Err(ChunkSubmitError::too_many_waiters(
                    "chunk already has the maximum number of coalesced waiters".to_string(),
                ));
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
            return Err(ChunkSubmitError::busy(
                "chunk load is in failure backoff; retry later".to_string(),
            ));
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
            Some(Ok(())) => {
                self.maybe_scale_up();
                Ok(rx)
            }
            Some(Err(flume::TrySendError::Full(_))) => {
                inflight.remove(&dedupe_key);
                Err(ChunkSubmitError::busy(
                    "chunk worker queue is full; retry later".to_string(),
                ))
            }
            Some(Err(flume::TrySendError::Disconnected(_))) | None => {
                inflight.remove(&dedupe_key);
                Err(ChunkSubmitError::closed(
                    "chunk executor is shutting down".to_string(),
                ))
            }
        }
    }

    /// 当前在飞行（排队或运行中）的去重任务数（观测/测试用）。
    ///
    /// Current in-flight (queued or running) deduped job count (observability/test use).
    pub fn in_flight(&self) -> usize {
        self.inner.inflight.lock().len()
    }

    /// 关闭：丢弃任务发送端（worker 排空后退出）并 join 全部线程。幂等。
    ///
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

/// 按名称 spawn 一个 worker 线程。
///
/// Spawns one named worker thread.
///
/// - `inner`: 共享执行器状态
/// - `is_core`: 是否常驻核心 worker
/// - `name`: 线程名
fn spawn_worker(
    inner: &Arc<ExecutorInner>,
    is_core: bool,
    name: &str,
) -> io::Result<thread::JoinHandle<()>> {
    let inner_clone = Arc::clone(inner);
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || worker_loop(is_core, inner_clone))
}

/// Worker 主循环：取任务、在本线程跑阻塞加载/生成、分发给合并等待者。
///
/// Worker main loop: take a job, run the blocking load/generate on this thread, fan out to coalesced waiters.
///
/// - `is_core`: 是否常驻核心 worker
/// - `inner`: 共享执行器状态
fn worker_loop(
    is_core: bool,
    inner: Arc<ExecutorInner>,
) {
    #[cfg(unix)]
    set_thread_background_priority();

    const IDLE_TIMEOUT: Duration = Duration::from_secs(3);

    loop {
        let target_limit = inner.target_worker_limit();
        let current = inner.current_workers.load(Ordering::Acquire);
        if current > target_limit && current > inner.min_workers {
            if inner.current_workers.compare_exchange(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                return;
            }
        }

        let job = if is_core && inner.min_workers > 0 {
            match inner.rx.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        } else {
            match inner.rx.recv_timeout(IDLE_TIMEOUT) {
                Ok(job) => job,
                Err(flume::RecvTimeoutError::Timeout) => {
                    let mut current = inner.current_workers.load(Ordering::Acquire);
                    while current > inner.min_workers {
                        match inner.current_workers.compare_exchange_weak(
                            current,
                            current - 1,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ) {
                            Ok(_) => return,
                            Err(actual) => current = actual,
                        }
                    }
                    continue;
                }
                Err(flume::RecvTimeoutError::Disconnected) => return,
            }
        };

        // A queued visual demand may have been cancelled while waiting for a
        // worker (for example, its player changed hard context). Skip the load
        // if no consumer remains; running generators are not force-cancelled.
        let has_live_waiters = {
            let mut waiters_by_key = inner.inflight.lock();
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

        struct ActiveGuard<'a>(&'a AtomicUsize);
        impl<'a> Drop for ActiveGuard<'a> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::Relaxed);
            }
        }

        inner.active_workers.fetch_add(1, Ordering::Relaxed);
        let active_guard = ActiveGuard(&inner.active_workers);

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
                    let reason = sc_async::panic_message(payload.as_ref());
                    log::error!(
                        "{}",
                        t_log!(
                            "console.world.load_panic",
                            world = format!("{:?}", job.dedupe_key.world),
                            key = format!("{:?}", job.key),
                            reason = reason
                        )
                    );
                    Err(WorldStorageError::backend(format!(
                        "chunk load worker panicked: {reason}"
                    )))
                }
            },
        );

        drop(active_guard);

        // Explicit cooperative yield to game logic and RakNet threads between chunk jobs
        thread::yield_now();

        // Update retry state while still holding `inflight`: try_submit uses
        // the same lock order, so a new request cannot slip between failure
        // publication and removal of this attempt.
        let mut waiters_by_key = inner.inflight.lock();
        {
            let mut backoff = inner.failures.lock();
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

    /// 计数生成器：统计实际生成次数（验证同键去重）。
    ///
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
        WorldChunkProvider::new_testing(Arc::new(InMemoryWorldStorage::default())).with_generator(generator)
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
                    .map_err(|error| WorldStorageError::backend(error.to_string()))?;
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
            Err(ChunkSubmitError::Busy { .. })
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
            Err(ChunkSubmitError::Busy { .. }),
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
            Err(ChunkSubmitError::TooManyWaiters { .. }),
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
            Err(ChunkSubmitError::Closed { .. }),
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

    /// 第一次加载失败、第二次成功的存储（验证失败后重试）。
    ///
    /// Storage that fails the first load and succeeds the second (verifies retry-after-failure).
    struct FailOnceStorage {
        failed: std::sync::atomic::AtomicBool,
    }
    impl WorldStorage for FailOnceStorage {
        fn load_chunk(&self, key: ChunkKey) -> Result<Option<Chunk>, WorldStorageError> {
            if !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return Err(WorldStorageError::backend(
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
        let provider = WorldChunkProvider::new_testing(storage);
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
            Err(ChunkSubmitError::Busy { .. })
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

    /// 小型阻塞等待（仅测试）：在临时 runtime 上等待一个 oneshot。
    ///
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

    #[test]
    fn elastic_pool_concession_under_tps_health_transitions() {
        let executor = WorldChunkExecutor::with_limits(2, 6).expect("create elastic executor");
        assert_eq!(executor.current_workers(), 2);
        assert_eq!(executor.target_concurrency(), 6);
        assert_eq!(executor.tick_health(), TickHealth::Healthy);

        // Under Pressured: target drops to 2/3 (4)
        executor.set_tick_health(TickHealth::Pressured);
        assert_eq!(executor.tick_health(), TickHealth::Pressured);
        assert_eq!(executor.target_concurrency(), 4);

        // Under Critical: target drops to min_workers (2)
        executor.set_tick_health(TickHealth::Critical);
        assert_eq!(executor.tick_health(), TickHealth::Critical);
        assert_eq!(executor.target_concurrency(), 2);

        // Under Healthy: target restores to max_workers (6)
        executor.set_tick_health(TickHealth::Healthy);
        assert_eq!(executor.tick_health(), TickHealth::Healthy);
        assert_eq!(executor.target_concurrency(), 6);

        executor.shutdown();
    }

    #[test]
    fn elastic_pool_scales_up_under_backlog() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let generator = Arc::new(BlockingGenerator {
            started: started_tx,
            release: Mutex::new(release_rx),
            blocked_once: std::sync::atomic::AtomicBool::new(false),
            calls: std::sync::atomic::AtomicU32::new(0),
        });
        let executor = WorldChunkExecutor::with_limits(1, 4).expect("create elastic executor");
        let provider = provider_with_generator(generator);
        let world = MinecraftWorldId::random();

        assert_eq!(executor.current_workers(), 1);

        // Submit first request to block worker 1
        let first = executor
            .try_submit(world.clone(), provider.clone(), ChunkKey::new(0, ChunkPosition::new(0, 0)), -64, 319)
            .expect("first submit");
        started_rx.recv_timeout(Duration::from_secs(2)).expect("first started");

        // Submit multiple additional requests to create backlog
        let mut receivers = Vec::new();
        for i in 1..=4 {
            let rx = executor
                .try_submit(world.clone(), provider.clone(), ChunkKey::new(0, ChunkPosition::new(i, 0)), -64, 319)
                .expect("subsequent submit");
            receivers.push(rx);
        }

        // Wait shortly for scale-up to trigger
        std::thread::sleep(Duration::from_millis(50));
        assert!(executor.current_workers() > 1, "executor must have scaled up workers");
        assert!(executor.current_workers() <= 4, "executor must not exceed max workers");

        // Release all jobs
        drop(release_tx);
        drop(first);
        for rx in receivers {
            drop(rx);
        }
        executor.shutdown();
    }

    #[test]
    fn idle_executor_scales_from_zero_on_demand() {
        let generator = Arc::new(CountingGenerator::new());
        let executor = WorldChunkExecutor::with_limits(0, 4).expect("create zero-min executor");
        let provider = provider_with_generator(generator.clone());
        let world = MinecraftWorldId::random();

        // When idle, min_workers is 0 so 0 threads initially run
        assert_eq!(executor.current_workers(), 0);

        // Submitting a job on demand triggers scale up from 0
        let rx = executor
            .try_submit(world, provider, ChunkKey::new(0, ChunkPosition::new(0, 0)), -64, 319)
            .expect("submit");

        let outcome = futures_block_on(rx).expect("recv");
        assert!(outcome.is_ok());
        assert_eq!(generator.count(), 1);
        assert!(executor.current_workers() >= 1);

        executor.shutdown();
    }
}
