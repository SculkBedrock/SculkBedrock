//! Chunk send pipeline.
//!
//! Sending uses a subscription epoch plus a per-player send lease: after a
//! reorder/teleport, stale async tasks may exit safely, but results from an
//! never runs two chunk-send batches concurrently.

use crate::chunk_encoder::ChunkEncodeExecutor;
use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::entity::EntityId;
use sc_ecs::params::resource::{Res, ResMut};
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_entity::motion::Transform;
use sc_log::t_log;
use sc_world::chunk::ChunkPosition;
use sc_world::chunk_executor::{TickHealth, WorldChunkExecutor};
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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::player_connection::{
    PacketSendOutcome, PlayerConnection, PlayerConnectionStatus,
};
use crate::protocol::server::chunk::{LevelChunk, NetworkChunkPublisherUpdate};
use crate::utils::ConnectionThreadManager;

static TOTAL_CHUNKS_SENT: AtomicU64 = AtomicU64::new(0);
static WINDOW_CHUNKS_SENT: AtomicU64 = AtomicU64::new(0);

struct CpsTracker {
    last_check: Instant,
    idle_streak: u32,
}

static CPS_TRACKER: Mutex<Option<CpsTracker>> = Mutex::new(None);
static LAST_TICK_TIME: Mutex<Option<Instant>> = Mutex::new(None);
static TICK_EWMA_MS: Mutex<f64> = Mutex::new(50.0);

static CHUNK_WIRE_DUMPED: AtomicBool = AtomicBool::new(false);
static CHUNK_WIRE_DUMP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const EQUIPMENT_CONTAINER_SYNC_CHUNKS: u32 = 7;
const DEFAULT_ACTIVE_CHUNK_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

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

    // Retained for debugging/observability; not yet read in this build.
    #[allow(dead_code)]
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

/// 全局管线计时，每个 tick 由 order_chunks 递增。
///
/// Global pipeline tick incremented by order_chunks every tick.
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

/// Periodic CPS monitor that logs chunk throughput per second.
fn tick_cps_logging(
    has_players: bool,
    in_flight: usize,
    queued: usize,
    worker_stats: Option<(usize, usize, TickHealth)>,
) {
    if !has_players && in_flight == 0 && queued == 0 {
        return;
    }

    let now = Instant::now();
    let Ok(mut lock) = CPS_TRACKER.lock() else {
        return;
    };
    let tracker = lock.get_or_insert_with(|| CpsTracker {
        last_check: now,
        idle_streak: 0,
    });

    let elapsed = now.duration_since(tracker.last_check);
    if elapsed < std::time::Duration::from_millis(1000) {
        return;
    }

    tracker.last_check = now;
    let count = WINDOW_CHUNKS_SENT.swap(0, Ordering::Relaxed);
    let total = TOTAL_CHUNKS_SENT.load(Ordering::Relaxed);
    let elapsed_secs = elapsed.as_secs_f64();
    let cps = count as f64 / elapsed_secs;

    let workers_str = match worker_stats {
        Some((active, current, health)) => format!("{active}/{current} ({health:?})"),
        None => "N/A".to_string(),
    };

    if count > 0 || in_flight > 0 || queued > 0 {
        tracker.idle_streak = 0;
        log::info!(
            "{}",
            t_log!(
                "console.chunk.cps",
                cps = format!("{:.1}", cps),
                secs = format!("{:.1}", elapsed_secs),
                count = count,
                in_flight = in_flight,
                queued = queued,
                workers = workers_str,
                total = total,
            )
        );
    } else if tracker.idle_streak < 2 && has_players {
        tracker.idle_streak += 1;
        if tracker.idle_streak == 1 {
            log::info!("{}", t_log!("console.chunk.cps_idle", total = total));
        }
    }
}

/// Sends each player load_queue within a per-tick per-player budget.
pub(crate) fn send_next_chunk(world: World, settings: Res<ChunkSendSettings>) {
    // Tick duration and TPS concession monitor
    let now = Instant::now();
    if let Ok(mut last_tick_guard) = LAST_TICK_TIME.lock() {
        if let Some(prev) = *last_tick_guard {
            let tick_delta_ms = now.duration_since(prev).as_secs_f64() * 1000.0;
            *last_tick_guard = Some(now);

            let smoothed_ms = if let Ok(mut ewma_guard) = TICK_EWMA_MS.lock() {
                // EWMA smoothing with alpha = 0.2 (adapts across ~5 ticks)
                let next_ewma = *ewma_guard * 0.8 + tick_delta_ms * 0.2;
                *ewma_guard = next_ewma;
                next_ewma
            } else {
                tick_delta_ms
            };

            // User-specified concession thresholds:
            // TPS >= 18 (<= 55.55ms): Healthy (full worker capacity)
            // TPS 15 ~ 18 (55.55ms ~ 66.67ms): Pressured (scale down to 2/3)
            // TPS < 15 (> 66.67ms): Critical (concede CPU to game loop, min workers)
            let health = if smoothed_ms <= 55.55 {
                TickHealth::Healthy
            } else if smoothed_ms <= 66.67 {
                TickHealth::Pressured
            } else {
                TickHealth::Critical
            };

            if let Some(executor) = world.get_resource::<WorldChunkExecutor>() {
                executor.set_tick_health(health);
            }
        } else {
            *last_tick_guard = Some(now);
        }
    }

    let chunks_per_tick = settings.chunks_per_tick;
    let spawn_threshold = settings.spawn_threshold;

    let mut total_in_flight = 0;
    let mut total_queued = 0;
    let mut has_players = false;

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
        has_players = true;
        let (epoch, pending) = {
            let mut data = view.inner.write();
            if data.send_lease.is_some() {
                total_in_flight += data.in_flight.len();
                total_queued += data.load_queue.len();
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
            total_in_flight += data.in_flight.len();
            total_queued += data.load_queue.len();
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

    let worker_stats = world.get_resource::<WorldChunkExecutor>().map(|executor| {
        (
            executor.active_workers(),
            executor.current_workers(),
            executor.tick_health(),
        )
    });

    tick_cps_logging(has_players, total_in_flight, total_queued, worker_stats);
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
fn encode_chunk_direct(
    column: &Arc<ChunkColumn>,
    wire_profile: u32,
) -> Result<Arc<CachedChunk>, ()> {
    let chunk = column.read();
    let generation = column.generation();
    let incarnation = column.incarnation();
    let packet = LevelChunk::from_chunk(&chunk).map_err(|_| ())?;
    Ok(Arc::new(CachedChunk {
        payload: Arc::new(packet.payload),
        subchunk_count: packet.sub_chunk_count,
        generation,
        incarnation,
        wire_profile,
    }))
}

async fn process_and_send_chunk(
    world: World,
    view: Arc<ChunkView>,
    connection: Arc<PlayerConnection>,
    provider: WorldChunkProvider,
    executor: Option<WorldChunkExecutor>,
    encoder: Option<ChunkEncodeExecutor>,
    send_budget: Option<ChunkPayloadSendBudget>,
    world_id: MinecraftWorldId,
    dimension: i32,
    min_y: i32,
    max_y: i32,
    epoch: u64,
    key: ChunkKey,
    spawn_threshold: u32,
    chunks_per_tick: u32,
    entity: EntityId,
) {
    if connection.connection.is_closed().await || !connection.get_status().can_send_chunks() {
        release_key(&view, epoch, &key);
        return;
    }
    if !view.context_is_current(epoch) || !view.accepts(epoch, &key) {
        release_key(&view, epoch, &key);
        return;
    }

    // 1. Get or generate chunk column
    let column = if let Some(column) = provider.cached_chunk(key) {
        column
    } else {
        let Some(executor) = executor else {
            release_key(&view, epoch, &key);
            return;
        };
        let receiver = match executor.try_submit(world_id.clone(), provider.clone(), key, min_y, max_y) {
            Ok(rx) => rx,
            Err(_) => {
                release_key(&view, epoch, &key);
                return;
            }
        };
        let loaded = tokio::select! {
            biased;
            result = receiver => Some(result),
            _ = tokio::time::sleep(PREPARE_KEY_BUDGET) => None,
        };
        let Some(result) = loaded else {
            release_key(&view, epoch, &key);
            return;
        };
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(_) => {
                release_key(&view, epoch, &key);
                return;
            }
        };
        match outcome.as_ref() {
            Ok(Some(column)) => column.clone(),
            Ok(None) => match provider.insert_empty_chunk(key, min_y, max_y) {
                Ok(column) => column,
                Err(_) => {
                    release_key(&view, epoch, &key);
                    return;
                }
            },
            Err(error) => {
                log::warn!("{}", t_log!("console.chunk.load_fail", pos = key.position, error = error));
                release_key(&view, epoch, &key);
                return;
            }
        }
    };

    if connection.connection.is_closed().await || !connection.get_status().can_send_chunks() {
        release_key(&view, epoch, &key);
        return;
    }
    if !view.context_is_current(epoch) || !view.accepts(epoch, &key) {
        release_key(&view, epoch, &key);
        return;
    }

    // 2. Encode payload (check cache first, then encoder executor or direct encode fallback)
    let cache_key = ChunkCacheKey::new(world_id.clone(), key);
    let cached = world.get_resource::<LevelChunkCache>().and_then(|cache| {
        cache
            .get(&cache_key)
            .filter(|entry| entry.matches_column(&column, connection.get_protocol_version()))
            .cloned()
    });

    let (payload, sub_chunk_count, payload_generation, payload_profile) = if let Some(entry) = cached {
        (entry.payload, entry.subchunk_count, entry.generation, entry.wire_profile)
    } else {
        let encoded_candidate = if let Some(encoder) = &encoder {
            if let Ok(mut ticket) = encoder.try_submit(cache_key.clone(), column.clone()) {
                match ticket.wait().await {
                    Ok(shared) => match shared.as_ref() {
                        Ok(candidate) => Some(candidate.clone()),
                        Err(_) => None,
                    },
                    Err(_) => None,
                }
            } else {
                None
            }
        } else {
            None
        };

        let candidate = match encoded_candidate {
            Some(cand) => cand,
            None => match encode_chunk_direct(&column, connection.get_protocol_version()) {
                Ok(cand) => cand,
                Err(_) => {
                    release_key(&view, epoch, &key);
                    return;
                }
            },
        };

        let published = match publish_encoded_payload(&world, &cache_key, &column, candidate) {
            Ok(entry) => entry,
            Err(_) => {
                release_key(&view, epoch, &key);
                return;
            }
        };

        (published.payload, published.subchunk_count, published.generation, published.wire_profile)
    };

    if column.generation() != payload_generation {
        release_key(&view, epoch, &key);
        return;
    }
    if !view.context_is_current(epoch) || !view.accepts(epoch, &key) {
        release_key(&view, epoch, &key);
        return;
    }

    dump_first_chunk_wire(
        key.position.x,
        key.position.z,
        dimension,
        sub_chunk_count,
        payload.as_ref(),
    );

    let Some(send_budget) = send_budget else {
        release_key(&view, epoch, &key);
        return;
    };
    let Some(payload_permit) = send_budget.try_reserve(payload.len()) else {
        release_key(&view, epoch, &key);
        return;
    };

    // 3. Send LevelChunk packet
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
        Ok(PacketSendOutcome::Queued) => {
            WINDOW_CHUNKS_SENT.fetch_add(1, Ordering::Relaxed);
            TOTAL_CHUNKS_SENT.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            release_key(&view, epoch, &key);
            return;
        }
    }

    // 4. Update view statistics and equipment containers
    let mut should_sync_equipment_containers = false;
    {
        let mut data = view.inner.write();
        if data.in_flight.get(&key).copied() == Some(epoch) {
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
    }

    if should_sync_equipment_containers {
        let _ = crate::handler::first_spawn::send_equipment_container_inventory(&world, entity).await;
    }
}

/// Send one batch of columns by streaming: prepare each column concurrently,
/// and deliver each column immediately as soon as it finishes decoration.
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
    let provider = minecraft_world.chunk_provider.clone();
    let executor = world.get_resource::<WorldChunkExecutor>().map(|res| (*res).clone());
    let encoder = world.get_resource::<ChunkEncodeExecutor>().map(|res| (*res).clone());
    let send_budget = world.get_resource::<ChunkPayloadSendBudget>().map(|budget| (*budget).clone());

    let mut join_set = tokio::task::JoinSet::new();

    for key in pending.iter().copied() {
        let task_world = world.clone();
        let task_view = view.clone();
        let task_conn = connection.clone();
        let task_provider = provider.clone();
        let task_executor = executor.clone();
        let task_encoder = encoder.clone();
        let task_budget = send_budget.clone();
        let task_world_id = world_id.clone();

        join_set.spawn(async move {
            process_and_send_chunk(
                task_world,
                task_view,
                task_conn,
                task_provider,
                task_executor,
                task_encoder,
                task_budget,
                task_world_id,
                dimension,
                min_y,
                max_y,
                epoch,
                key,
                spawn_threshold,
                chunks_per_tick,
                entity,
            )
            .await;
        });
    }

    while let Some(_) = join_set.join_next().await {}

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
        let _ = crate::handler::first_spawn::send_first_spawn(&world, entity, epoch).await;
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
