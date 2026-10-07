//! Regionized parallel tick domains (see docs/regionized_threading.md).
//!
//! Entities of one `World` are partitioned into regions and ticked in parallel:
//! - [`Region`]`<R>`: region state container (R = region id type defined by
//!   the caller: coordinates, grid, any Copy type);
//! - [`RegionHub`]`<R>`: region registry (Resource);
//! - [`EntityRegion`]`<R>`: membership component (which region thread ticks
//!   this entity);
//! - [`RegionDriver`]`<R>`: fixed-step driver with one dedicated thread per
//!   region. Tick interval and tick function are injected by the caller; the
//!   framework binds no game parameters (the caller picks the TPS).
//!
//! Concurrency model: region threads share one `World` (short-lived interior
//! lock access, docs/ecs_concurrency.md); cross-region moves are atomic
//! [`EntityRegion`] handoffs (tick ownership), entity storage never moves.

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

/// Region running state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionStatus {
    /// Takes part in ticking.
    Running,
    /// Paused (teleporting/saving); skips ticking.
    Paused,
    /// Unloading (removed from the hub after entity migration/flush).
    Unloading,
}

/// Region state container (**without World**: regions share the storage of their World).
#[derive(Clone, Debug)]
pub struct Region<R: Copy + Send + Sync + 'static> {
    pub id: R,
    pub status: RegionStatus,
    /// Ticks completed by this region (incremented by the region thread).
    pub tick: u64,
    /// Entities pending inbound to this region (migration inbound queue; shared
    /// short lock guard, entity storage never moves).
    pub inbound: Vec<MigrationRequest<R>>,
}

impl<R: Copy + Send + Sync + 'static> Region<R> {
    pub fn new(id: R) -> Self {
        Self {
            id,
            status: RegionStatus::Running,
            tick: 0,
            inbound: Vec::new(),
        }
    }
}

/// Region migration request (by value, delivered through the target region inbound queue).
#[derive(Clone, Debug)]
pub struct MigrationRequest<R: Copy + Send + Sync + 'static> {
    pub entity: EntityId,
    pub from: R,
    pub to: R,
}

/// In-migration marker (explicit transitional state): the entity has been
/// detached from its source region but not yet attached to the target.
///
/// Meanwhile the entity has **no `EntityRegion`**, so no region ticks it
/// (region systems filter on `EntityRegion == this region`); `process_inbound`
/// removes this marker when re-attaching to the target region. Used for
/// system filtering and observability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Migrating<R: Copy + Send + Sync + 'static>(pub R);

impl<R: Copy + Send + Sync + 'static> Component for Migrating<R> {
    fn name() -> String {
        format!("Migrating<{}>", std::any::type_name::<R>())
    }
}

/// Region registry (Resource, mounted on the partitioned World).
#[derive(Clone, Debug)]
pub struct RegionHub<R: Copy + Send + Sync + 'static> {
    regions: HashMap<R, Region<R>>,
    pending_migrations: HashMap<EntityId, PendingMigration<R>>,
}

#[derive(Clone, Debug)]
struct PendingMigration<R: Copy + Send + Sync + 'static> {
    from: R,
    to: R,
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
    pub fn spawn_region(&mut self, id: R) -> R {
        self.regions.entry(id).or_insert_with(|| Region::new(id));
        id
    }

    pub fn get(&self, id: &R) -> Option<&Region<R>> {
        self.regions.get(id)
    }

    pub fn get_mut(&mut self, id: &R) -> Option<&mut Region<R>> {
        self.regions.get_mut(id)
    }

    pub fn remove(&mut self, id: &R) -> Option<Region<R>> {
        self.regions.remove(id)
    }

    pub fn count(&self) -> usize {
        self.regions.len()
    }

    /// Iterates Running regions (read-only).
    pub fn running(&self) -> impl Iterator<Item = &Region<R>> + '_ {
        self.regions
            .values()
            .filter(|r| r.status == RegionStatus::Running)
    }

    /// Iterates Running regions (writable).
    pub fn running_mut(&mut self) -> impl Iterator<Item = &mut Region<R>> + '_ {
        self.regions
            .values_mut()
            .filter(|r| r.status == RegionStatus::Running)
    }

    // ===================== Cross-region migration =====================

    /// Starts a migration (call inside the source region thread's tick):
    /// detaches the entity's `EntityRegion` (so no region ticks it anymore) +
    /// attaches the `Migrating(from)` transitional marker, then delivers to
    /// the target region's inbound queue.
    ///
    /// Returns false when the entity does not exist or never belonged to a region (caller handles it).
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

    /// Pushes a migration request into the target region inbound queue.
    /// Returns false when the target does not exist or is not Running.
    pub fn push_inbound(&mut self, to: R, request: MigrationRequest<R>) -> bool {
        if let Some(region) = self.regions.get_mut(&to) {
            if region.status == RegionStatus::Running {
                region.inbound.push(request);
                return true;
            }
        }
        false
    }

    /// Drains the target region inbound queue (call inside the target region
    /// thread's tick): re-attaches migrating entities as `EntityRegion(to)`,
    /// completing the tick-ownership handoff.
    ///
    /// Requests for despawned entities or without a `Migrating` marker (abnormal path) are dropped.
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

    /// Restores migrations the target region has not consumed for a long time,
    /// so entities never stay in `Migrating` forever. Restoration also drops
    /// the stale request from the target queue, so a late consumer cannot move
    /// the entity back to the target region afterwards.
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

/// Membership component (generic): which region thread ticks this entity.
///
/// Migration atomically swaps this component (plus a transitional marker); storage never moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntityRegion<R: Copy + Send + Sync + 'static>(pub R);

impl<R: Copy + Send + Sync + 'static> Component for EntityRegion<R> {
    fn name() -> String {
        format!("EntityRegion<{}>", std::any::type_name::<R>())
    }
}

/// Region thread driver (Resource, generic R = region id type).
///
/// One **dedicated thread** (`std::thread`) per region runs `tick_fn(world,
/// region)` on the caller-provided fixed `tick_interval`. The framework binds
/// no game parameters:
/// - the tick interval (e.g. 20 TPS = 50ms) is passed by the caller, so this
///   stays reusable outside this project;
/// - the tick function is injected by the caller.
/// Concurrency model: region threads share the caller-provided ECS `World`
/// (short-lived interior-lock access).
#[derive(Debug)]
pub struct RegionDriver<R: Copy + Send + Sync + 'static> {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
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
    /// Starts a dedicated thread for the given region: fixed-step
    /// `tick_interval`, running `tick_fn` each step.
    ///
    /// `tick_fn` must be `Fn(&World, R)` (R = region id); when behind, ticks
    /// are dropped rather than caught up.
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

    /// Stops all region threads and joins them (call on server shutdown; the framework never joins automatically).
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }

    pub fn region_count(&self) -> usize {
        self.threads.len()
    }
}

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

/// Region event (by value, from -> to; the only cross-region communication carrier).
#[derive(Clone, Debug)]
pub struct RegionEvent<R, T> {
    pub from: R,
    pub to: R,
    pub payload: T,
}

/// Region event bus (Resource, R = region id, T = payload).
///
/// A region thread `send`s into the target region queue (FIFO); the target
/// region drains it with `take_for` on its tick. **By value**: no
/// cross-region references/locks (docs/regionized_threading.md).
#[derive(Clone, Debug)]
pub struct RegionBus<R, T> {
    queues: HashMap<R, VecDeque<RegionEvent<R, T>>>,
    max_events: usize,
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
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            queues: HashMap::new(),
            max_events: max_events.max(1),
            pending_events: 0,
        }
    }

    /// Sends into the target region queue (FIFO, ordered per same source and target).
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

    /// Takes this region's own events (call at the start of the region thread tick; short lock hold).
    pub fn take_for(&mut self, region: &R) -> Vec<RegionEvent<R, T>> {
        let events: Vec<_> = self
            .queues
            .remove(region)
            .map(|events| events.into_iter().collect())
            .unwrap_or_default();
        self.pending_events = self.pending_events.saturating_sub(events.len());
        events
    }

    /// Total pending events (for scheduler observability).
    pub fn pending(&self) -> usize {
        self.pending_events
    }
}
