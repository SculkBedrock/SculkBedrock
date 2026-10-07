//! Single-writer block mutation pipeline.
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

/// UpdateBlock network flags.
pub mod update_flags {
    pub const NEIGHBORS: u32 = 0b0001;
    pub const NETWORK: u32 = 0b0010;
    pub const NO_GRAPHIC: u32 = 0b0100;
    pub const PRIORITY: u32 = 0b1000;
    pub const DEFAULT: u32 = NEIGHBORS | NETWORK;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockChangeCause {
    Player(EntityId),
    Command,
    Plugin,
    Generation,
    Redstone,
}

#[derive(Clone, Debug)]
pub struct BlockChange {
    pub world_id: MinecraftWorldId,
    pub position: crate::position::BlockPosition,
    pub layer: u8,
    pub state: BlockRuntimeId,
    pub cause: BlockChangeCause,
    pub flags: u32,
}

#[derive(Debug)]
pub struct PendingBlockChange {
    pub request_id: u64,
    pub change: BlockChange,
    pub expectation: Option<BlockExpectation>,
    pub break_context: Option<BlockBreakContext>,
}

#[derive(Clone, Copy, Debug)]
pub struct BlockExpectation {
    pub state: BlockRuntimeId,
    pub incarnation: u128,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct BlockBreakContext {
    pub hand: crate::mining_drops::HandSnapshot,
    pub creative: bool,
}

#[derive(Event, Clone, Debug)]
pub struct BlockChangeResult {
    pub request_id: u64,
    pub applied: bool,
}

/// Preallocated slots on first write to an empty queue.
///
/// Rationale, see [`BlockChangeQueue::drain`].
const BLOCK_CHANGE_QUEUE_PREALLOC: usize = 4096;

#[derive(Resource, Default)]
pub struct BlockChangeQueue {
    pending: Vec<PendingBlockChange>,
    next_request_id: u64,
}

impl BlockChangeQueue {
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

    fn defer(&mut self, pending: PendingBlockChange) {
        self.pending.push(pending);
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Takes and clears all pending changes.
    ///
    /// Must use `mem::take` here: all three callers consume by value
    /// (`for PendingBlockChange { .. } in pending`, where `change` is moved into
    /// `apply_loaded_change` or `PendingBlockChangeLoad`), so in-place iteration
    /// over the internal buffer is not possible.
    ///
    /// The cost is returning `pending` capacity to the allocator. The mitigation is
    /// [`Self::push`] reserving `BLOCK_CHANGE_QUEUE_PREALLOC` on first write to an empty queue,
    /// avoiding re-doubling every tick.
    pub(crate) fn drain(&mut self) -> Vec<PendingBlockChange> {
        std::mem::take(&mut self.pending)
    }
}

const MAX_PENDING_BLOCK_CHANGE_LOADS: usize = 4096;
const MAX_BLOCK_CHANGE_BUSY_AGE: Duration = Duration::from_secs(60);

#[derive(Resource, Default)]
pub struct PendingBlockChangeLoads {
    pending: Mutex<Vec<PendingBlockChangeLoad>>,
}

struct PendingBlockChangeLoad {
    request_id: u64,
    change: BlockChange,
    receiver: Option<oneshot::Receiver<Arc<ChunkLoadOutcome>>>,
    busy_since: Instant,
    retry_at: Instant,
    busy_retries: u8,
    expectation: Option<BlockExpectation>,
    break_context: Option<BlockBreakContext>,
}

impl PendingBlockChangeLoad {
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

    fn defer_after_busy(&mut self, now: Instant) {
        self.busy_retries = self.busy_retries.saturating_add(1);
        self.retry_at = now + block_change_retry_delay(self.busy_retries);
    }
}

#[derive(Clone)]
struct BlockChangeLoadContext {
    provider: WorldChunkProvider,
    dimension: i32,
    min_y: i32,
    max_y: i32,
}

impl BlockChangeLoadContext {
    fn key_for(&self, change: &BlockChange) -> ChunkKey {
        ChunkKey::new(self.dimension, change.position.chunk_position())
    }
}

fn block_change_retry_delay(retries: u8) -> Duration {
    let shift = u32::from(retries.min(4));
    Duration::from_millis((50_u64 << shift).min(800))
}

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

#[derive(Event, Clone, Debug)]
pub struct BlockChanged {
    pub request_id: u64,
    pub world_id: MinecraftWorldId,
    pub dimension: i32,
    pub incarnation: u128,
    pub generation: u64,
    pub position: crate::position::BlockPosition,
    pub layer: u8,
    pub previous: BlockRuntimeId,
    pub current: BlockRuntimeId,
    pub cause: BlockChangeCause,
    pub flags: u32,
    pub break_context: Option<BlockBreakContext>,
}

/// Reliable block facts for high-frequency consumers.
///
/// ECS events remain available for compatibility, but gameplay systems that
/// must not miss a successful write consume this explicit FIFO queue instead.
#[derive(Resource, Default)]
pub struct BlockChangedQueue {
    entries: VecDeque<BlockChanged>,
}

impl BlockChangedQueue {
    pub const MAX_ENTRIES: usize = 4096;

    pub fn has_capacity(&self) -> bool {
        self.entries.len() < Self::MAX_ENTRIES
    }

    pub fn pop_front(&mut self) -> Option<BlockChanged> {
        self.entries.pop_front()
    }

    pub fn push(&mut self, change: BlockChanged) {
        assert!(
            self.has_capacity(),
            "block fact capacity must be reserved before mutation"
        );
        self.entries.push_back(change);
    }

    pub fn drain(&mut self) -> Vec<BlockChanged> {
        self.entries.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn send_change_result(world: &World, request_id: u64, applied: bool) {
    world.send_event(BlockChangeResult {
        request_id,
        applied,
    });
}

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

/// Poll chunk worker results and retry bounded executor backpressure without
/// blocking the tick thread. Busy edits remain pending for up to one minute;
/// after that, a failure result is emitted so inventory reservations can roll back.
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
                Err(ChunkSubmitError::Busy | ChunkSubmitError::TooManyWaiters) => {
                    entry.defer_after_busy(now);
                    remaining.push(entry);
                }
                Err(ChunkSubmitError::Closed) => {
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

/// Apply queued changes without performing synchronous storage I/O.
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
            Err(ChunkSubmitError::Busy | ChunkSubmitError::TooManyWaiters) => {
                log::debug!("block change #{request_id}: executor busy; queueing bounded retry");
                None
            }
            Err(ChunkSubmitError::Closed) => {
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
