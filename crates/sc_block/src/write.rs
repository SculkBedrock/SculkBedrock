//! 单写者方块变更流水线。
//!
//! Single-writer block mutation pipeline.
//!
//! 生产方入队 [`BlockChange`]。PostUpdate 系统把变更立即应用到已缓存的列，
//! 缓存未命中的则委托给 [`sc_world::chunk_executor::WorldChunkExecutor`]。
//! ECS tick 线程上不发生磁盘 I/O 或区块生成。
//!
//! Producers enqueue [`BlockChange`] values. The PostUpdate systems apply
//! changes to already cached columns immediately and delegate cache misses to
//! [`sc_world::chunk_executor::WorldChunkExecutor`]. No disk I/O or chunk
//! generation runs on the ECS tick thread.

use log::warn;
use sc_ecs::entity::EntityId;
use sc_ecs::event::Event;
use sc_ecs::params::resource::ResMut;
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_log::t_log;
use sc_world::chunk::{BlockRuntimeId, SUBCHUNK_SIZE};
use sc_world::chunk_executor::{ChunkLoadOutcome, ChunkSubmitError, WorldChunkExecutor};
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use sc_world::storage::{ChunkColumn, ChunkKey, WorldChunkProvider};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// UpdateBlock 网络标志位。
///
/// UpdateBlock network flags.
pub mod update_flags {
    /// 通知邻居方块更新 / Notify neighbor block updates.
    pub const NEIGHBORS: u32 = 0b0001;
    /// 广播到网络 / Broadcast to the network.
    pub const NETWORK: u32 = 0b0010;
    /// 无图形更新（跳过渲染刷新）/ No graphic update (skips render refresh).
    pub const NO_GRAPHIC: u32 = 0b0100;
    /// 优先发送 / Send with priority.
    pub const PRIORITY: u32 = 0b1000;
    /// 默认组合：邻居 + 网络 / Default combination: neighbors + network.
    pub const DEFAULT: u32 = NEIGHBORS | NETWORK;
}

/// 方块变更的来源原因（进入 `BlockChanged` 事实，供消费方分流）。
///
/// Cause of a block change (carried into `BlockChanged` facts for consumers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockChangeCause {
    /// 玩家行为 / Player action.
    Player(EntityId),
    /// 命令触发 / Command-triggered.
    Command,
    /// 插件触发 / Plugin-triggered.
    Plugin,
    /// 世界生成 / World generation.
    Generation,
    /// 红石逻辑 / Redstone logic.
    Redstone,
}

/// 一次待应用的方块变更请求。
///
/// One pending block change request.
#[derive(Clone, Debug)]
pub struct BlockChange {
    /// 目标世界 / Target world.
    pub world_id: MinecraftWorldId,
    /// 目标方块坐标 / Target block position.
    pub position: crate::position::BlockPosition,
    /// 目标层（通常 0）/ Target layer (usually 0).
    pub layer: u8,
    /// 目标状态哈希 / Target state hash.
    pub state: BlockRuntimeId,
    /// 变更原因 / Change cause.
    pub cause: BlockChangeCause,
    /// UpdateBlock 网络标志位 / UpdateBlock network flags.
    pub flags: u32,
}

/// 队列中的待处理变更（携带请求 id 与可选的破坏上下文）。
///
/// A pending change inside the queue (carries the request id and optional break context).
#[derive(Debug)]
pub struct PendingBlockChange {
    /// 请求 id（回发 [`BlockChangeResult`] 时对应）/ Request id (matched by [`BlockChangeResult`]).
    pub request_id: u64,
    /// 变更本体 / The change itself.
    pub change: BlockChange,
    /// 可选的前置期望（乐观并发控制）/ Optional precondition (optimistic concurrency control).
    pub expectation: Option<BlockExpectation>,
    /// 可选的破坏上下文（挖掘掉落需要）/ Optional break context (needed for mining drops).
    pub break_context: Option<BlockBreakContext>,
}

/// 应用前期望看到的方块状态（incarnation/generation 防旧写覆写新值）。
///
/// Block state expected before applying (incarnation/generation guard stale writes
/// from clobbering newer values).
#[derive(Clone, Copy, Debug)]
pub struct BlockExpectation {
    /// 期望的状态哈希 / Expected state hash.
    pub state: BlockRuntimeId,
    /// 期望的列 incarnation / Expected column incarnation.
    pub incarnation: u128,
    /// 期望的列 generation / Expected column generation.
    pub generation: u64,
}

/// 破坏方块时的上下文快照（供挖掘掉落结算使用）。
///
/// Context snapshot taken when breaking a block (used for mining-drop settlement).
#[derive(Clone, Debug)]
pub struct BlockBreakContext {
    /// 手持物品快照 / Held-item snapshot.
    pub hand: crate::mining_drops::HandSnapshot,
    /// 是否创造模式 / Whether in creative mode.
    pub creative: bool,
    /// 破坏开始时的工具已损耗值 / Tool damage when the break began.
    pub initial_damage: u32,
}

/// 方块变更结果事件（按请求 id 回报是否应用）。
///
/// Block change result event (reports whether the change was applied, by request id).
#[derive(Event, Clone, Debug)]
pub struct BlockChangeResult {
    /// 对应请求的 id / Id of the corresponding request.
    pub request_id: u64,
    /// 是否成功应用 / Whether it was applied successfully.
    pub applied: bool,
}

/// 空队列首次写入时的预分配容量。
///
/// Preallocated slots on first write to an empty queue.
///
/// 理由见 [`BlockChangeQueue::drain`]。
///
/// Rationale, see [`BlockChangeQueue::drain`].
const BLOCK_CHANGE_QUEUE_PREALLOC: usize = 4096;

/// 单写者方块变更队列（资源）。
///
/// Single-writer block change queue (resource).
#[derive(Resource, Default)]
pub struct BlockChangeQueue {
    /// 待处理变更列表 / Pending change list.
    pending: Vec<PendingBlockChange>,
    /// 下一个请求 id（回绕递增）/ Next request id (wrapping increment).
    next_request_id: u64,
}

impl BlockChangeQueue {
    /// 入队一次变更，返回请求 id。
    ///
    /// Enqueues one change and returns the request id.
    ///
    /// - `change`: 待入队的变更
    pub fn push(&mut self, change: BlockChange) -> u64 {
        // After `drain()` returns capacity, `pending` becomes `Vec::new()` (capacity 0), so the
        // next changed tick would double from 4 to N (~log2 N reallocs).
        // Reserving up to the high-water mark on first write to an empty queue amortizes
        // the doubling cost into one allocation.
        if self.pending.capacity() == 0 {
            self.pending.reserve(BLOCK_CHANGE_QUEUE_PREALLOC);
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.pending.push(PendingBlockChange {
            request_id,
            change,
            expectation: None,
            break_context: None,
        });
        request_id
    }

    /// 尝试入队一次带期望与破坏上下文的破坏变更；事实队列满时拒绝。
    ///
    /// Tries to enqueue a break change with expectation and break context; refuses
    /// when the fact queue is full.
    ///
    /// - `change`: 待入队的变更
    /// - `expectation`: 应用前期望看到的状态
    /// - `context`: 破坏上下文
    ///
    /// 返回 `None` 表示因容量限制被拒绝（调用方应回滚预留）。
    ///
    /// Returns `None` when refused due to capacity (callers should roll back
    /// reservations).
    pub fn try_push_break(
        &mut self,
        change: BlockChange,
        expectation: BlockExpectation,
        context: BlockBreakContext,
    ) -> Option<u64> {
        if self.pending.len() >= BlockChangedQueue::MAX_ENTRIES {
            return None;
        }
        let id = self.push(change);
        let pending = self.pending.last_mut().expect("just pushed");
        pending.expectation = Some(expectation);
        pending.break_context = Some(context);
        Some(id)
    }

    /// 把已加载但暂不可应用的变更重新压回队头等待（保持 FIFO 顺序）。
    ///
    /// Pushes a loaded-but-not-yet-appliable change back onto the queue (keeps FIFO order).
    ///
    /// - `pending`: 待重新入队的变更
    fn defer(&mut self, pending: PendingBlockChange) {
        self.pending.push(pending);
    }

    /// 队列长度 / Queue length.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// 队列是否为空 / Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// 取走并清空全部待处理变更。
    ///
    /// Takes and clears all pending changes.
    ///
    /// 必须使用 `mem::take`：三个调用方都按值消费
    /// （`for PendingBlockChange { .. } in pending`，其中 `change` 被移入
    /// `apply_loaded_change` 或 `PendingBlockChangeLoad`），无法就地进行迭代。
    ///
    /// Must use `mem::take` here: all three callers consume by value
    /// (`for PendingBlockChange { .. } in pending`, where `change` is moved into
    /// `apply_loaded_change` or `PendingBlockChangeLoad`), so in-place iteration
    /// over the internal buffer is not possible.
    ///
    /// 代价是把 `pending` 的容量还给分配器。缓解措施是 [`Self::push`] 在空队列
    /// 首次写入时预留 `BLOCK_CHANGE_QUEUE_PREALLOC`，避免每个 tick 重新倍增。
    ///
    /// The cost is returning `pending` capacity to the allocator. The mitigation is
    /// [`Self::push`] reserving `BLOCK_CHANGE_QUEUE_PREALLOC` on first write to an empty queue,
    /// avoiding re-doubling every tick.
    pub fn drain(&mut self) -> Vec<PendingBlockChange> {
        std::mem::take(&mut self.pending)
    }
}

/// 等待区块加载的待处理变更上限 / Cap on pending changes awaiting chunk load.
const MAX_PENDING_BLOCK_CHANGE_LOADS: usize = 4096;
/// 变更在 executor 繁忙状态下最长等待时长（超时发失败结果）/ Longest a change may
/// wait under executor backpressure before a failure result is emitted.
const MAX_BLOCK_CHANGE_BUSY_AGE: Duration = Duration::from_secs(60);

/// 等待区块加载的变更表（资源）。
///
/// Table of changes awaiting chunk load (resource).
#[derive(Resource, Default)]
pub struct PendingBlockChangeLoads {
    /// 待加载变更列表，`Mutex` 保护 / Pending-load list, guarded by `Mutex`.
    pending: Mutex<Vec<PendingBlockChangeLoad>>,
}

/// 一条等待区块加载的变更。
///
/// One change awaiting chunk load.
struct PendingBlockChangeLoad {
    /// 请求 id / Request id.
    request_id: u64,
    /// 变更本体 / The change itself.
    change: BlockChange,
    /// 已提交的加载接收器（尚未返回时存在）/ Receiver for a submitted load (present until it resolves).
    receiver: Option<oneshot::Receiver<Arc<ChunkLoadOutcome>>>,
    /// 进入繁忙等待的起点 / When backpressure waiting began.
    busy_since: Instant,
    /// 下次重试时刻 / Next retry time.
    retry_at: Instant,
    /// 繁忙重试计数 / Backpressure retry count.
    busy_retries: u8,
    /// 可选的前置期望 / Optional precondition.
    expectation: Option<BlockExpectation>,
    /// 可选的破坏上下文 / Optional break context.
    break_context: Option<BlockBreakContext>,
}

impl PendingBlockChangeLoad {
    /// 新建一条待加载变更；无接收器时立即安排首次重试。
    ///
    /// Creates a pending load; schedules the first retry immediately when there is
    /// no receiver.
    ///
    /// - `request_id`: 请求 id
    /// - `change`: 变更本体
    /// - `receiver`: 已提交加载的接收器（未提交时为 `None`）
    fn new(
        request_id: u64,
        change: BlockChange,
        receiver: Option<oneshot::Receiver<Arc<ChunkLoadOutcome>>>,
    ) -> Self {
        let now = Instant::now();
        let retry_at = if receiver.is_some() {
            now
        } else {
            now + block_change_retry_delay(0)
        };
        Self {
            request_id,
            change,
            receiver,
            busy_since: now,
            retry_at,
            busy_retries: 0,
            expectation: None,
            break_context: None,
        }
    }

    /// 繁忙后退避：重试计数 +1，并按退避策略推迟重试时刻。
    ///
    /// Backs off after busy: increments the retry count and postpones the retry
    /// time per the backoff policy.
    ///
    /// - `now`: 当前时刻
    fn defer_after_busy(&mut self, now: Instant) {
        self.busy_retries = self.busy_retries.saturating_add(1);
        self.retry_at = now + block_change_retry_delay(self.busy_retries);
    }
}

/// 一次加载尝试所需的世界上下文（provider、维度与垂直边界）。
///
/// World context needed for one load attempt (provider, dimension and vertical bounds).
#[derive(Clone)]
struct BlockChangeLoadContext {
    /// 区块存储提供者 / Chunk storage provider.
    provider: WorldChunkProvider,
    /// 维度 id / Dimension id.
    dimension: i32,
    /// 世界最低 y / World minimum y.
    min_y: i32,
    /// 世界最高 y / World maximum y.
    max_y: i32,
}

impl BlockChangeLoadContext {
    /// 计算变更目标所在的区块键。
    ///
    /// Computes the chunk key for the change target.
    ///
    /// - `change`: 目标变更
    fn key_for(&self, change: &BlockChange) -> ChunkKey {
        ChunkKey::new(self.dimension, change.position.chunk_position())
    }
}

/// 繁忙重试的退避延时：50ms 起，每次翻倍，上限 800ms。
///
/// Backoff delay for busy retries: starts at 50ms, doubles each time, capped at 800ms.
///
/// - `retries`: 已重试次数
fn block_change_retry_delay(retries: u8) -> Duration {
    let shift = u32::from(retries.min(4));
    Duration::from_millis((50_u64 << shift).min(800))
}

/// 把待加载变更压入等待表；表满或锁中毒时返回原条目。
///
/// Pushes a pending load into the waiting table; returns the original entry when
/// the table is full or the lock is poisoned.
///
/// - `pending`: 等待表资源
/// - `entry`: 待入队条目
fn try_queue_pending_block_change(
    pending: &PendingBlockChangeLoads,
    entry: PendingBlockChangeLoad,
) -> Result<(), PendingBlockChangeLoad> {
    let Ok(mut queue) = pending.pending.lock() else {
        return Err(entry);
    };
    if queue.len() >= MAX_PENDING_BLOCK_CHANGE_LOADS {
        return Err(entry);
    }
    queue.push(entry);
    Ok(())
}

/// 方块已变更事实事件（含前后状态，供网络广播与掉落结算消费）。
///
/// Block-changed fact event (carries previous/current state for network broadcast
/// and drop settlement).
#[derive(Event, Clone, Debug)]
pub struct BlockChanged {
    /// 对应请求的 id / Id of the corresponding request.
    pub request_id: u64,
    /// 所在世界 / Owning world.
    pub world_id: MinecraftWorldId,
    /// 维度 id / Dimension id.
    pub dimension: i32,
    /// 列的 incarnation / Column incarnation.
    pub incarnation: u128,
    /// 列的 generation / Column generation.
    pub generation: u64,
    /// 方块坐标 / Block position.
    pub position: crate::position::BlockPosition,
    /// 层 / Layer.
    pub layer: u8,
    /// 变更前状态 / Previous state.
    pub previous: BlockRuntimeId,
    /// 变更后状态 / Current state.
    pub current: BlockRuntimeId,
    /// 变更原因 / Change cause.
    pub cause: BlockChangeCause,
    /// UpdateBlock 网络标志位 / UpdateBlock network flags.
    pub flags: u32,
    /// 可选的破坏上下文 / Optional break context.
    pub break_context: Option<BlockBreakContext>,
}

/// 面向高频消费者的可靠方块事实队列。
///
/// Reliable block facts for high-frequency consumers.
///
/// ECS 事件仍然保留（兼容旧消费者），但绝不能漏掉成功写入的玩法系统
/// 应消费这个显式 FIFO 队列。
///
/// ECS events remain available for compatibility, but gameplay systems that
/// must not miss a successful write consume this explicit FIFO queue instead.
#[derive(Resource, Default)]
pub struct BlockChangedQueue {
    /// 事实队列 / Fact queue.
    entries: VecDeque<BlockChanged>,
}

impl BlockChangedQueue {
    /// 队列容量上限 / Queue capacity cap.
    pub const MAX_ENTRIES: usize = 4096;

    /// 是否还有容量 / Whether capacity remains.
    pub fn has_capacity(&self) -> bool {
        self.entries.len() < Self::MAX_ENTRIES
    }

    /// 弹出队首事实 / Pops the front fact.
    pub fn pop_front(&mut self) -> Option<BlockChanged> {
        self.entries.pop_front()
    }

    /// 压入一条事实（调用前必须已预留容量）。
    ///
    /// Pushes one fact (capacity must be reserved before mutation).
    ///
    /// - `change`: 待入队事实
    pub fn push(&mut self, change: BlockChanged) {
        assert!(
            self.has_capacity(),
            "block fact capacity must be reserved before mutation"
        );
        self.entries.push_back(change);
    }

    /// 取走并清空全部事实 / Takes and clears all facts.
    pub fn drain(&mut self) -> Vec<BlockChanged> {
        self.entries.drain(..).collect()
    }

    /// 队列长度 / Queue length.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 队列是否为空 / Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 向世界发送一次变更结果事件。
///
/// Sends one change result event to the world.
///
/// - `world`: ECS 世界
/// - `request_id`: 对应请求 id
/// - `applied`: 是否成功应用
fn send_change_result(world: &World, request_id: u64, applied: bool) {
    world.send_event(BlockChangeResult {
        request_id,
        applied,
    });
}

/// 把已加载列上的变更实际落列；事实队列满时返回原变更以便 defer。
///
/// Applies a change onto an already-loaded column; returns the original change for
/// deferral when the fact queue is full.
///
/// 期望校验失败（incarnation/generation/状态不匹配）时不改写方块，
/// 仅推送纠正事实并回报失败。
///
/// On expectation mismatch (incarnation/generation/state), the block is not
/// rewritten; only a correction fact is pushed and failure is reported.
///
/// - `world`: ECS 世界
/// - `pending`: 待应用变更
/// - `column`: 已加载的区块列
fn apply_loaded_change(
    world: &World,
    pending: PendingBlockChange,
    column: Arc<ChunkColumn>,
) -> Result<(), PendingBlockChange> {
    let Some(mut facts) = world.get_resource_mut::<BlockChangedQueue>() else {
        return Err(pending);
    };
    if !facts.has_capacity() {
        return Err(pending);
    }
    let PendingBlockChange {
        request_id,
        change,
        expectation,
        break_context,
    } = pending;
    let subchunk_y = change.position.y.div_euclid(SUBCHUNK_SIZE) as i8;
    let (previous, dimension, generation) = {
        let mut chunk = column.write();
        if let Some(expected) = expectation {
            let current = chunk.block_at_layer(
                change.layer as usize,
                change.position.local_x(),
                change.position.y,
                change.position.local_z(),
            );
            if column.incarnation() != expected.incarnation
                || column.generation() != expected.generation
                || current != Some(expected.state)
            {
                if let Some(current) = current {
                    facts.push(BlockChanged {
                        request_id,
                        world_id: change.world_id.clone(),
                        dimension: chunk.dimension,
                        incarnation: column.incarnation(),
                        generation: column.generation(),
                        position: change.position,
                        layer: change.layer,
                        previous: current,
                        current,
                        cause: change.cause,
                        flags: update_flags::NETWORK,
                        break_context: None,
                    });
                }
                send_change_result(world, request_id, false);
                return Ok(());
            }
        }
        let previous = chunk.set_block_at(
            change.layer as usize,
            change.position.local_x(),
            change.position.y,
            change.position.local_z(),
            change.state,
        );
        if previous.is_some_and(|previous| previous != change.state) {
            column.mark_dirty_locked(&chunk, subchunk_y);
        }
        (previous, chunk.dimension, column.generation())
    };
    let Some(previous) = previous else {
        warn!(
            "{}",
            t_log!(
                "console.block.change_rejected",
                request = request_id,
                pos = change.position
            )
        );
        send_change_result(world, request_id, false);
        return Ok(());
    };
    if previous == change.state {
        send_change_result(world, request_id, true);
        return Ok(());
    }

    let changed = BlockChanged {
        request_id,
        world_id: change.world_id.clone(),
        dimension,
        incarnation: column.incarnation(),
        generation,
        position: change.position,
        layer: change.layer,
        previous,
        current: change.state,
        cause: change.cause,
        flags: change.flags,
        break_context,
    };
    world.send_event(changed.clone());
    facts.push(changed);
    send_change_result(world, request_id, true);
    Ok(())
}

/// 轮询区块 worker 结果，并以有界重试消化 executor 繁忙，不阻塞 tick 线程。
/// 繁忙的变更最多挂起一分钟；超时后发出失败结果，供库存预留回滚。
///
/// Poll chunk worker results and retry bounded executor backpressure without
/// blocking the tick thread. Busy edits remain pending for up to one minute;
/// after that, a failure result is emitted so inventory reservations can roll back.
///
/// - `world`: ECS 世界
/// - `pending`: 等待加载的变更表
pub fn poll_block_change_loads(world: World, pending: ResMut<PendingBlockChangeLoads>) {
    let executor = world
        .get_resource::<WorldChunkExecutor>()
        .map(|resource| (*resource).clone());
    let now = Instant::now();
    let ready = {
        let manager = world.get_resource::<MinecraftWorldManager>();
        let Ok(mut pending) = pending.pending.lock() else {
            return;
        };
        let mut ready = Vec::new();
        let mut remaining = Vec::with_capacity(pending.len());
        let mut contexts: HashMap<MinecraftWorldId, Option<BlockChangeLoadContext>> =
            HashMap::new();

        for mut entry in pending.drain(..) {
            let world_id = entry.change.world_id.clone();
            let context = contexts.entry(world_id.clone()).or_insert_with(|| {
                manager.as_ref().and_then(|manager| {
                    manager.get_world(&world_id).map(|minecraft_world| {
                        let (min_y, max_y) = minecraft_world.vertical_bounds();
                        BlockChangeLoadContext {
                            provider: minecraft_world.chunk_provider.clone(),
                            dimension: minecraft_world.world_data.get_dimension(),
                            min_y,
                            max_y,
                        }
                    })
                })
            });
            let Some(world_context) = context.as_ref() else {
                warn!(
                    "{}",
                    t_log!("console.block.world_unloaded", request = entry.request_id)
                );
                ready.push((entry, None, None));
                continue;
            };
            let context = BlockChangeLoadContext {
                provider: world_context.provider.clone(),
                dimension: world_context.dimension,
                min_y: world_context.min_y,
                max_y: world_context.max_y,
            };
            let key = context.key_for(&entry.change);

            if let Some(mut receiver) = entry.receiver.take() {
                match receiver.try_recv() {
                    Ok(outcome) => ready.push((entry, Some(outcome), Some(context.clone()))),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                        entry.receiver = Some(receiver);
                        remaining.push(entry);
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        ready.push((entry, None, Some(context.clone())));
                    }
                }
                continue;
            }

            if now < entry.retry_at {
                remaining.push(entry);
                continue;
            }
            if let Some(column) = context.provider.cached_chunk(key) {
                ready.push((
                    entry,
                    Some(Arc::new(Ok(Some(column)))),
                    Some(context.clone()),
                ));
                continue;
            }
            if now.duration_since(entry.busy_since) >= MAX_BLOCK_CHANGE_BUSY_AGE {
                warn!(
                    "{}",
                    t_log!("console.block.executor_busy_60s", request = entry.request_id)
                );
                ready.push((entry, None, Some(context.clone())));
                continue;
            }

            let Some(executor) = executor.as_ref() else {
                warn!(
                    "{}",
                    t_log!("console.block.executor_unavailable", request = entry.request_id)
                );
                ready.push((entry, None, Some(context.clone())));
                continue;
            };
            match executor.try_submit(
                world_id,
                context.provider.clone(),
                key,
                context.min_y,
                context.max_y,
            ) {
                Ok(receiver) => {
                    entry.receiver = Some(receiver);
                    remaining.push(entry);
                }
                Err(ChunkSubmitError::Busy { .. } | ChunkSubmitError::TooManyWaiters { .. }) => {
                    entry.defer_after_busy(now);
                    remaining.push(entry);
                }
                Err(ChunkSubmitError::Closed { .. }) => {
                    warn!(
                        "block change #{}: chunk executor is closed",
                        entry.request_id
                    );
                    ready.push((entry, None, Some(context.clone())));
                }
            }
        }
        *pending = remaining;
        ready
    };

    for (entry, outcome, context) in ready {
        let column = match (context.as_ref(), outcome.as_deref()) {
            (Some(_), Some(Ok(Some(column)))) => Some(column.clone()),
            (Some(context), Some(Ok(None))) => match context.provider.insert_empty_chunk(
                context.key_for(&entry.change),
                context.min_y,
                context.max_y,
            ) {
                Ok(column) => Some(column),
                Err(error) => {
                    warn!(
                        "{}",
                        t_log!(
                            "console.block.admission_rejected",
                            request = entry.request_id,
                            error = error
                        )
                    );
                    None
                }
            },
            (_, Some(Err(error))) => {
                warn!(
                    "{}",
                    t_log!(
                        "console.block.load_failed",
                        request = entry.request_id,
                        error = error
                    )
                );
                None
            }
            (_, None) => {
                warn!(
                    "{}",
                    t_log!("console.block.load_cancelled", request = entry.request_id)
                );
                None
            }
            (None, Some(Ok(_))) => None,
        };

        if let Some(column) = column {
            let pending = PendingBlockChange {
                request_id: entry.request_id,
                change: entry.change,
                expectation: entry.expectation,
                break_context: entry.break_context,
            };
            if let Err(pending) = apply_loaded_change(&world, pending, column) {
                if let Some(mut queue) = world.get_resource_mut::<BlockChangeQueue>() {
                    queue.defer(pending);
                }
            }
        } else {
            send_change_result(&world, entry.request_id, false);
        }
    }
}

/// 应用队列中的变更，且不执行同步存储 I/O（未命中缓存的走 executor）。
///
/// Apply queued changes without performing synchronous storage I/O (cache misses
/// go through the executor).
///
/// - `world`: ECS 世界
/// - `queue`: 方块变更队列
/// - `pending_loads`: 等待加载的变更表
pub fn apply_block_changes(
    world: World,
    mut queue: ResMut<BlockChangeQueue>,
    pending_loads: ResMut<PendingBlockChangeLoads>,
) {
    if queue.is_empty() {
        return;
    }

    let pending = queue.drain();
    let Some(manager) = world.get_resource::<MinecraftWorldManager>() else {
        warn!(
            "{}",
            t_log!("console.block.manager_not_ready", count = pending.len())
        );
        for PendingBlockChange { request_id, .. } in pending {
            send_change_result(&world, request_id, false);
        }
        return;
    };
    let executor = world.get_resource::<WorldChunkExecutor>();

    for entry in pending {
        let PendingBlockChange {
            request_id,
            change,
            expectation,
            break_context,
        } = entry;
        let Some(minecraft_world) = manager.get_world(&change.world_id) else {
            warn!("block change #{request_id} targets unknown world; dropped");
            send_change_result(&world, request_id, false);
            continue;
        };
        let (min_y, max_y) = minecraft_world.vertical_bounds();
        if change.position.y < min_y || change.position.y > max_y {
            warn!(
                "{}",
                t_log!(
                    "console.block.out_of_bounds",
                    request = request_id,
                    pos = change.position
                )
            );
            send_change_result(&world, request_id, false);
            continue;
        }

        let key = ChunkKey::new(
            minecraft_world.world_data.get_dimension(),
            change.position.chunk_position(),
        );
        if let Some(column) = minecraft_world.chunk_provider.cached_chunk(key) {
            let entry = PendingBlockChange {
                request_id,
                change,
                expectation,
                break_context,
            };
            if let Err(entry) = apply_loaded_change(&world, entry, column) {
                queue.defer(entry);
            }
            continue;
        }

        let Some(executor) = executor.as_ref() else {
            warn!("{}", t_log!("console.block.executor_missing", request = request_id));
            send_change_result(&world, request_id, false);
            continue;
        };
        let submission = executor.try_submit(
            change.world_id.clone(),
            minecraft_world.chunk_provider.clone(),
            key,
            min_y,
            max_y,
        );
        let receiver = match submission {
            Ok(receiver) => Some(receiver),
            Err(ChunkSubmitError::Busy { .. } | ChunkSubmitError::TooManyWaiters { .. }) => {
                log::debug!("block change #{request_id}: executor busy; queueing bounded retry");
                None
            }
            Err(ChunkSubmitError::Closed { .. }) => {
                warn!("{}", t_log!("console.block.executor_closed", request = request_id));
                send_change_result(&world, request_id, false);
                continue;
            }
        };
        let mut entry = PendingBlockChangeLoad::new(request_id, change, receiver);
        entry.expectation = expectation;
        entry.break_context = break_context;
        if try_queue_pending_block_change(&pending_loads, entry).is_err() {
            warn!("{}", t_log!("console.block.load_limit", request = request_id));
            send_change_result(&world, request_id, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::BlockPosition;

    fn change(state: u32) -> BlockChange {
        BlockChange {
            world_id: MinecraftWorldId::random(),
            position: BlockPosition::new(0, 0, 0),
            layer: 0,
            state: BlockRuntimeId(state),
            cause: BlockChangeCause::Command,
            flags: update_flags::DEFAULT,
        }
    }

    fn guarded_fixture() -> (World, Arc<ChunkColumn>, PendingBlockChange) {
        let world = World::new();
        world.insert_resource(BlockChangedQueue::default());
        let column = Arc::new(ChunkColumn::new(sc_world::chunk::Chunk::empty(
            sc_world::chunk::ChunkPosition::new(0, 0),
            0,
            -64,
            319,
        )));
        column.write().set_block_at(0, 0, 64, 0, BlockRuntimeId(7));
        let pending = PendingBlockChange {
            request_id: 42,
            change: BlockChange {
                position: BlockPosition::new(0, 64, 0),
                state: BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()),
                ..change(0)
            },
            expectation: Some(BlockExpectation {
                state: BlockRuntimeId(7),
                incarnation: column.incarnation(),
                generation: column.generation(),
            }),
            break_context: Some(BlockBreakContext {
                hand: crate::mining_drops::HandSnapshot {
                    item: Some("minecraft:iron_pickaxe".into()),
                    ..Default::default()
                },
                creative: false,
                initial_damage: 0,
            }),
        };
        (world, column, pending)
    }

    #[test]
    fn stale_break_does_not_delete_replacement_and_emits_correction() {
        let (world, column, pending) = guarded_fixture();
        column.write().set_block_at(0, 0, 64, 0, BlockRuntimeId(8));
        apply_loaded_change(&world, pending, column.clone()).unwrap();
        assert_eq!(column.read().block_at(0, 64, 0), Some(BlockRuntimeId(8)));
        let facts = world
            .get_resource_mut::<BlockChangedQueue>()
            .unwrap()
            .drain();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].previous, facts[0].current);
        assert_eq!(facts[0].current, BlockRuntimeId(8));
    }

    #[test]
    fn full_fact_queue_defers_mutation_and_preserves_break_context() {
        let (world, column, pending) = guarded_fixture();
        for request_id in 0..BlockChangedQueue::MAX_ENTRIES {
            world
                .get_resource_mut::<BlockChangedQueue>()
                .unwrap()
                .push(BlockChanged {
                    request_id: request_id as u64,
                    world_id: pending.change.world_id.clone(),
                    dimension: 0,
                    incarnation: column.incarnation(),
                    generation: column.generation(),
                    position: BlockPosition::new(1, 64, 0),
                    layer: 0,
                    previous: BlockRuntimeId(7),
                    current: BlockRuntimeId(8),
                    cause: BlockChangeCause::Command,
                    flags: update_flags::DEFAULT,
                    break_context: None,
                });
        }
        let pending = apply_loaded_change(&world, pending, column.clone()).unwrap_err();
        assert_eq!(column.read().block_at(0, 64, 0), Some(BlockRuntimeId(7)));
        world
            .get_resource_mut::<BlockChangedQueue>()
            .unwrap()
            .drain();
        apply_loaded_change(&world, pending, column).unwrap();
        let facts = world
            .get_resource_mut::<BlockChangedQueue>()
            .unwrap()
            .drain();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].request_id, 42);
        assert_eq!(
            facts[0]
                .break_context
                .as_ref()
                .unwrap()
                .hand
                .item
                .as_deref(),
            Some("minecraft:iron_pickaxe")
        );
    }

    #[test]
    fn reloaded_column_cannot_accept_an_old_break() {
        let (world, column, pending) = guarded_fixture();
        let reloaded = Arc::new(ChunkColumn::new(column.read().clone()));
        apply_loaded_change(&world, pending, reloaded.clone()).unwrap();
        assert_eq!(reloaded.read().block_at(0, 64, 0), Some(BlockRuntimeId(7)));
        assert_eq!(world.get_resource::<BlockChangedQueue>().unwrap().len(), 1);
    }

    #[test]
    fn pending_chunk_load_queue_has_a_hard_capacity() {
        let pending = PendingBlockChangeLoads::default();
        let change = change(1);
        for request_id in 0..MAX_PENDING_BLOCK_CHANGE_LOADS as u64 {
            assert!(try_queue_pending_block_change(
                &pending,
                PendingBlockChangeLoad::new(request_id, change.clone(), None),
            )
            .is_ok());
        }
        assert!(try_queue_pending_block_change(
            &pending,
            PendingBlockChangeLoad::new(MAX_PENDING_BLOCK_CHANGE_LOADS as u64, change, None,),
        )
        .is_err());
        assert_eq!(
            pending.pending.lock().unwrap().len(),
            MAX_PENDING_BLOCK_CHANGE_LOADS
        );
    }

    #[test]
    fn queue_preallocates_so_drain_does_not_cause_regrowth() {
        let mut queue = BlockChangeQueue::default();
        assert_eq!(queue.pending.capacity(), 0);

        // The first push reserves up to the high-water mark (amortizing post-`mem::take` doubling).
        queue.push(change(1));
        assert!(
            queue.pending.capacity() >= BLOCK_CHANGE_QUEUE_PREALLOC,
            "首次 push 未预分配：capacity={}",
            queue.pending.capacity()
        );

        // After drain the contents are cleared and capacity returned (existing behavior); the next push reserves again.
        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        assert!(queue.pending.capacity() == 0);
        queue.push(change(2));
        assert!(queue.pending.capacity() >= BLOCK_CHANGE_QUEUE_PREALLOC);
        assert_eq!(queue.drain().len(), 1);
    }

    #[test]
    fn queue_preserves_fifo_order_with_monotonic_request_ids() {
        let mut queue = BlockChangeQueue::default();
        let first = queue.push(change(1));
        let second = queue.push(change(2));
        assert!(second > first);
        let drained = queue.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].request_id, first);
        assert_eq!(drained[0].change.state, BlockRuntimeId(1));
        assert_eq!(drained[1].request_id, second);
        assert!(queue.is_empty());
        let third = queue.push(change(3));
        assert!(third > second);
    }
}
