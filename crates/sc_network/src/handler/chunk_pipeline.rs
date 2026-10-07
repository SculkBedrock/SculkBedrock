//! Chunk send pipeline.
//!
//! Sending uses a subscription epoch plus a per-player send lease: after a
//! reorder/teleport, stale async tasks may exit safely, but results from an
//! never runs two chunk-send batches concurrently.

use crate::chunk_encoder::{
    ChunkEncodeError, ChunkEncodeExecutor, ChunkEncodeSubmitError, ChunkEncodeWaitError,
};
use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::entity::EntityId;
use sc_ecs::params::resource::{Res, ResMut};
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_entity::motion::Transform;
use sc_log::t_log;
use sc_world::chunk::ChunkPosition;
use sc_world::chunk_executor::{ChunkSubmitError, WorldChunkExecutor};
use sc_world::chunk_view::{
    AdmittedChunkBaseline, CachedChunk, ChunkCacheKey, ChunkSendSettings, ChunkView,
    LevelChunkCache, MAX_ADMITTED_CHUNK_BASELINES,
};
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use sc_world::storage::{ChunkColumn, ChunkKey, WorldChunkProvider};
use sha2::Digest;
use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::player_connection::{
    PacketSendOutcome, PlayerConnection, PlayerConnectionStatus, TrySendPacketError,
};
use crate::protocol::server::chunk::{LevelChunk, NetworkChunkPublisherUpdate};
use crate::utils::ConnectionThreadManager;

static CHUNK_WIRE_DUMPED: AtomicBool = AtomicBool::new(false);
static CHUNK_WIRE_DUMP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const EQUIPMENT_CONTAINER_SYNC_CHUNKS: u32 = 7;
const DEFAULT_ACTIVE_CHUNK_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

/// Global admission budget for the per-connection `Vec<u8>` copy constructed
/// for a LevelChunk packet. Cache-owned payloads are accounted separately by
/// `LevelChunkCache`; this estimate stays reserved through outbound command
/// admission, then releases when the send loop transfers ownership to SendQueue.
#[derive(Resource, Clone, Debug)]
pub(crate) struct ChunkPayloadSendBudget {
    active_bytes: Arc<AtomicUsize>,
    max_bytes: usize,
}

impl Default for ChunkPayloadSendBudget {
    fn default() -> Self {
        Self::with_limit(DEFAULT_ACTIVE_CHUNK_PAYLOAD_BYTES)
    }
}

impl ChunkPayloadSendBudget {
    fn with_limit(max_bytes: usize) -> Self {
        Self {
            active_bytes: Arc::new(AtomicUsize::new(0)),
            max_bytes,
        }
    }

    fn try_reserve(&self, bytes: usize) -> Option<ChunkPayloadSendPermit> {
        let mut current = self.active_bytes.load(Ordering::Acquire);
        loop {
            let next = current.saturating_add(bytes);
            // Permit one oversize payload when the budget is otherwise empty;
            // otherwise a single unusually large but valid chunk would never
            // be deliverable. It remains an explicitly isolated oversize case.
            if next > self.max_bytes && current != 0 {
                return None;
            }
            match self.active_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(ChunkPayloadSendPermit {
                        active_bytes: Arc::clone(&self.active_bytes),
                        bytes,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    pub fn active_bytes(&self) -> usize {
        self.active_bytes.load(Ordering::Acquire)
    }
}

struct ChunkPayloadSendPermit {
    active_bytes: Arc<AtomicUsize>,
    bytes: usize,
}

impl Drop for ChunkPayloadSendPermit {
    fn drop(&mut self) {
        self.active_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct PublisherTicketGuard {
    view: Arc<ChunkView>,
    ticket: (u64, u64),
    armed: bool,
}

impl PublisherTicketGuard {
    fn new(view: Arc<ChunkView>, ticket: (u64, u64)) -> Self {
        Self {
            view,
            ticket,
            armed: true,
        }
    }

    fn complete(&mut self, queued: bool) {
        self.view.complete_publisher_ticket(self.ticket, queued);
        self.armed = false;
    }
}

impl Drop for PublisherTicketGuard {
    fn drop(&mut self) {
        if self.armed {
            self.view.complete_publisher_ticket(self.ticket, false);
        }
    }
}

/// Write one complete v2168 LevelChunk payload in the same line-oriented format
/// as the reference diagnostic dump. Only the first packet is persisted so
/// a normal chunk-radius login does not generate hundreds of megabytes of logs.
fn dump_first_chunk_wire(
    chunk_x: i32,
    chunk_z: i32,
    dimension: i32,
    sub_chunks: u32,
    payload: &[u8],
) {
    if CHUNK_WIRE_DUMPED.load(Ordering::Acquire) {
        return;
    }

    let lock = CHUNK_WIRE_DUMP_LOCK.get_or_init(|| Mutex::new(()));
    let Ok(_guard) = lock.lock() else {
        log::debug!("[chunk-wire-dump] mutex poisoned; skipping payload dump");
        return;
    };
    if CHUNK_WIRE_DUMPED.load(Ordering::Relaxed) {
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let mut digest = sha2::Sha256::new();
    digest.update(payload);
    let digest = digest.finalize();
    let mut sha256 = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut sha256, "{byte:02x}");
    }

    let mut hex = String::with_capacity(payload.len() * 2);
    for byte in payload {
        let _ = write!(&mut hex, "{byte:02x}");
    }

    let line = format!(
        "timestamp={timestamp} stage=level_chunk_payload packet=LevelChunkPacket \
         chunkX={chunk_x} chunkZ={chunk_z} dimension={dimension} \
         subChunks={sub_chunks} payloadLength={} sha256={sha256} hex={hex}\n",
        payload.len()
    );

    let result = (|| {
        let path = std::path::Path::new("diagnostics")
            .join("sc-2168")
            .join("logs")
            .join("chunk-wire-dump.log");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        file.write_all(line.as_bytes())?;
        file.flush()
    })();

    match result {
        Ok(()) => {
            CHUNK_WIRE_DUMPED.store(true, Ordering::Release);
            log::debug!(
                "[chunk-wire-dump] wrote first LevelChunk payload: {}B sha256={sha256}",
                payload.len()
            );
        }
        Err(error) => {
            log::debug!("[chunk-wire-dump] failed to write payload dump: {error}");
        }
    }
}

enum PayloadPublishError {
    CacheUnavailable,
    StaleGeneration,
}

/// Publish a worker-produced result under a short cache guard. The source
/// column read guard stabilizes content while the generation is checked and
/// the immutable payload is inserted; serialization has already completed in
/// the bounded encoder pool.
fn publish_encoded_payload(
    world: &World,
    cache_key: &ChunkCacheKey,
    column: &ChunkColumn,
    candidate: Arc<CachedChunk>,
) -> Result<CachedChunk, PayloadPublishError> {
    let chunk = column.read();
    if !candidate.matches_column(column, candidate.wire_profile) {
        return Err(PayloadPublishError::StaleGeneration);
    }
    let Some(mut cache) = world.get_resource_mut::<LevelChunkCache>() else {
        return Err(PayloadPublishError::CacheUnavailable);
    };
    if let Some(existing) = cache
        .get(cache_key)
        .filter(|entry| entry.matches_column(column, candidate.wire_profile))
        .cloned()
    {
        drop(cache);
        drop(chunk);
        return Ok(existing);
    }
    if !candidate.matches_column(column, candidate.wire_profile) {
        drop(cache);
        drop(chunk);
        return Err(PayloadPublishError::StaleGeneration);
    }

    cache.insert(cache_key.clone(), candidate.as_ref().clone());
    let stable = candidate.matches_column(column, candidate.wire_profile);
    if !stable
        && cache.get(cache_key).is_some_and(|entry| {
            entry.incarnation == candidate.incarnation
                && entry.generation == candidate.generation
                && entry.wire_profile == candidate.wire_profile
        })
    {
        cache.invalidate(cache_key);
    }
    drop(cache);
    drop(chunk);
    if !stable {
        return Err(PayloadPublishError::StaleGeneration);
    }
    Ok(candidate.as_ref().clone())
}

/// Global pipeline tick, incremented by `order_chunks` every tick.
#[derive(Resource, Clone, Debug, Default)]
pub struct PipelineTick(pub u64);

/// Rebuilds each player subscription queue by position, plus unload grace period and publisher update.
pub(crate) fn order_chunks(
    world: World,
    settings: Res<ChunkSendSettings>,
    mut tick: ResMut<PipelineTick>,
) {
    tick.0 += 1;
    let now = tick.0;
    let grace = settings.unload_grace_ticks;
    let reorder = settings.reorder_interval;
    let mut cache_invalidations = Vec::new();

    for entity in world.entities_with_component::<ChunkView>() {
        let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
            continue;
        };
        if !connection.get_status().can_send_chunks() {
            continue;
        }
        let Some(view) = world.get_component::<ChunkView>(&entity) else {
            continue;
        };
        let Some(center) = player_center(&world, &entity) else {
            continue;
        };
        let view_world_id = { view.read().world_id.clone() };

        let (expired, publisher_ticket) = {
            let mut data = view.inner.write();
            if data.next_order_run > 0 && data.center == center {
                data.next_order_run -= 1;
                continue;
            }

            let center_changed = data.update_center(center);
            if center_changed {
                // Ordinary movement only bumps the view revision; in-flight load/send
                // work on overlapping view distance under the same hard context stays valid.
                log::debug!(
                    "[chunk] player view center moved -> ({},{}) radius={} revision={}",
                    center.x,
                    center.z,
                    data.radius,
                    data.view_revision
                );
            }
            let radius = data.radius.max(0);
            let dimension = data.dimension;

            let mut new_queue: BTreeSet<(u64, ChunkKey)> = BTreeSet::new();
            let mut in_radius: HashSet<ChunkKey> =
                HashSet::with_capacity(((radius * 2 + 1) as usize).pow(2));
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let key =
                        ChunkKey::new(dimension, ChunkPosition::new(center.x + x, center.z + z));
                    in_radius.insert(key);
                    if (!data.used_chunks.contains(&key) || data.refresh_required.contains(&key))
                        && !data.in_flight.contains_key(&key)
                    {
                        new_queue.insert(((x * x + z * z) as u64, key));
                    }
                    data.unloading.remove(&key);
                }
            }

            let mut far = Vec::new();
            for key in data.used_chunks.iter() {
                if !in_radius.contains(key) {
                    far.push(*key);
                }
            }
            for key in far {
                data.unloading.entry(key).or_insert(now + grace as u64);
            }

            let mut expired = Vec::new();
            for (key, expire) in data.unloading.iter() {
                if *expire <= now {
                    expired.push(*key);
                }
            }
            for key in &expired {
                data.unloading.remove(key);
                data.used_chunks.remove(key);
                data.delivery_ledger.remove(key);
                data.refresh_required.remove(key);
            }

            let mut refresh = std::mem::take(&mut data.refresh_required);
            refresh.retain(|key| in_radius.contains(key) || data.delivery_ledger.contains_key(key));
            data.refresh_required = refresh;
            data.desired_chunks = in_radius;

            data.load_queue = new_queue;
            data.next_order_run = reorder;
            // Publisher updates are tied to a concrete view ticket and retried
            // only after the previous async attempt completes.
            let publisher_ticket = data.take_publisher_ticket();
            (expired, publisher_ticket)
        };

        // Expiry unloads only invalidate the network payload. Dirty columns are
        // flushed by the bounded sc_world write-back system, never by awaiting LevelDB here.
        if !expired.is_empty() {
            cache_invalidations.extend(
                expired
                    .iter()
                    .map(|key| ChunkCacheKey::new(view_world_id.clone(), *key)),
            );
        }

        if let Some(ticket) = publisher_ticket {
            let task_world = world.clone();
            let task_view = view.clone();
            let manager_world = world.clone();
            let handle = SCECSAsync::runtime().spawn(async move {
                let mut ticket_guard = PublisherTicketGuard::new(task_view.clone(), ticket);
                let Some(connection) = task_world.get_component::<PlayerConnection>(&entity) else {
                    return;
                };
                if connection.connection.is_closed().await
                    || !connection.get_status().can_send_chunks()
                    || !task_view.publisher_ticket_is_current(ticket)
                {
                    return;
                }
                let radius = (task_view.read().radius.max(1) * 16) as u32;
                let position = task_world
                    .get_component::<Transform>(&entity)
                    .map(|tf| tf.inner.read().position)
                    .unwrap_or_default();
                let result = connection
                    .send_packet_with_checked_outcome(
                        NetworkChunkPublisherUpdate {
                            x: position.x.floor() as i32,
                            y: position.y.floor() as i32,
                            z: position.z.floor() as i32,
                            radius,
                            saved_chunks: Vec::new(),
                        },
                        true,
                        |plain, permit, trace| {
                            let mut data = task_view.write();
                            if data.epoch != ticket.0
                                || data.view_revision != ticket.1
                                || data.publisher_in_flight != Some(ticket)
                            {
                                return Ok(PacketSendOutcome::StaleContext);
                            }
                            connection.admit_prepared(plain, true, permit, trace)?;
                            data.publisher_pending = false;
                            data.publisher_in_flight = None;
                            Ok(PacketSendOutcome::Queued)
                        },
                    )
                    .await;
                let queued = matches!(result, Ok(PacketSendOutcome::Queued));
                match &result {
                    Err(error) => {
                        log::warn!("{}", t_log!("console.chunk.enqueue_fail", error = error))
                    }
                    Ok(outcome) if !queued => {
                        log::debug!("[chunk] publisher update not queued: {outcome:?}")
                    }
                    _ => {}
                }
                ticket_guard.complete(queued);
            });
            if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
                manager.insert(entity, handle);
            }
        }
    }

    // Do not hold the global payload-cache write guard while scanning players,
    // writing dirty columns, or spawning publisher tasks.
    if !cache_invalidations.is_empty() {
        if let Some(mut cache) = world.get_resource_mut::<LevelChunkCache>() {
            for key in cache_invalidations {
                cache.invalidate(&key);
            }
        }
    }
}

/// Sends each player load_queue within a per-tick per-player budget.
pub(crate) fn send_next_chunk(world: World, settings: Res<ChunkSendSettings>) {
    let chunks_per_tick = settings.chunks_per_tick;
    let spawn_threshold = settings.spawn_threshold;

    for entity in world.entities_with_component::<ChunkView>() {
        let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
            continue;
        };
        if !connection.get_status().can_send_chunks() {
            continue;
        }
        let Some(view) = world.get_component::<ChunkView>(&entity) else {
            continue;
        };
        let (epoch, pending) = {
            let mut data = view.inner.write();
            if data.send_lease.is_some() {
                continue;
            }
            let epoch = data.epoch;
            let mut out = Vec::new();
            while out.len() < chunks_per_tick as usize {
                let Some((_, key)) = data.load_queue.pop_first() else {
                    break;
                };
                if (data.used_chunks.contains(&key) && !data.refresh_required.contains(&key))
                    || data.in_flight.contains_key(&key)
                {
                    continue;
                }
                data.in_flight.insert(key, epoch);
                out.push(key);
            }
            if out.is_empty() {
                continue;
            }
            data.send_lease = Some(epoch);
            (epoch, out)
        };

        let task_world = world.clone();
        let task_view = view.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            send_chunks_batch(
                task_world,
                task_view,
                entity,
                epoch,
                pending,
                spawn_threshold,
                chunks_per_tick,
            )
            .await;
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}

/// Releases in-flight markers for the given epoch without clearing newer leases.
fn release_batch(view: &ChunkView, epoch: u64, pending: &[ChunkKey]) {
    let mut data = view.inner.write();
    for key in pending {
        if data.in_flight.get(key).copied() == Some(epoch) {
            data.in_flight.remove(key);
        }
    }
    if data.send_lease == Some(epoch) {
        data.send_lease = None;
    }
}

fn release_key(view: &ChunkView, epoch: u64, key: &ChunkKey) {
    let mut data = view.inner.write();
    if data.in_flight.get(key).copied() == Some(epoch) {
        data.in_flight.remove(key);
    }
}

/// Snapshot the context under a short read guard. Returning `None` drops that
/// guard before callers acquire the write lock to release stale work.
fn chunk_view_context(view: &ChunkView, epoch: u64) -> Option<(MinecraftWorldId, i32)> {
    let data = view.read();
    (data.epoch == epoch).then(|| (data.world_id.clone(), data.dimension))
}

/// Sends one chunk batch: load, cached encode, send, then epoch-checked view update.
/// Per-key budget for a single column's load preparation.
///
/// A column that needs more than this releases its in-flight marker and is
/// retried by a later reorder instead of holding the whole batch lease (§8.2).
const PREPARE_KEY_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// One column that finished preparing and is ready for delivery.
struct PreparedColumn {
    key: ChunkKey,
    column: Arc<ChunkColumn>,
}

/// Send one batch of columns: prepare them all in parallel, then deliver the
/// ready ones under this tick's admission budget.
///
/// §8.2 splits the two windows:
/// - **Prepare**: every key in the batch is submitted to the shared worker pool
///   at once, so a deliberately slow column cannot stall the others.
/// - **Delivery**: keys are admitted nearest-first, and only `chunks_per_tick`
///   of them may enter the ordered delivery chain in this tick (checked again at
///   final admission).
///
/// Stalled keys are released rather than awaited, so the batch always makes
/// progress on the columns that are actually ready.
async fn send_chunks_batch(
    world: World,
    view: Arc<ChunkView>,
    entity: EntityId,
    epoch: u64,
    pending: Vec<ChunkKey>,
    spawn_threshold: u32,
    chunks_per_tick: u32,
) {
    let Some((world_id, dimension)) = chunk_view_context(&view, epoch) else {
        // The helper has returned, so no view read guard is held here.
        release_batch(&view, epoch, &pending);
        return;
    };
    let mut context_changes = view.context_epoch_receiver();
    if *context_changes.borrow_and_update() != epoch {
        release_batch(&view, epoch, &pending);
        return;
    }

    let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
        release_batch(&view, epoch, &pending);
        return;
    };
    if connection.connection.is_closed().await || !connection.get_status().can_send_chunks() {
        release_batch(&view, epoch, &pending);
        return;
    }
    let minecraft_world = {
        let Some(manager) = world.get_resource::<MinecraftWorldManager>() else {
            release_batch(&view, epoch, &pending);
            return;
        };
        let Some(minecraft_world) = manager.get_world(&world_id).cloned() else {
            release_batch(&view, epoch, &pending);
            return;
        };
        minecraft_world
    };
    let (min_y, max_y) = minecraft_world.vertical_bounds();

    // Chunk load/generation goes through WorldChunkExecutor (dedicated worker
    // threads with per-key dedup), never inline on Tokio; send tasks only await receipts.
    let provider = minecraft_world.chunk_provider.clone();

    // ---- Prepare window: submit every cold key before awaiting any of them ----
    let mut slots: Vec<Option<PreparedColumn>> =
        (0..pending.len()).map(|_| None).collect::<Vec<_>>();
    let mut waiters: Vec<(usize, _)> = Vec::new();

    for (index, key) in pending.iter().copied().enumerate() {
        if connection.connection.is_closed().await || !connection.get_status().can_send_chunks() {
            release_batch(&view, epoch, &pending[index..]);
            return;
        }
        if !view.context_is_current(epoch) {
            release_batch(&view, epoch, &pending[index..]);
            return;
        }
        if !view.accepts(epoch, &key) {
            release_key(&view, epoch, &key);
            continue;
        }
        if let Some(column) = provider.cached_chunk(key) {
            slots[index] = Some(PreparedColumn { key, column });
            continue;
        }
        let Some(executor) = world
            .get_resource::<WorldChunkExecutor>()
            .map(|res| (*res).clone())
        else {
            release_batch(&view, epoch, &pending[index..]);
            return;
        };
        match executor.try_submit(world_id.clone(), provider.clone(), key, min_y, max_y) {
            Ok(receiver) => waiters.push((index, receiver)),
            Err(ChunkSubmitError::Busy | ChunkSubmitError::TooManyWaiters) => {
                log::debug!(
                    "[chunk] executor saturated; defer ({},{}) until next reorder",
                    key.position.x,
                    key.position.z
                );
                release_key(&view, epoch, &key);
            }
            Err(ChunkSubmitError::Closed) => {
                log::warn!(
                    "{}",
                    t_log!("console.chunk.executor_closed", pos = key.position)
                );
                release_batch(&view, epoch, &pending[index..]);
                return;
            }
        }
    }

    // Collect the loads that finish inside the per-key budget. A stalled column
    // releases its marker so a later reorder can retry it.
    let mut stalled = Vec::new();
    for (index, receiver) in waiters {
        let key = pending[index];
        let loaded = tokio::select! {
            biased;
            result = receiver => Some(result),
            _ = tokio::time::sleep(PREPARE_KEY_BUDGET) => None,
        };
        let Some(result) = loaded else {
            log::debug!(
                "[chunk] load of ({},{}) exceeded the per-key prepare budget; deferring",
                key.position.x,
                key.position.z
            );
            release_key(&view, epoch, &key);
            stalled.push(index);
            continue;
        };
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(_) => {
                log::warn!(
                    "{}",
                    t_log!("console.chunk.receipt_cancelled", pos = key.position)
                );
                release_key(&view, epoch, &key);
                stalled.push(index);
                continue;
            }
        };
        let column = match outcome.as_ref() {
            Ok(Some(column)) => column.clone(),
            Ok(None) => match provider.insert_empty_chunk(key, min_y, max_y) {
                Ok(column) => column,
                Err(error) => {
                    log::debug!(
                        "[chunk] empty column cache admission deferred {}: {error}",
                        key.position
                    );
                    release_key(&view, epoch, &key);
                    stalled.push(index);
                    continue;
                }
            },
            Err(error) => {
                log::warn!(
                    "{}",
                    t_log!("console.chunk.load_fail", pos = key.position, error = error)
                );
                release_key(&view, epoch, &key);
                stalled.push(index);
                continue;
            }
        };
        slots[index] = Some(PreparedColumn { key, column });
    }

    if stalled.len() > 0 && stalled.len() == pending.len() {
        log::debug!(
            "[chunk] no column of this batch became ready ({} stalled); yielding the batch lease",
            stalled.len()
        );
        release_batch(&view, epoch, &pending);
        return;
    }

    // ---- Delivery window: admit ready columns nearest-first ----
    //
    // `slots` keeps the batch order, which `order_chunks` filled nearest-first,
    // so delivery stays distance-prioritised while skipping the columns whose
    // preparation was released above.
    for (index, slot) in slots.into_iter().enumerate() {
        let Some(PreparedColumn { key, column }) = slot else {
            continue;
        };
        if connection.connection.is_closed().await || !connection.get_status().can_send_chunks() {
            release_batch(&view, epoch, &pending[index..]);
            return;
        }
        if !view.context_is_current(epoch) {
            release_batch(&view, epoch, &pending[index..]);
            return;
        }
        if !view.accepts(epoch, &key) {
            release_key(&view, epoch, &key);
            continue;
        }

        let cache_key = ChunkCacheKey::new(world_id.clone(), key);
        let cached = world.get_resource::<LevelChunkCache>().and_then(|cache| {
            cache
                .get(&cache_key)
                .filter(|entry| entry.matches_column(&column, connection.get_protocol_version()))
                .cloned()
        });
        let (payload, sub_chunk_count, payload_generation, payload_profile) = if let Some(entry) =
            cached
        {
            (
                entry.payload,
                entry.subchunk_count,
                entry.generation,
                entry.wire_profile,
            )
        } else {
            let Some(encoder) = world
                .get_resource::<ChunkEncodeExecutor>()
                .map(|resource| (*resource).clone())
            else {
                release_batch(&view, epoch, &pending[index..]);
                return;
            };
            let mut ticket = match encoder.try_submit(cache_key.clone(), column.clone()) {
                Ok(ticket) => ticket,
                Err(ChunkEncodeSubmitError::Busy | ChunkEncodeSubmitError::TooManyWaiters) => {
                    log::debug!(
                        "[chunk] encode queue saturated; defer ({},{}) until next reorder",
                        key.position.x,
                        key.position.z
                    );
                    release_batch(&view, epoch, &pending[index..]);
                    return;
                }
                Err(ChunkEncodeSubmitError::Closed) => {
                    log::warn!(
                        "{}",
                        t_log!("console.chunk.encoder_closed", pos = key.position)
                    );
                    release_batch(&view, epoch, &pending[index..]);
                    return;
                }
            };
            let shared_result = tokio::select! {
                biased;
                result = ticket.wait() => match result {
                    Ok(result) => result,
                    Err(ChunkEncodeWaitError::Closed) => {
                        log::warn!("{}", t_log!("console.chunk.encode_channel_closed", pos = key.position));
                        release_batch(&view, epoch, &pending[index..]);
                        return;
                    }
                },
                changed = context_changes.changed() => {
                    if changed.is_err() || !view.context_is_current(epoch) {
                        release_batch(&view, epoch, &pending[index..]);
                        return;
                    }
                    continue;
                }
            };
            let candidate = match shared_result.as_ref() {
                Ok(candidate) => candidate.clone(),
                Err(ChunkEncodeError::StaleGeneration) => {
                    release_key(&view, epoch, &key);
                    continue;
                }
                Err(ChunkEncodeError::InvalidBlockMapping(error)) => {
                    log::error!(
                        "{}",
                        t_log!(
                            "console.chunk.invalid_block_mapping",
                            x = key.position.x,
                            y = key.position.z,
                            error = error
                        )
                    );
                    release_batch(&view, epoch, &pending[index..]);
                    return;
                }
                Err(ChunkEncodeError::WorkerPanicked(error)) => {
                    log::warn!(
                        "{}",
                        t_log!(
                            "console.chunk.encode_fail",
                            x = key.position.x,
                            y = key.position.z,
                            error = error
                        )
                    );
                    release_key(&view, epoch, &key);
                    continue;
                }
            };
            let published = match publish_encoded_payload(&world, &cache_key, &column, candidate) {
                Ok(entry) => entry,
                Err(PayloadPublishError::CacheUnavailable) => {
                    release_batch(&view, epoch, &pending[index..]);
                    return;
                }
                Err(PayloadPublishError::StaleGeneration) => {
                    release_key(&view, epoch, &key);
                    continue;
                }
            };
            drop(ticket);
            (
                published.payload,
                published.subchunk_count,
                published.generation,
                published.wire_profile,
            )
        };

        if column.generation() != payload_generation {
            release_key(&view, epoch, &key);
            continue;
        }

        // Release view/cache guards before network awaits. A normal center change keeps
        // overlapping keys valid; a hard-context change rejects all old work.
        if !view.context_is_current(epoch) {
            release_batch(&view, epoch, &pending[index..]);
            return;
        }
        if !view.accepts(epoch, &key) {
            release_key(&view, epoch, &key);
            continue;
        }
        log::debug!(
            "[chunk] send ({},{}) payload={}B sub_chunk_count={}",
            key.position.x,
            key.position.z,
            payload.len(),
            sub_chunk_count
        );
        dump_first_chunk_wire(
            key.position.x,
            key.position.z,
            dimension,
            sub_chunk_count,
            payload.as_ref(),
        );
        let Some(send_budget) = world
            .get_resource::<ChunkPayloadSendBudget>()
            .map(|budget| (*budget).clone())
        else {
            release_batch(&view, epoch, &pending[index..]);
            return;
        };
        let Some(payload_permit) = send_budget.try_reserve(payload.len()) else {
            log::debug!(
                "[chunk] transient payload byte budget saturated; defer ({},{})",
                key.position.x,
                key.position.z
            );
            release_batch(&view, epoch, &pending[index..]);
            return;
        };
        let send_result = connection
            .try_send_packet_with_checked_outcome(
                || LevelChunk {
                    chunk_x: key.position.x,
                    chunk_z: key.position.z,
                    dimension,
                    sub_chunk_count,
                    cache_enabled: false,
                    payload: payload.as_ref().clone(),
                },
                true,
                payload_permit,
                |plain, permit, trace| {
                    // Tick -> column -> view -> cipher. The tick guard keeps
                    // the current admission budget stable; all preparation has
                    // finished and this block performs no await or disk I/O.
                    let Some(tick) = world.get_resource::<PipelineTick>() else {
                        return Ok(PacketSendOutcome::BudgetDeferred);
                    };
                    let _chunk = column.read();
                    let mut data = view.write();
                    if data.epoch != epoch
                        || data.world_id != world_id
                        || data.dimension != key.dimension
                        || !data.desired_chunks.contains(&key)
                        || data.in_flight.get(&key) != Some(&epoch)
                    {
                        return Ok(PacketSendOutcome::StaleContext);
                    }
                    if column.generation() != payload_generation
                        || connection.get_protocol_version() != payload_profile
                    {
                        return Ok(PacketSendOutcome::StaleContent);
                    }
                    let count = if data.admission_tick == tick.0 {
                        data.admitted_this_tick
                    } else {
                        0
                    };
                    if count >= chunks_per_tick
                        || data.publisher_pending
                        || data.publisher_in_flight.is_some()
                        || (!data.delivery_ledger.contains_key(&key)
                            && data.delivery_ledger.len() >= MAX_ADMITTED_CHUNK_BASELINES)
                    {
                        return Ok(PacketSendOutcome::BudgetDeferred);
                    }
                    connection.admit_prepared(plain, true, permit, trace)?;
                    data.admission_tick = tick.0;
                    data.admitted_this_tick = count + 1;
                    data.delivery_ledger.insert(
                        key,
                        AdmittedChunkBaseline {
                            incarnation: column.incarnation(),
                            generation: payload_generation,
                        },
                    );
                    data.refresh_required.remove(&key);
                    Ok(PacketSendOutcome::Queued)
                },
            )
            .await;
        match send_result {
            Ok(PacketSendOutcome::Queued) => {}
            Ok(outcome) => {
                log::debug!(
                    "[chunk] LevelChunk ({},{}) not queued: {outcome:?}",
                    key.position.x,
                    key.position.z
                );
                release_batch(&view, epoch, &pending[index..]);
                return;
            }
            Err(TrySendPacketError::Busy) => {
                log::debug!(
                    "[chunk] outbound send slots saturated; defer ({},{}) until next reorder",
                    key.position.x,
                    key.position.z
                );
                release_batch(&view, epoch, &pending[index..]);
                return;
            }
            Err(TrySendPacketError::Connection(error)) => {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.chunk.enqueue_fail",
                        pos = key.position,
                        error = error
                    )
                );
                release_batch(&view, epoch, &pending[index..]);
                return;
            }
        }

        let mut should_sync_equipment_containers = false;
        {
            let mut data = view.inner.write();
            if data.in_flight.get(&key).copied() != Some(epoch) {
                continue;
            }
            data.in_flight.remove(&key);
            if data.epoch == epoch && data.desired_chunks.contains(&key) {
                if data.used_chunks.insert(key) {
                    data.chunks_sent += 1;
                }
                if !data.equipment_container_sync_sent
                    && data.chunks_sent >= EQUIPMENT_CONTAINER_SYNC_CHUNKS
                {
                    data.equipment_container_sync_sent = true;
                    should_sync_equipment_containers = true;
                }
                if !data.has_spawn_chunks && data.chunks_sent >= spawn_threshold {
                    data.has_spawn_chunks = true;
                }
            }
        }

        if should_sync_equipment_containers {
            log::debug!(
                "[chunk] equipment container sync after {} chunks",
                view.read().chunks_sent
            );
            if crate::handler::first_spawn::send_equipment_container_inventory(&world, entity)
                .await
                .is_err()
            {
                release_batch(&view, epoch, &pending[index + 1..]);
                return;
            }
        }
    }

    // Evaluate the spawn threshold after the current chunk-order run. Keep
    // PLAYER_SPAWN after every LevelChunk packet in this batch.
    let should_spawn = {
        let data = view.read();
        data.has_spawn_chunks && connection.get_status() == PlayerConnectionStatus::Initializing
    };
    if should_spawn {
        log::info!(
            "{}",
            t_log!(
                "console.chunk.spawn_threshold",
                sent = view.read().chunks_sent,
                threshold = spawn_threshold
            )
        );
        if let Err(error) =
            crate::handler::first_spawn::send_first_spawn(&world, entity, epoch).await
        {
            log::warn!(
                "{}",
                t_log!("console.chunk.spawn_incomplete", error = error)
            );
            release_batch(&view, epoch, &pending);
            return;
        }
    }

    release_batch(&view, epoch, &pending);
}

/// Returns the chunk containing the player (read from `Transform`).
fn player_center(world: &World, entity: &EntityId) -> Option<ChunkPosition> {
    let transform = world.get_component::<Transform>(entity)?;
    let pos = {
        let tf = transform.inner.read();
        tf.position
    };
    Some(ChunkPosition::from_world(
        pos.x.floor() as i32,
        pos.z.floor() as i32,
    ))
}

// Keeps this import as a semantic boundary for future worker-based submission.
#[allow(dead_code)]
fn _provider_type_marker(_: &WorldChunkProvider) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_chunk_payload_budget_bounds_reservations_and_releases_on_drop() {
        let budget = ChunkPayloadSendBudget::with_limit(10);
        let first = budget.try_reserve(6).expect("reserve six bytes");
        assert_eq!(budget.active_bytes(), 6);
        assert!(budget.try_reserve(5).is_none());
        drop(first);
        assert_eq!(budget.active_bytes(), 0);

        let oversize = budget.try_reserve(11).expect("isolated oversize reserve");
        assert_eq!(budget.active_bytes(), 11);
        assert!(budget.try_reserve(1).is_none());
        drop(oversize);
        assert_eq!(budget.active_bytes(), 0);
    }
    use sc_world::chunk::ChunkPosition;

    #[test]
    fn stale_context_releases_its_batch_after_read_guard_is_dropped() {
        let world_id = MinecraftWorldId::random();
        let view = ChunkView::new(world_id, 0, 2, ChunkPosition::new(0, 0));
        let key = ChunkKey::new(0, ChunkPosition::new(1, 0));
        let stale_epoch = view.read().epoch;
        {
            let mut data = view.write();
            data.epoch = stale_epoch + 1;
            data.in_flight.insert(key, stale_epoch);
            data.send_lease = Some(stale_epoch);
        }

        assert!(chunk_view_context(&view, stale_epoch).is_none());
        release_batch(&view, stale_epoch, &[key]);

        let data = view.read();
        assert!(!data.in_flight.contains_key(&key));
        assert_eq!(data.send_lease, None);
        assert_eq!(data.epoch, stale_epoch + 1);
    }
}
