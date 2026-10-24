//! 区域化并行 tick 域。
//!
//! 一个 `World` 的实体被划分到各区域并行 tick：
//! - [`Region`]`<R>`：区域状态容器（R = 区域 id 类型）；
//! - [`RegionHub`]`<R>`：区域注册表（Resource）；
//! - [`EntityRegion`]`<R>`：成员组件（哪个区域线程 tick 该实体）；
//! - [`RegionDriver`]`<R>`：固定步长驱动，每个区域一个专用线程，
//!   tick 间隔与 tick 函数由调用方注入。
//!
//! 跨区域移动交接 [`EntityRegion`]（tick 所有权），实体本身不移动。
//!
//! Regionized parallel tick domains: entities of one `World` are partitioned
//! into regions and ticked in parallel.
//! - [`Region`]`<R>`: region state container (R = region id type);
//! - [`RegionHub`]`<R>`: region registry (Resource);
//! - [`EntityRegion`]`<R>`: membership component (which region thread ticks
//!   this entity);
//! - [`RegionDriver`]`<R>`: fixed-step driver with one dedicated thread per
//!   region; the tick interval and tick function are injected by the caller.
//!
//! Cross-region moves hand off the [`EntityRegion`] component (tick
//! ownership); the entity itself does not move.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::marker::PhantomData;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::component::Component;
use crate::entity::EntityId;
use crate::resource::{cached_resource_id_for, Resource, ResourceId};
use crate::world::World;
use sc_log::t_log;

/// 区域运行状态。
///
/// Region running state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionStatus {
    /// 参与 tick。
    ///
    /// Takes part in ticking.
    Running,
    /// 暂停（传送/存档中）；跳过 tick。
    ///
    /// Paused (teleporting/saving); skips ticking.
    Paused,
    /// 卸载中（实体迁移/刷盘后从 hub 移除）。
    ///
    /// Unloading (removed from the hub after entity migration/flush).
    Unloading,
}

/// 区域状态容器（**不含 World**：各区域共享其 World 的存储）。
///
/// Region state container (**without World**: regions share the storage of their World).
#[derive(Clone, Debug)]
pub struct Region<R: Copy + Send + Sync + 'static> {
    /// 区域 id。
    ///
    /// The region id.
    pub id: R,
    /// 运行状态。
    ///
    /// The running status.
    pub status: RegionStatus,
    /// 本区域已执行的 tick 数（由区域线程递增）。
    ///
    /// Ticks completed by this region (incremented by the region thread).
    pub tick: u64,
    /// 待迁入本区域的实体（迁入队列）。
    ///
    /// Entities pending inbound to this region (migration inbound queue).
    pub inbound: Vec<MigrationRequest<R>>,
}

impl<R: Copy + Send + Sync + 'static> Region<R> {
    /// 以指定 id 创建运行中的区域。
    ///
    /// Creates a running region with the given id.
    ///
    /// - `id`: 区域 id
    pub fn new(id: R) -> Self {
        Self {
            id,
            status: RegionStatus::Running,
            tick: 0,
            inbound: Vec::new(),
        }
    }
}

/// 区域迁移请求（按值传递，经目标区域 inbound 队列投递）。
///
/// Region migration request (by value, delivered through the target region inbound queue).
#[derive(Clone, Debug)]
pub struct MigrationRequest<R: Copy + Send + Sync + 'static> {
    /// 被迁移的实体。
    ///
    /// The entity being migrated.
    pub entity: EntityId,
    /// 来源区域。
    ///
    /// The source region.
    pub from: R,
    /// 目标区域。
    ///
    /// The target region.
    pub to: R,
}

/// 迁入中标记（显式过渡态）：实体已从来源区域摘除，但尚未挂到目标区域。
///
/// In-migration marker (explicit transitional state): the entity has been
/// detached from its source region but not yet attached to the target.
///
/// 此期间实体没有 `EntityRegion`，因此没有区域会 tick 它。
///
/// Meanwhile the entity has no `EntityRegion`, so no region ticks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Migrating<R: Copy + Send + Sync + 'static>(pub R);

impl<R: Copy + Send + Sync + 'static> Component for Migrating<R> {
    fn name() -> String {
        format!("Migrating<{}>", std::any::type_name::<R>())
    }
}

/// 区域注册表（Resource，挂载在被分区的 World 上）。
///
/// Region registry (Resource, mounted on the partitioned World).
#[derive(Clone, Debug)]
pub struct RegionHub<R: Copy + Send + Sync + 'static> {
    /// id 到区域的映射。
    ///
    /// Map from id to region.
    regions: HashMap<R, Region<R>>,
    /// 进行中的迁移（实体 -> 迁移信息）。
    ///
    /// In-flight migrations (entity -> migration info).
    pending_migrations: HashMap<EntityId, PendingMigration<R>>,
}

/// 进行中的迁移记录（用于超时恢复）。
///
/// Record of an in-flight migration (used for timeout recovery).
#[derive(Clone, Debug)]
struct PendingMigration<R: Copy + Send + Sync + 'static> {
    /// 来源区域。
    ///
    /// The source region.
    from: R,
    /// 目标区域。
    ///
    /// The target region.
    to: R,
    /// 迁移开始时间。
    ///
    /// When the migration started.
    started_at: Instant,
}

impl<R: Copy + Send + Sync + Eq + Hash + 'static> Default for RegionHub<R> {
    fn default() -> Self {
        Self {
            regions: HashMap::new(),
            pending_migrations: HashMap::new(),
        }
    }
}

// Generic resource: hand-written Resource impl (the derive macro has no generic support).
// Note: keep the layout code in sync with the fields (the derive version generates it).
impl<R: Copy + Send + Sync + Eq + Hash + 'static> Resource for RegionHub<R> {
    fn resource_id() -> ResourceId {
        cached_resource_id_for::<Self>(|| {
            format!(
            "RegionHub{{pending_migrations:HashMap<EntityId,PendingMigration<R>>,regions:HashMap<R,Region<R>>}}<{}>",
            std::any::type_name::<R>()
        )
        })
    }

    fn name() -> String {
        format!("RegionHub<{}>", std::any::type_name::<R>())
    }
}

impl<R: Copy + Send + Sync + Eq + Hash + 'static> RegionHub<R> {
    /// 创建（或获取已存在的）区域并返回其 id。
    ///
    /// Creates (or fetches the existing) region and returns its id.
    ///
    /// - `id`: 区域 id
    pub fn spawn_region(&mut self, id: R) -> R {
        self.regions.entry(id).or_insert_with(|| Region::new(id));
        id
    }

    /// 按 id 获取区域引用；不存在时返回 `None`。
    ///
    /// Returns a reference to the region for `id`; returns `None` if it
    /// does not exist.
    ///
    /// - `id`: 区域 id
    pub fn get(&self, id: &R) -> Option<&Region<R>> {
        self.regions.get(id)
    }

    /// 按 id 获取区域可变引用；不存在时返回 `None`。
    ///
    /// Returns a mutable reference to the region for `id`; returns `None`
    /// if it does not exist.
    ///
    /// - `id`: 区域 id
    pub fn get_mut(&mut self, id: &R) -> Option<&mut Region<R>> {
        self.regions.get_mut(id)
    }

    /// 移除并返回区域；不存在时返回 `None`。
    ///
    /// Removes and returns the region; returns `None` if it does not
    /// exist.
    ///
    /// - `id`: 区域 id
    pub fn remove(&mut self, id: &R) -> Option<Region<R>> {
        self.regions.remove(id)
    }

    /// 区域数量。
    ///
    /// The number of regions.
    pub fn count(&self) -> usize {
        self.regions.len()
    }

    /// 遍历 Running 状态的区域（只读）。
    ///
    /// Iterates Running regions (read-only).
    pub fn running(&self) -> impl Iterator<Item = &Region<R>> + '_ {
        self.regions
            .values()
            .filter(|r| r.status == RegionStatus::Running)
    }

    /// 遍历 Running 状态的区域（可写）。
    ///
    /// Iterates Running regions (writable).
    pub fn running_mut(&mut self) -> impl Iterator<Item = &mut Region<R>> + '_ {
        self.regions
            .values_mut()
            .filter(|r| r.status == RegionStatus::Running)
    }

    // ===================== Cross-region migration =====================

    /// 开始一次迁移（在来源区域线程的 tick 内调用）：摘除实体的
    /// `EntityRegion`（不再有任何区域 tick 它）+ 挂上 `Migrating(from)`
    /// 过渡标记，然后投递到目标区域的 inbound 队列。
    ///
    /// Starts a migration (call inside the source region thread's tick):
    /// detaches the entity's `EntityRegion` (so no region ticks it anymore) +
    /// attaches the `Migrating(from)` transitional marker, then delivers to
    /// the target region's inbound queue.
    ///
    /// 实体不存在或从未属于任何区域时返回 false（由调用方处理）。
    ///
    /// Returns false when the entity does not exist or never belonged to a
    /// region (caller handles it).
    ///
    /// - `world`: 世界
    /// - `entity`: 被迁移实体
    /// - `from`: 来源区域
    /// - `to`: 目标区域
    pub fn begin_migration(&mut self, world: &World, entity: EntityId, from: R, to: R) -> bool {
        let Some(target) = self.regions.get(&to) else {
            return false;
        };
        if target.status != RegionStatus::Running || from == to {
            return false;
        }
        if world
            .get_component::<EntityRegion<R>>(&entity)
            .map(|region| region.0 != from)
            .unwrap_or(true)
        {
            return false;
        }
        if world.remove_component::<EntityRegion<R>>(&entity).is_none() {
            return false;
        }
        world.add_component(&entity, Migrating(from));
        if let Some(target) = self.regions.get_mut(&to) {
            target.inbound.push(MigrationRequest { entity, from, to });
            self.pending_migrations.insert(
                entity,
                PendingMigration {
                    from,
                    to,
                    started_at: Instant::now(),
                },
            );
            true
        } else {
            world.remove_component::<Migrating<R>>(&entity);
            world.add_component(&entity, EntityRegion(from));
            false
        }
    }

    /// 把迁移请求推入目标区域的 inbound 队列；目标不存在或非 Running
    /// 时返回 false。
    ///
    /// Pushes a migration request into the target region inbound queue.
    /// Returns false when the target does not exist or is not Running.
    ///
    /// - `to`: 目标区域
    /// - `request`: 迁移请求
    pub fn push_inbound(&mut self, to: R, request: MigrationRequest<R>) -> bool {
        if let Some(region) = self.regions.get_mut(&to) {
            if region.status == RegionStatus::Running {
                region.inbound.push(request);
                return true;
            }
        }
        false
    }

    /// 排空目标区域的 inbound 队列（在目标区域线程的 tick 内调用）：
    /// 把迁移中的实体重新挂为 `EntityRegion(to)`，完成 tick 所有权交接。
    ///
    /// Drains the target region inbound queue (call inside the target region
    /// thread's tick): re-attaches migrating entities as `EntityRegion(to)`,
    /// completing the tick-ownership handoff.
    ///
    /// 已 despawn 的实体或无 `Migrating` 标记的请求（异常路径）会被丢弃。
    ///
    /// Requests for despawned entities or without a `Migrating` marker
    /// (abnormal path) are dropped.
    ///
    /// - `world`: 世界
    /// - `region`: 目标区域 id
    pub fn process_inbound(&mut self, world: &World, region: &R) {
        let Some(target) = self.regions.get_mut(region) else {
            return;
        };
        if target.status != RegionStatus::Running {
            return;
        }
        let requests = std::mem::take(&mut target.inbound);
        for request in requests {
            if request.to != *region {
                self.pending_migrations.remove(&request.entity);
                continue;
            }
            // Liveness check: a migrating entity must carry the Migrating marker (attached by begin_migration).
            if world
                .get_component::<Migrating<R>>(&request.entity)
                .map(|migrating| migrating.0 != request.from)
                .unwrap_or(true)
            {
                self.pending_migrations.remove(&request.entity);
                continue;
            }
            world.remove_component::<Migrating<R>>(&request.entity);
            world.add_component(&request.entity, EntityRegion(request.to));
            self.pending_migrations.remove(&request.entity);
        }
    }

    /// 恢复目标区域长时间未消费的迁移，使实体不会永远停留在 `Migrating`；
    /// 过期请求同时从目标队列丢弃。
    ///
    /// Restores migrations the target region has not consumed for a long time,
    /// so entities never stay in `Migrating` forever; the stale requests are
    /// also dropped from the target queues.
    ///
    /// 返回恢复的迁移数量。
    ///
    /// Returns the number of recovered migrations.
    ///
    /// - `world`: 世界
    /// - `timeout`: 迁移超时时间
    pub fn recover_expired_migrations(&mut self, world: &World, timeout: Duration) -> usize {
        let now = Instant::now();
        let expired = self
            .pending_migrations
            .iter()
            .filter(|(_, migration)| now.duration_since(migration.started_at) >= timeout)
            .map(|(entity, migration)| (*entity, migration.from, migration.to))
            .collect::<Vec<_>>();

        let mut recovered = 0;
        for (entity, from, _to) in expired {
            self.pending_migrations.remove(&entity);
            for region in self.regions.values_mut() {
                region.inbound.retain(|request| request.entity != entity);
            }
            if world.get_component::<Migrating<R>>(&entity).is_some() {
                world.remove_component::<Migrating<R>>(&entity);
                world.add_component(&entity, EntityRegion(from));
                recovered += 1;
            }
            log::warn!(
                "{}",
                t_log!(
                    "console.region.migration_timeout",
                    region = std::any::type_name::<R>(),
                    entity = format!("{entity:?}")
                )
            );
        }
        recovered
    }
}

/// 成员组件（泛型）：哪个区域线程 tick 该实体。
///
/// Membership component (generic): which region thread ticks this entity.
///
/// 迁移时本组件被原子交接。
///
/// Migration atomically swaps this component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntityRegion<R: Copy + Send + Sync + 'static>(pub R);

impl<R: Copy + Send + Sync + 'static> Component for EntityRegion<R> {
    fn name() -> String {
        format!("EntityRegion<{}>", std::any::type_name::<R>())
    }
}

/// 区域线程驱动（Resource，泛型 R = 区域 id 类型）。
///
/// 每个区域一个专用线程（`std::thread`），按调用方给定的 `tick_interval`
/// 运行注入的 `tick_fn(world, region)`。
///
/// Region thread driver (Resource, R = region id type). One dedicated thread
/// (`std::thread`) per region runs the caller-injected `tick_fn(world,
/// region)` on the given `tick_interval`.
#[derive(Debug)]
pub struct RegionDriver<R: Copy + Send + Sync + 'static> {
    /// 停止标志（置位后线程退出）。
    ///
    /// Stop flag (threads exit once set).
    stop: Arc<AtomicBool>,
    /// 区域线程句柄。
    ///
    /// Region thread handles.
    threads: Vec<std::thread::JoinHandle<()>>,
    /// R 类型占位。
    ///
    /// Phantom data for the R type.
    _marker: PhantomData<R>,
}

impl<R: Copy + Send + Sync + 'static> Default for RegionDriver<R> {
    fn default() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            threads: Vec::new(),
            _marker: PhantomData,
        }
    }
}

impl<R: Copy + Send + Sync + 'static> Resource for RegionDriver<R> {
    fn resource_id() -> ResourceId {
        cached_resource_id_for::<Self>(|| {
            format!(
            "RegionDriver{{_marker:PhantomData<R>,stop:Arc<AtomicBool>,threads:Vec<std::thread::JoinHandle<()>>}}<{}>",
            std::any::type_name::<R>()
        )
        })
    }

    fn name() -> String {
        format!("RegionDriver<{}>", std::any::type_name::<R>())
    }
}

impl<R: Copy + Send + Sync + 'static> RegionDriver<R> {
    /// 为指定区域启动一个专用线程：固定步长 `tick_interval`，每步运行
    /// 一次 `tick_fn`。
    ///
    /// Starts a dedicated thread for the given region: fixed-step
    /// `tick_interval`, running `tick_fn` each step.
    ///
    /// `tick_fn` 必须是 `Fn(&World, R)`（R = 区域 id）；落后时丢弃 tick
    /// 而不追赶。
    ///
    /// `tick_fn` must be `Fn(&World, R)` (R = region id); when behind, ticks
    /// are dropped rather than caught up.
    ///
    /// - `world`: 共享的世界
    /// - `region`: 区域 id
    /// - `tick_interval`: tick 间隔
    /// - `tick_fn`: 每步执行的函数
    pub fn start<F>(&mut self, world: &World, region: R, tick_interval: Duration, tick_fn: F)
    where
        F: Fn(&World, R) + Send + Sync + 'static,
    {
        self.stop.store(false, Ordering::Release);
        let world = world.clone();
        let stop = self.stop.clone();
        self.threads.push(std::thread::spawn(move || {
            region_loop(world, region, tick_interval, stop, tick_fn);
        }));
    }

    /// 停止并 join 所有区域线程（需显式调用，如服务器关闭时）。
    ///
    /// Stops all region threads and joins them (must be called explicitly,
    /// e.g. on server shutdown).
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }

    /// 已启动的区域线程数量。
    ///
    /// The number of started region threads.
    pub fn region_count(&self) -> usize {
        self.threads.len()
    }
}

/// 区域线程主循环：固定步长 `tick_interval`，每步运行注入的 `tick_fn`。
///
/// Region thread main loop: fixed-step `tick_interval`, running the injected `tick_fn` each step.
fn region_loop<R, F>(
    world: World,
    region: R,
    tick_interval: Duration,
    stop: Arc<AtomicBool>,
    tick_fn: F,
) where
    R: Copy + Send + 'static,
    F: Fn(&World, R) + Send + 'static,
{
    let mut next = Instant::now();
    while !stop.load(Ordering::Acquire) {
        next += tick_interval;
        if catch_unwind(AssertUnwindSafe(|| tick_fn(&world, region))).is_err() {
            log::error!("{}", t_log!("console.region.thread_panic"));
            std::thread::sleep(Duration::from_millis(100));
            next = Instant::now();
        }
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            // Behind schedule: skip catch-up frames (drop ticks to keep the step stable).
            next = now;
        }
    }
}

/// 区域事件（按值，from -> to）。
///
/// Region event (by value, from -> to).
#[derive(Clone, Debug)]
pub struct RegionEvent<R, T> {
    /// 来源区域。
    ///
    /// The source region.
    pub from: R,
    /// 目标区域。
    ///
    /// The target region.
    pub to: R,
    /// 事件负载。
    ///
    /// The event payload.
    pub payload: T,
}

/// 区域事件总线（Resource，R = 区域 id，T = 负载）。
/// 区域线程 `send` 进目标区域队列（FIFO）；目标区域在 tick 时用
/// `take_for` 取走。事件按值传递，无跨区域引用/锁。
///
/// Region event bus (Resource, R = region id, T = payload). A region thread
/// `send`s into the target region queue (FIFO); the target region drains it
/// with `take_for` on its tick. Events pass by value, with no cross-region
/// references/locks.
#[derive(Clone, Debug)]
pub struct RegionBus<R, T> {
    /// 区域 -> 事件队列。
    ///
    /// Region -> event queue.
    queues: HashMap<R, VecDeque<RegionEvent<R, T>>>,
    /// 队列事件数上限（超出时淘汰最旧）。
    ///
    /// Maximum number of queued events (the oldest is dropped when
    /// exceeded).
    max_events: usize,
    /// 当前待处理事件总数。
    ///
    /// The total number of currently pending events.
    pending_events: usize,
}

impl<R: Hash + Eq + Clone + Send + Sync + 'static, T: Send + Sync + 'static> Default
    for RegionBus<R, T>
{
    fn default() -> Self {
        Self {
            queues: HashMap::new(),
            max_events: 8192,
            pending_events: 0,
        }
    }
}

impl<R: Hash + Eq + Clone + Send + Sync + 'static, T: Send + Sync + 'static> Resource
    for RegionBus<R, T>
{
    fn resource_id() -> ResourceId {
        cached_resource_id_for::<Self>(|| {
            format!(
            "RegionBus{{max_events:usize,pending_events:usize,queues:HashMap<R,VecDeque<RegionEvent<R,T>>>}}<{},{}>",
            std::any::type_name::<R>(),
            std::any::type_name::<T>()
        )
        })
    }

    fn name() -> String {
        format!(
            "RegionBus<{}, {}>",
            std::any::type_name::<R>(),
            std::any::type_name::<T>()
        )
    }
}

impl<R: Hash + Eq + Clone + Send + Sync + 'static, T: Send + Sync + 'static> RegionBus<R, T> {
    /// 以指定容量上限创建事件总线（最小为 1）。
    ///
    /// Creates an event bus with the given capacity limit (at least 1).
    ///
    /// - `max_events`: 事件数上限
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            queues: HashMap::new(),
            max_events: max_events.max(1),
            pending_events: 0,
        }
    }

    /// 发送到目标区域队列（FIFO，同源同目标间保序）；超出上限时淘汰
    /// 最旧事件。
    ///
    /// Sends into the target region queue (FIFO, ordered per same source and target); the oldest
    /// event is dropped when the limit is exceeded.
    ///
    /// - `from`: 来源区域
    /// - `to`: 目标区域
    /// - `payload`: 事件负载
    pub fn send(&mut self, from: R, to: R, payload: T) {
        if self.pending_events >= self.max_events {
            if let Some(queue) = self.queues.values_mut().find(|queue| !queue.is_empty()) {
                queue.pop_front();
                self.pending_events -= 1;
            }
        }
        self.queues
            .entry(to.clone())
            .or_insert_with(VecDeque::new)
            .push_back(RegionEvent { from, to, payload });
        self.pending_events += 1;
    }

    /// 取走本区域的事件（在区域线程 tick 内调用）。
    ///
    /// Takes this region's own events (call inside the region thread tick).
    ///
    /// - `region`: 区域 id
    pub fn take_for(&mut self, region: &R) -> Vec<RegionEvent<R, T>> {
        let events: Vec<_> = self
            .queues
            .remove(region)
            .map(|events| events.into_iter().collect())
            .unwrap_or_default();
        self.pending_events = self.pending_events.saturating_sub(events.len());
        events
    }


    /// 待处理事件总数。
    ///
    /// Total pending events.
    pub fn pending(&self) -> usize {
        self.pending_events
    }
}

// ---------------------------------------------------------------------------
// BoundedRegionScheduler: the bounded region scheduler.
// ---------------------------------------------------------------------------

/// 单区域不可重入的执行状态常量。
///
/// Execution states for single-region non-reentrancy.
pub const REGION_STATE_IDLE: u8 = 0;
pub const REGION_STATE_SCHEDULED: u8 = 1;
pub const REGION_STATE_RUNNING: u8 = 2;

/// 有界调度器跟踪的已注册区域任务。
///
/// A registered region task tracked by the bounded scheduler.
pub struct BoundedRegionTask<R: Copy + Send + Sync + 'static> {
    /// 区域 id。
    ///
    /// The region id.
    pub region: R,
    /// 原子执行状态（IDLE/SCHEDULED/RUNNING）。
    ///
    /// Atomic execution state (IDLE/SCHEDULED/RUNNING).
    pub state: std::sync::atomic::AtomicU8,
    /// tick 间隔。
    ///
    /// The tick interval.
    pub tick_interval: parking_lot::Mutex<Duration>,
    /// 下一次 tick 时间。
    ///
    /// The next tick time.
    pub next_tick: parking_lot::Mutex<Instant>,
}

/// 有界工作线程池调度器：用固定数量的工作线程调度活跃区域；
/// 同一区域不会并发重入执行（经原子状态转换 IDLE -> SCHEDULED -> RUNNING）。
///
/// Bounded worker-pool scheduler for active regions; one region never executes
/// concurrently with itself (enforced via atomic state transitions
/// IDLE -> SCHEDULED -> RUNNING).
pub struct BoundedRegionScheduler<R: Copy + Eq + Hash + Send + Sync + 'static> {
    /// 工作线程数。
    ///
    /// The number of worker threads.
    worker_count: usize,
    /// 停止标志。
    ///
    /// The stop flag.
    stop: Arc<AtomicBool>,
    /// 工作线程句柄。
    ///
    /// Worker thread handles.
    workers: Vec<std::thread::JoinHandle<()>>,
    /// 区域 id -> 任务。
    ///
    /// Region id -> task.
    tasks: Arc<parking_lot::RwLock<HashMap<R, Arc<BoundedRegionTask<R>>>>>,
    /// 待运行区域队列（锁 + 条件变量）。
    ///
    /// The queue of regions to run (lock + condvar).
    queue: Arc<(parking_lot::Mutex<VecDeque<R>>, parking_lot::Condvar)>,
}

impl<R: Copy + Eq + Hash + Send + Sync + 'static> Resource for BoundedRegionScheduler<R> {
    fn resource_id() -> ResourceId {
        cached_resource_id_for::<Self>(|| {
            format!("BoundedRegionScheduler<{}>", std::any::type_name::<R>())
        })
    }

    fn name() -> String {
        format!("BoundedRegionScheduler<{}>", std::any::type_name::<R>())
    }
}

impl<R: Copy + Eq + Hash + Send + Sync + 'static> BoundedRegionScheduler<R> {
    /// 创建调度器并启动工作线程池（线程数限制在 1..=64）。
    ///
    /// Creates the scheduler and starts the worker pool (clamped to
    /// 1..=64 threads).
    ///
    /// - `world`: 共享的世界
    /// - `worker_count`: 请求的工作线程数
    /// - `tick_fn`: 每个区域 tick 执行的函数
    pub fn new<F>(world: &World, worker_count: usize, tick_fn: F) -> Self
    where
        F: Fn(&World, R) + Send + Sync + 'static,
    {
        let worker_count = worker_count.clamp(1, 64);
        let stop = Arc::new(AtomicBool::new(false));
        let tasks = Arc::new(parking_lot::RwLock::new(HashMap::new()));
        let queue = Arc::new((parking_lot::Mutex::new(VecDeque::new()), parking_lot::Condvar::new()));
        let tick_fn = Arc::new(tick_fn);

        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let world = world.clone();
            let stop = stop.clone();
            let tasks = tasks.clone();
            let queue = queue.clone();
            let tick_fn = tick_fn.clone();

            workers.push(std::thread::spawn(move || {
                scheduler_worker_loop(world, stop, tasks, queue, tick_fn);
            }));
        }

        Self {
            worker_count,
            stop,
            workers,
            tasks,
            queue,
        }
    }

    /// 向调度器注册区域并排队其首次 tick。
    ///
    /// Registers a region with the scheduler and queues its first tick.
    ///
    /// - `region`: 区域 id
    /// - `tick_interval`: tick 间隔
    pub fn schedule_region(&self, region: R, tick_interval: Duration) {
        let task = {
            let mut tasks = self.tasks.write();
            tasks.entry(region).or_insert_with(|| {
                Arc::new(BoundedRegionTask {
                    region,
                    state: std::sync::atomic::AtomicU8::new(REGION_STATE_IDLE),
                    tick_interval: parking_lot::Mutex::new(tick_interval),
                    next_tick: parking_lot::Mutex::new(Instant::now()),
                })
            }).clone()
        };
        // Transition IDLE -> SCHEDULED and enqueue
        if task.state.compare_exchange(
            REGION_STATE_IDLE,
            REGION_STATE_SCHEDULED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_ok() {
            let (lock, cvar) = &*self.queue;
            lock.lock().push_back(region);
            cvar.notify_one();
        }
    }

    /// 立即触发一个区域的 tick（如收到 inbound 邮箱消息时）。
    ///
    /// Immediately triggers a tick for a region (e.g. upon inbound mailbox message).
    ///
    /// - `region`: 区域 id
    pub fn trigger_tick(&self, region: R) {
        let task = {
            let tasks = self.tasks.read();
            tasks.get(&region).cloned()
        };
        if let Some(task) = task {
            if task.state.compare_exchange(
                REGION_STATE_IDLE,
                REGION_STATE_SCHEDULED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                let (lock, cvar) = &*self.queue;
                lock.lock().push_back(region);
                cvar.notify_one();
            }
        }
    }

    /// Stops all workers and joins worker threads.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        let (_, cvar) = &*self.queue;
        cvar.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }

    pub fn region_count(&self) -> usize {
        self.tasks.read().len()
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }
}

fn scheduler_worker_loop<R, F>(
    world: World,
    stop: Arc<AtomicBool>,
    tasks: Arc<parking_lot::RwLock<HashMap<R, Arc<BoundedRegionTask<R>>>>>,
    queue: Arc<(parking_lot::Mutex<VecDeque<R>>, parking_lot::Condvar)>,
    tick_fn: Arc<F>,
) where
    R: Copy + Eq + Hash + Send + Sync + 'static,
    F: Fn(&World, R) + Send + Sync + 'static,
{
    let (lock, cvar) = &*queue;
    while !stop.load(Ordering::Acquire) {
        let region = {
            let mut q = lock.lock();
            while q.is_empty() && !stop.load(Ordering::Acquire) {
                let wait_res = cvar.wait_for(&mut q, Duration::from_millis(10));
                if wait_res.timed_out() {
                    break;
                }
            }
            if stop.load(Ordering::Acquire) {
                break;
            }
            if q.is_empty() {
                let now = Instant::now();
                let registered = tasks.read();
                for task in registered.values() {
                    if *task.next_tick.lock() <= now {
                        if task.state.compare_exchange(
                            REGION_STATE_IDLE,
                            REGION_STATE_SCHEDULED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ).is_ok() {
                            q.push_back(task.region);
                        }
                    }
                }
            }
            q.pop_front()
        };

        let Some(region) = region else {
            continue;
        };

        let task = {
            let read = tasks.read();
            read.get(&region).cloned()
        };

        let Some(task) = task else {
            continue;
        };

        // Transition SCHEDULED -> RUNNING
        if task.state.compare_exchange(
            REGION_STATE_SCHEDULED,
            REGION_STATE_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_err() {
            continue;
        }

        let _ = catch_unwind(AssertUnwindSafe(|| {
            tick_fn(&world, region);
        }));

        let interval = *task.tick_interval.lock();
        let next = Instant::now() + interval;
        *task.next_tick.lock() = next;

        // Transition RUNNING -> IDLE
        task.state.store(REGION_STATE_IDLE, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn bounded_scheduler_non_reentrancy_and_concurrency() {
        let world = World::new();
        // Counter per region to detect concurrent execution
        static RUNNING: [AtomicUsize; 4] = [
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        ];
        static TOTAL_TICKS: AtomicUsize = AtomicUsize::new(0);

        let mut scheduler = BoundedRegionScheduler::new(&world, 2, |_w: &World, r: usize| {
            let prior = RUNNING[r].fetch_add(1, Ordering::SeqCst);
            assert_eq!(prior, 0, "region {r} reentered concurrently!");
            std::thread::sleep(Duration::from_millis(5));
            RUNNING[r].fetch_sub(1, Ordering::SeqCst);
            TOTAL_TICKS.fetch_add(1, Ordering::Relaxed);
        });

        for i in 0..4 {
            scheduler.schedule_region(i, Duration::from_millis(10));
        }

        // Trigger ticks rapidly to test non-reentrancy
        for _ in 0..10 {
            for i in 0..4 {
                scheduler.trigger_tick(i);
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        std::thread::sleep(Duration::from_millis(50));
        assert!(TOTAL_TICKS.load(Ordering::Relaxed) > 0);
        scheduler.shutdown();
    }
}

