//! Chunk pipeline data model.
//!
//! Defines state only, no behavior: the network-side `order_chunks` / `send_next_chunk`
//! consume these types. Subscription generations are kept here so stale async tasks across
//! dimension/teleport switches cannot pollute the current player view; the payload cache uses a saved-world dimension isolation key.

use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::chunk::ChunkPosition;
use crate::manager::MinecraftWorldId;
use crate::storage::ChunkKey;

/// Chunk-send settings (per-tick count / spawn threshold / view distance).
#[derive(Resource, Clone, Debug)]
pub struct ChunkSendSettings {
    /// Max effectively admitted chunks per tick (default 4).
    ///
    /// This is a delivery budget: prepared payloads wait in the delivery window
    /// until a tick slot frees up. It is no longer also the amount of in-flight
    /// preparation work (§8.2).
    pub chunks_per_tick: u32,
    /// Max chunks per player concurrently in the admitted-prepared state.
    ///
    /// Preparation (worker load + encode) is bounded separately from delivery so
    /// one slow column can no longer occupy a player's whole per-tick allowance,
    /// and so an idle tick cannot accumulate an unbounded backlog. The design doc
    /// lists 8–16 as the experiment range; 12 is the starting value.
    pub prepare_window: u32,
    /// Chunks sent before PlayStatus::PLAYER_SPAWN (default 56).
    pub spawn_threshold: u32,
    /// Player view distance (chunk radius; default 10).
    pub view_distance: i32,
    /// Tick interval between `order_chunks` runs (default 20).
    pub reorder_interval: u32,
    /// Unload grace period after a chunk leaves view distance (ticks; default 600 = 30s).
    pub unload_grace_ticks: u32,
}

impl Default for ChunkSendSettings {
    fn default() -> Self {
        Self {
            chunks_per_tick: 4,
            prepare_window: 12,
            spawn_threshold: 56,
            view_distance: 10,
            reorder_interval: 20,
            unload_grace_ticks: 600,
        }
    }
}

impl ChunkSendSettings {
    /// Build from ServerProperties ([chunk] section), falling back to defaults when absent.
    pub fn from_properties(view_distance: i32, chunks_per_tick: u32, spawn_threshold: u32) -> Self {
        Self {
            view_distance,
            chunks_per_tick,
            spawn_threshold,
            ..Self::default()
        }
    }

    /// Effective preparation window; never below the delivery budget so a
    /// configuration of `chunks_per_tick > prepare_window` cannot deadlock.
    pub fn prepare_window(&self) -> u32 {
        self.prepare_window.max(self.chunks_per_tick).max(1)
    }
}

/// Per-player chunk view state (sent chunks / load queue / next reorder run).
#[derive(Component, Clone, Debug)]
pub struct ChunkView {
    pub inner: Arc<parking_lot::RwLock<ChunkViewData>>,
    context_epoch_tx: tokio::sync::watch::Sender<u64>,
}

#[derive(Clone, Copy, Debug)]
pub struct AdmittedChunkBaseline {
    pub incarnation: u128,
    pub generation: u64,
}

pub const MAX_ADMITTED_CHUNK_BASELINES: usize = 4096;

/// Bounded number of columns tracked per connection for delta-version gaps.
pub const MAX_TRACKED_DELTA_GAP_COLUMNS: usize = 512;

/// Mutable chunk-view state.
#[derive(Clone, Debug)]
pub struct ChunkViewData {
    /// Hard context generation; incremented on world/dimension/teleport context switches.
    /// Ordinary center moves only bump `view_revision` without discarding still-overlapping work.
    pub epoch: u64,
    /// Version of the desired chunk set/priority; incremented on ordinary center or radius changes.
    pub view_revision: u64,
    /// Saved world the player is in.
    pub world_id: MinecraftWorldId,
    /// World dimension id (used to build ChunkKey).
    pub dimension: i32,
    /// Current view radius (chunk count).
    pub radius: i32,
    /// View center (chunk the player is in).
    pub center: ChunkPosition,
    /// Desired chunk set for the current epoch.
    pub desired_chunks: HashSet<ChunkKey>,
    /// Chunks already sent to this player.
    pub used_chunks: HashSet<ChunkKey>,
    pub delivery_ledger: HashMap<ChunkKey, AdmittedChunkBaseline>,
    pub refresh_required: HashSet<ChunkKey>,
    pub admission_tick: u64,
    pub admitted_this_tick: u32,
    /// Pending send queue, ordered by (distance-squared, chunk) (near first).
    pub load_queue: BTreeSet<(u64, ChunkKey)>,
    /// Popped chunks whose send task is still in flight, with their owning epoch.
    pub in_flight: HashMap<ChunkKey, u64>,
    /// At most one chunk-send task per player; value is the holder hard-context epoch.
    pub send_lease: Option<u64>,
    /// Publisher update not yet queued; set when view/context changes.
    pub publisher_pending: bool,
    /// In-flight publisher-update attempt ticket (context epoch, view revision).
    pub publisher_in_flight: Option<(u64, u64)>,
    /// Ticks remaining until the next reorder.
    pub next_order_run: u32,
    /// Total chunks sent (spawn-threshold check).
    pub chunks_sent: u32,
    /// Whether the pre-spawn equipment container sync has been sent.
    ///
    /// Armor/cursor/offhand inventory state is sent during the chunk phase,
    /// before the final full inventory sync and PLAYER_SPAWN transition.
    pub equipment_container_sync_sent: bool,
    /// Whether PlayStatus::PLAYER_SPAWN has been sent.
    pub has_spawn_chunks: bool,
    /// Whether an unreachable PLAYER_SPAWN threshold has been diagnosed for
    /// this hard context. Keeps repeated radius requests from spamming logs.
    spawn_threshold_warning_emitted: bool,
    /// Out-of-range chunk to expiry tick (grace-period unload).
    pub unloading: HashMap<ChunkKey, u64>,
    /// Ticks this view went unscheduled in a row (cross-player fairness).
    ///
    /// §8.3: fairness uses a rotation cursor plus waiting age, so a fixed
    /// traversal order cannot let one player monopolize the budget. The value is
    /// the age of this view's demand and feeds the per-tick rotation.
    pub waiting_ticks: u32,
    /// `waiting_ticks` at the last scheduling (diagnostics only).
    pub last_served_waiting_ticks: u32,
    /// Columns whose admitted block deltas skipped at least one content version.
    ///
    /// §10.3: a delta that jumps more than one content generation can only carry
    /// its own block, so an older edit of a *different* position in the same
    /// column stays unapplied until the column is refreshed. That is the precise
    /// condition under which a bounded delta journal becomes necessary, so it is
    /// counted per connection instead of being assumed away.
    pub delta_gap_columns: HashSet<ChunkKey>,
    /// Total observed version gaps (diagnostic counter).
    pub delta_gap_total: u64,
}

impl ChunkViewData {
    pub fn request_refresh(&mut self, key: ChunkKey) {
        if self.delivery_ledger.contains_key(&key) || self.desired_chunks.contains(&key) {
            self.refresh_required.insert(key);
        }
        if self.desired_chunks.contains(&key) {
            let dx = (i64::from(key.position.x) - i64::from(self.center.x)).unsigned_abs();
            let dz = (i64::from(key.position.z) - i64::from(self.center.z)).unsigned_abs();
            let distance = dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz));
            self.load_queue.insert((distance, key));
        }
    }

    fn advance_view_revision(&mut self) {
        self.view_revision = self.view_revision.wrapping_add(1).max(1);
        self.publisher_pending = true;
    }

    /// Update the moving center without invalidating the hard context epoch.
    /// The planner replaces desired/load sets in the same tick.
    pub fn update_center(&mut self, center: ChunkPosition) -> bool {
        if self.center == center {
            return false;
        }
        self.center = center;
        self.advance_view_revision();
        true
    }

    /// Apply a normal radius/center change while preserving already-used and
    /// in-flight overlap. Desired keys are updated immediately so stale sends
    /// cannot rely on the old radius before the planner's next tick.
    pub fn update_view(&mut self, radius: i32, center: ChunkPosition) -> bool {
        let radius = radius.max(0);
        if self.radius == radius && self.center == center {
            return false;
        }
        self.radius = radius;
        self.center = center;
        self.advance_view_revision();
        self.next_order_run = 0;
        self.load_queue.clear();

        let mut desired = HashSet::with_capacity(((radius * 2 + 1) as usize).pow(2));
        for x in -radius..=radius {
            for z in -radius..=radius {
                desired.insert(ChunkKey::new(
                    self.dimension,
                    ChunkPosition::new(center.x + x, center.z + z),
                ));
            }
        }
        self.desired_chunks = desired;
        true
    }

    /// Reserve up to `limit` keys for preparation, honouring the in-flight cap.
    ///
    /// The delivery budget (`chunks_per_tick`) is applied later, at admission
    /// time; this only reserves preparation slots.
    pub fn take_prepare_batch(
        &mut self,
        prepare_window: u32,
        already_in_flight: usize,
    ) -> Vec<ChunkKey> {
        let window = prepare_window.max(1) as usize;
        let mut room = window.saturating_sub(already_in_flight);
        if room == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(room.min(self.load_queue.len()));
        while room > 0 {
            let Some((_, key)) = self.load_queue.pop_first() else {
                break;
            };
            if self.in_flight.contains_key(&key)
                || (self.used_chunks.contains(&key) && !self.refresh_required.contains(&key))
            {
                continue;
            }
            out.push(key);
            room -= 1;
        }
        self.last_served_waiting_ticks = self.waiting_ticks;
        self.waiting_ticks = 0;
        out
    }

    /// Note that a block delta skipped one or more content versions.
    ///
    /// Returns `true` when this column newly entered the gapped set. The caller
    /// uses it for a rate-limited diagnostic; the bounded set itself keeps the
    /// accounting free.
    pub fn note_delta_gap(&mut self, key: ChunkKey) -> bool {
        self.delta_gap_total = self.delta_gap_total.saturating_add(1);
        let inserted = self.delta_gap_columns.insert(key);
        if self.delta_gap_columns.len() > MAX_TRACKED_DELTA_GAP_COLUMNS {
            // Keep the diagnostic bounded; the counter stays exact.
            if let Some(first) = self.delta_gap_columns.iter().next().copied() {
                self.delta_gap_columns.remove(&first);
            }
        }
        inserted
    }

    /// Columns currently known to have skipped content versions.
    pub fn delta_gap_column_count(&self) -> usize {
        self.delta_gap_columns.len()
    }

    /// Total version gaps observed for this view.
    pub fn delta_gap_total(&self) -> u64 {
        self.delta_gap_total
    }

    /// Record that this view was not served this tick, so its demand ages.
    pub fn note_unserved_tick(&mut self) {
        self.waiting_ticks = self.waiting_ticks.saturating_add(1);
    }

    /// How long this view has been waiting for delivery budget.
    pub fn waiting_age(&self) -> u32 {
        self.waiting_ticks
    }

    /// Reserve one publisher update for the current view version.
    pub fn take_publisher_ticket(&mut self) -> Option<(u64, u64)> {
        if !self.publisher_pending || self.publisher_in_flight.is_some() {
            return None;
        }
        let ticket = (self.epoch, self.view_revision);
        self.publisher_in_flight = Some(ticket);
        Some(ticket)
    }
}

impl ChunkView {
    pub fn new(
        world_id: MinecraftWorldId,
        dimension: i32,
        radius: i32,
        center: ChunkPosition,
    ) -> Self {
        let (context_epoch_tx, _) = tokio::sync::watch::channel(1);
        Self {
            inner: Arc::new(parking_lot::RwLock::new(ChunkViewData {
                epoch: 1,
                view_revision: 1,
                world_id,
                dimension,
                radius,
                center,
                desired_chunks: HashSet::new(),
                used_chunks: HashSet::new(),
                delivery_ledger: HashMap::new(),
                refresh_required: HashSet::new(),
                admission_tick: 0,
                admitted_this_tick: 0,
                load_queue: BTreeSet::new(),
                in_flight: HashMap::new(),
                send_lease: None,
                publisher_pending: true,
                publisher_in_flight: None,
                next_order_run: 0,
                chunks_sent: 0,
                equipment_container_sync_sent: false,
                has_spawn_chunks: false,
                spawn_threshold_warning_emitted: false,
                unloading: HashMap::new(),
                waiting_ticks: 0,
                last_served_waiting_ticks: 0,
                delta_gap_columns: HashSet::new(),
                delta_gap_total: 0,
            })),
            context_epoch_tx,
        }
    }

    pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, ChunkViewData> {
        self.inner.read()
    }

    pub fn write(&self) -> parking_lot::RwLockWriteGuard<'_, ChunkViewData> {
        self.inner.write()
    }

    /// Restart the chunk subscription context. Old async tasks may still finish but cannot commit results.
    pub fn reset_subscription(
        &self,
        world_id: MinecraftWorldId,
        dimension: i32,
        radius: i32,
        center: ChunkPosition,
    ) -> u64 {
        let mut data = self.inner.write();
        data.epoch = data.epoch.wrapping_add(1).max(1);
        self.context_epoch_tx.send_replace(data.epoch);
        data.advance_view_revision();
        data.publisher_in_flight = None;
        data.world_id = world_id;
        data.dimension = dimension;
        data.radius = radius;
        data.center = center;
        data.desired_chunks.clear();
        data.used_chunks.clear();
        data.delivery_ledger.clear();
        data.refresh_required.clear();
        data.admission_tick = 0;
        data.admitted_this_tick = 0;
        data.load_queue.clear();
        data.in_flight.clear();
        data.unloading.clear();
        data.send_lease = None;
        data.next_order_run = 0;
        data.chunks_sent = 0;
        data.equipment_container_sync_sent = false;
        data.has_spawn_chunks = false;
        data.spawn_threshold_warning_emitted = false;
        data.waiting_ticks = 0;
        data.last_served_waiting_ticks = 0;
        data.delta_gap_columns.clear();
        data.delta_gap_total = 0;
        data.epoch
    }

    /// Mark and report an unreachable PLAYER_SPAWN threshold for the current
    /// negotiated square view. Returns the maximum number of chunks available
    /// on the first diagnostic in this hard context.
    pub fn diagnose_unreachable_spawn_threshold(&self, threshold: u32) -> Option<u64> {
        let mut data = self.inner.write();
        let radius = data.radius.max(0) as u64;
        let side = radius.saturating_mul(2).saturating_add(1);
        let available = side.saturating_mul(side);
        if u64::from(threshold) <= available {
            return None;
        }

        if data.spawn_threshold_warning_emitted {
            return None;
        }
        data.spawn_threshold_warning_emitted = true;
        Some(available)
    }

    /// Subscribe to hard-context changes so queued consumers can drop their
    /// shared-load waiter without cancelling work still needed by other players.
    pub fn context_epoch_receiver(&self) -> tokio::sync::watch::Receiver<u64> {
        self.context_epoch_tx.subscribe()
    }

    pub fn publisher_ticket_is_current(&self, ticket: (u64, u64)) -> bool {
        let data = self.read();
        data.epoch == ticket.0
            && data.view_revision == ticket.1
            && data.publisher_in_flight == Some(ticket)
    }

    /// Complete publisher admission. Failed/stale attempts leave the current
    /// view dirty so the next reorder can retry it.
    pub fn complete_publisher_ticket(&self, ticket: (u64, u64), queued: bool) {
        let mut data = self.write();
        if data.publisher_in_flight != Some(ticket) {
            return;
        }
        data.publisher_in_flight = None;
        if queued && data.epoch == ticket.0 && data.view_revision == ticket.1 {
            data.publisher_pending = false;
        }
    }

    /// Whether an asynchronous result still belongs to the active hard context.
    pub fn context_is_current(&self, epoch: u64) -> bool {
        self.inner.read().epoch == epoch
    }

    /// Whether the key still belongs to the same subscription context and is in view.
    pub fn accepts(&self, epoch: u64, key: &ChunkKey) -> bool {
        let data = self.inner.read();
        data.epoch == epoch && data.desired_chunks.contains(key)
    }

    /// Whether the chunk is subscribed (already sent).
    pub fn is_subscribed(&self, key: &ChunkKey) -> bool {
        self.read().used_chunks.contains(key)
    }
}

/// World isolation key for the payload cache.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ChunkCacheKey {
    pub world_id: MinecraftWorldId,
    pub chunk: ChunkKey,
}

impl ChunkCacheKey {
    pub fn new(world_id: MinecraftWorldId, chunk: ChunkKey) -> Self {
        Self { world_id, chunk }
    }
}

/// Encoded LevelChunk payload cache entry.
#[derive(Clone, Debug)]
pub struct CachedChunk {
    /// Encoded LevelChunk payload (without the x/z/dimension header).
    pub payload: Arc<Vec<u8>>,
    /// sub_chunk_count field.
    pub subchunk_count: u32,
    /// Source ChunkColumn change counter; mismatch invalidates the entry for re-encoding.
    pub generation: u64,
    pub incarnation: u128,
    pub wire_profile: u32,
}

impl CachedChunk {
    pub fn matches_column(&self, column: &crate::storage::ChunkColumn, wire_profile: u32) -> bool {
        self.incarnation == column.incarnation()
            && self.generation == column.generation()
            && self.wire_profile == wire_profile
    }
}

/// Global LevelChunk payload cache (saved world + dimension + x/z to encoded result).
///
/// The cache is bounded by both entry count and owned payload allocation
/// capacity. `payload_bytes` intentionally excludes HashMap/order metadata and
/// Arc references held by active send tasks; it is a cache-owned allocation
/// estimate, not process RSS or total live-payload memory.
const DEFAULT_LEVEL_CHUNK_CACHE_CAPACITY: usize = 256;
const DEFAULT_LEVEL_CHUNK_CACHE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
struct LevelChunkCacheEntry {
    chunk: CachedChunk,
    order_token: u64,
    payload_bytes: usize,
}

#[derive(Resource, Clone, Debug)]
pub struct LevelChunkCache {
    entries: HashMap<ChunkCacheKey, LevelChunkCacheEntry>,
    /// Append-only insertion tokens; stale tokens are skipped during eviction.
    order: VecDeque<(ChunkCacheKey, u64)>,
    next_order_token: u64,
    max_entries: usize,
    max_payload_bytes: usize,
    payload_bytes: usize,
}

impl Default for LevelChunkCache {
    fn default() -> Self {
        Self::with_limits(
            DEFAULT_LEVEL_CHUNK_CACHE_CAPACITY,
            DEFAULT_LEVEL_CHUNK_CACHE_BYTES,
        )
    }
}

impl LevelChunkCache {
    pub fn with_capacity(max_entries: usize) -> Self {
        Self::with_limits(max_entries, DEFAULT_LEVEL_CHUNK_CACHE_BYTES)
    }

    pub fn with_limits(max_entries: usize, max_payload_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            next_order_token: 1,
            max_entries: max_entries.max(1),
            max_payload_bytes: max_payload_bytes.max(1),
            payload_bytes: 0,
        }
    }

    pub fn get(&self, key: &ChunkCacheKey) -> Option<&CachedChunk> {
        self.entries.get(key).map(|entry| &entry.chunk)
    }

    pub fn insert(&mut self, key: ChunkCacheKey, chunk: CachedChunk) {
        let payload_bytes = chunk.payload.capacity();
        self.remove_entry(&key);
        // An individual payload larger than the entire cache budget is sent to
        // the current consumer but not retained as a permanent cache entry.
        if payload_bytes > self.max_payload_bytes {
            self.compact_order_if_needed();
            return;
        }

        let order_token = self.next_order_token;
        self.next_order_token = self.next_order_token.wrapping_add(1).max(1);
        self.payload_bytes = self.payload_bytes.saturating_add(payload_bytes);
        self.entries.insert(
            key.clone(),
            LevelChunkCacheEntry {
                chunk,
                order_token,
                payload_bytes,
            },
        );
        self.order.push_back((key, order_token));

        while self.entries.len() > self.max_entries || self.payload_bytes > self.max_payload_bytes {
            let Some((oldest_key, oldest_token)) = self.order.pop_front() else {
                break;
            };
            let is_current = self
                .entries
                .get(&oldest_key)
                .is_some_and(|entry| entry.order_token == oldest_token);
            if is_current {
                self.remove_entry(&oldest_key);
            }
        }
        self.compact_order_if_needed();
    }

    pub fn invalidate(&mut self, key: &ChunkCacheKey) {
        self.remove_entry(key);
        self.compact_order_if_needed();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.payload_bytes = 0;
    }

    fn remove_entry(&mut self, key: &ChunkCacheKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.payload_bytes = self.payload_bytes.saturating_sub(entry.payload_bytes);
        }
    }

    fn compact_order_if_needed(&mut self) {
        let threshold = self.entries.len().saturating_mul(2).saturating_add(64);
        if self.order.len() <= threshold {
            return;
        }
        let entries = &self.entries;
        self.order.retain(|(key, token)| {
            entries
                .get(key)
                .is_some_and(|entry| entry.order_token == *token)
        });
    }

    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }

    pub fn max_payload_bytes(&self) -> usize {
        self.max_payload_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_defaults_match_reference() {
        let settings = ChunkSendSettings::default();
        assert_eq!(settings.chunks_per_tick, 4);
        assert_eq!(settings.prepare_window, 12);
        assert!(settings.prepare_window() >= settings.chunks_per_tick);
        assert_eq!(settings.spawn_threshold, 56);
        assert_eq!(settings.view_distance, 10);
        assert_eq!(settings.reorder_interval, 20);
    }

    #[test]
    fn unreachable_spawn_threshold_is_diagnosed_once_per_context() {
        let world_id = MinecraftWorldId::random();
        let center = ChunkPosition::new(0, 0);
        let view = ChunkView::new(world_id.clone(), 0, 2, center);

        assert_eq!(view.diagnose_unreachable_spawn_threshold(56), Some(25));
        assert_eq!(view.diagnose_unreachable_spawn_threshold(56), None);
        assert_eq!(view.diagnose_unreachable_spawn_threshold(25), None);

        view.write().update_view(3, center);
        assert_eq!(view.diagnose_unreachable_spawn_threshold(56), None);
        view.write().update_view(4, center);
        assert_eq!(view.diagnose_unreachable_spawn_threshold(56), None);

        view.reset_subscription(world_id, 0, 2, center);
        assert_eq!(view.diagnose_unreachable_spawn_threshold(56), Some(25));
    }

    #[test]
    fn cache_isolated_by_world_id() {
        let mut cache = LevelChunkCache::default();
        let chunk = ChunkKey::new(0, ChunkPosition::new(1, -2));
        let world_a = MinecraftWorldId::random();
        let world_b = MinecraftWorldId::random();
        cache.insert(
            ChunkCacheKey::new(world_a.clone(), chunk),
            CachedChunk {
                payload: Arc::new(vec![1, 2, 3]),
                subchunk_count: 24,
                generation: 7,
                incarnation: 1,
                wire_profile: 2168,
            },
        );
        assert!(cache.get(&ChunkCacheKey::new(world_a, chunk)).is_some());
        assert!(cache.get(&ChunkCacheKey::new(world_b, chunk)).is_none());
    }

    #[test]
    fn cache_evicts_oldest_entry_at_capacity() {
        let mut cache = LevelChunkCache::with_capacity(2);
        let world_id = MinecraftWorldId::random();
        let first =
            ChunkCacheKey::new(world_id.clone(), ChunkKey::new(0, ChunkPosition::new(0, 0)));
        let second =
            ChunkCacheKey::new(world_id.clone(), ChunkKey::new(0, ChunkPosition::new(1, 0)));
        let third = ChunkCacheKey::new(world_id, ChunkKey::new(0, ChunkPosition::new(2, 0)));
        let cached = |value| CachedChunk {
            payload: Arc::new(vec![value]),
            subchunk_count: 1,
            generation: 0,
            incarnation: 1,
            wire_profile: 2168,
        };
        cache.insert(first.clone(), cached(1));
        cache.insert(second.clone(), cached(2));
        cache.insert(third.clone(), cached(3));
        assert!(cache.get(&first).is_none());
        assert!(cache.get(&second).is_some());
        assert!(cache.get(&third).is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn ordinary_center_move_advances_view_revision_without_invalidating_context() {
        let world_id = MinecraftWorldId::random();
        let view = ChunkView::new(world_id, 0, 3, ChunkPosition::new(0, 0));
        let (epoch, revision) = {
            let data = view.read();
            (data.epoch, data.view_revision)
        };

        let overlap = ChunkKey::new(0, ChunkPosition::new(0, 0));
        {
            let mut data = view.write();
            data.desired_chunks.insert(overlap);
            data.in_flight.insert(overlap, epoch);
            assert!(data.update_center(ChunkPosition::new(1, 0)));
            assert!(!data.update_center(ChunkPosition::new(1, 0)));
        }
        let data = view.read();
        assert_eq!(data.epoch, epoch);
        assert_eq!(data.view_revision, revision + 1);
        assert_eq!(data.in_flight.get(&overlap), Some(&epoch));
        assert!(view.context_is_current(epoch));
        assert!(view.accepts(epoch, &overlap));
    }

    #[test]
    fn radius_update_preserves_context_and_used_overlap() {
        let world_id = MinecraftWorldId::random();
        let view = ChunkView::new(world_id, 0, 1, ChunkPosition::new(0, 0));
        let overlap = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let (epoch, revision) = {
            let mut data = view.write();
            data.used_chunks.insert(overlap);
            let epoch = data.epoch;
            let revision = data.view_revision;
            data.in_flight.insert(overlap, epoch);
            (epoch, revision)
        };

        assert!(view.write().update_view(2, ChunkPosition::new(0, 0)));
        let data = view.read();
        assert_eq!(data.epoch, epoch);
        assert_eq!(data.view_revision, revision + 1);
        assert_eq!(data.used_chunks.len(), 1);
        assert_eq!(data.desired_chunks.len(), 25);
        assert!(data.desired_chunks.contains(&overlap));
        assert!(data.in_flight.contains_key(&overlap));
        assert!(data.publisher_pending);
        assert_eq!(data.publisher_in_flight, None);
        assert_eq!(data.load_queue.len(), 0);
    }

    #[test]
    fn prepare_window_is_independent_of_the_delivery_budget() {
        let world_id = MinecraftWorldId::random();
        let mut settings = ChunkSendSettings::default();
        settings.chunks_per_tick = 4;
        settings.prepare_window = 12;
        let view = ChunkView::new(world_id, 0, 3, ChunkPosition::new(0, 0));
        {
            let mut data = view.write();
            data.update_view(3, ChunkPosition::new(0, 0));
            for x in -3..=3i32 {
                for z in -3..=3i32 {
                    let key = ChunkKey::new(0, ChunkPosition::new(x, z));
                    data.desired_chunks.insert(key);
                    data.load_queue.insert(((x * x + z * z) as u64, key));
                }
            }
        }

        // Preparation reserves up to `prepare_window` keys, not 4.
        let batch = view
            .write()
            .take_prepare_batch(settings.prepare_window(), 0);
        assert_eq!(batch.len(), 12);
        assert_eq!(view.read().load_queue.len(), 49 - 12);

        // With 10 already in flight only two more slots remain.
        let batch = view
            .write()
            .take_prepare_batch(settings.prepare_window(), 10);
        assert_eq!(batch.len(), 2);

        // And a saturated prepare window reserves nothing.
        assert!(view
            .write()
            .take_prepare_batch(
                settings.prepare_window(),
                settings.prepare_window() as usize
            )
            .is_empty());
    }

    #[test]
    fn prepare_window_never_deadlocks_below_the_delivery_budget() {
        let mut settings = ChunkSendSettings::default();
        settings.chunks_per_tick = 8;
        settings.prepare_window = 2;
        assert_eq!(
            settings.prepare_window(),
            8,
            "a smaller prepare window would starve delivery"
        );
    }

    #[test]
    fn unserved_views_age_and_served_views_reset() {
        let view = ChunkView::new(MinecraftWorldId::random(), 0, 2, ChunkPosition::new(0, 0));
        {
            let mut data = view.write();
            data.desired_chunks
                .insert(ChunkKey::new(0, ChunkPosition::new(0, 0)));
            data.load_queue
                .insert((0, ChunkKey::new(0, ChunkPosition::new(0, 0))));
        }
        assert_eq!(view.read().waiting_age(), 0);

        view.write().note_unserved_tick();
        view.write().note_unserved_tick();
        assert_eq!(view.read().waiting_age(), 2);

        let served = view.write().take_prepare_batch(4, 0);
        assert_eq!(served.len(), 1);
        let data = view.read();
        assert_eq!(data.waiting_age(), 0);
        assert_eq!(
            data.last_served_waiting_ticks, 2,
            "the served age is recorded for diagnostics"
        );
    }

    #[test]
    fn a_hard_context_reset_clears_fairness_state() {
        let world_id = MinecraftWorldId::random();
        let view = ChunkView::new(world_id.clone(), 0, 2, ChunkPosition::new(0, 0));
        view.write().note_unserved_tick();
        assert_eq!(view.read().waiting_age(), 1);
        view.reset_subscription(world_id, 0, 2, ChunkPosition::new(0, 0));
        let data = view.read();
        assert_eq!(data.waiting_age(), 0);
        assert_eq!(data.last_served_waiting_ticks, 0);
    }

    #[test]
    fn delta_version_gaps_are_counted_and_bounded() {
        let view = ChunkView::new(MinecraftWorldId::random(), 0, 2, ChunkPosition::new(0, 0));
        let first = ChunkKey::new(0, ChunkPosition::new(1, 0));
        let second = ChunkKey::new(0, ChunkPosition::new(2, 0));

        assert!(view.write().note_delta_gap(first));
        assert!(
            !view.write().note_delta_gap(first),
            "an already tracked column must not re-report"
        );
        assert!(view.write().note_delta_gap(second));
        {
            let data = view.read();
            assert_eq!(data.delta_gap_column_count(), 2);
            assert_eq!(data.delta_gap_total(), 3, "the counter stays exact");
        }

        // The tracked set is bounded; the counter is not. (Read guards are
        // released first: parking_lot write locks are not reentrant.)
        for index in 0..(MAX_TRACKED_DELTA_GAP_COLUMNS + 16) {
            view.write()
                .note_delta_gap(ChunkKey::new(0, ChunkPosition::new(100 + index as i32, 0)));
        }
        let data = view.read();
        assert!(
            data.delta_gap_column_count() <= MAX_TRACKED_DELTA_GAP_COLUMNS,
            "gap tracking must not grow without bound"
        );
        assert!(data.delta_gap_total() > 3);
    }

    #[test]
    fn a_hard_context_reset_clears_delta_gap_evidence() {
        let world_id = MinecraftWorldId::random();
        let view = ChunkView::new(world_id.clone(), 0, 2, ChunkPosition::new(0, 0));
        view.write()
            .note_delta_gap(ChunkKey::new(0, ChunkPosition::new(1, 0)));
        assert_eq!(view.read().delta_gap_total(), 1);
        view.reset_subscription(world_id, 1, 2, ChunkPosition::new(5, 5));
        let data = view.read();
        assert_eq!(data.delta_gap_column_count(), 0);
        assert_eq!(data.delta_gap_total(), 0);
    }

    #[test]
    fn stale_publisher_ticket_cannot_clear_a_new_view_update() {
        let view = ChunkView::new(MinecraftWorldId::random(), 0, 2, ChunkPosition::new(0, 0));
        let first = view
            .write()
            .take_publisher_ticket()
            .expect("initial ticket");
        view.write().update_view(3, ChunkPosition::new(0, 0));
        view.complete_publisher_ticket(first, true);
        assert!(view.read().publisher_pending);

        let second = view
            .write()
            .take_publisher_ticket()
            .expect("updated ticket");
        assert_ne!(first, second);
        view.complete_publisher_ticket(first, true);
        assert_eq!(view.read().publisher_in_flight, Some(second));
        view.complete_publisher_ticket(second, true);
        assert!(!view.read().publisher_pending);
    }

    #[test]
    fn hard_context_reset_notifies_load_consumers() {
        let world_id = MinecraftWorldId::random();
        let view = ChunkView::new(world_id.clone(), 0, 3, ChunkPosition::new(0, 0));
        let mut receiver = view.context_epoch_receiver();
        let new_epoch = view.reset_subscription(world_id, 1, 3, ChunkPosition::new(4, 4));

        assert!(receiver
            .has_changed()
            .expect("context channel remains open"));
        assert_eq!(*receiver.borrow_and_update(), new_epoch);
    }

    #[test]
    fn payload_cache_evicts_by_owned_bytes_and_tracks_accounting() {
        let mut cache = LevelChunkCache::with_limits(10, 5);
        let world_id = MinecraftWorldId::random();
        let first =
            ChunkCacheKey::new(world_id.clone(), ChunkKey::new(0, ChunkPosition::new(0, 0)));
        let second = ChunkCacheKey::new(world_id, ChunkKey::new(0, ChunkPosition::new(1, 0)));
        let cached = |value| CachedChunk {
            payload: Arc::new(vec![value; 3]),
            subchunk_count: 1,
            generation: 0,
            incarnation: 1,
            wire_profile: 2168,
        };

        cache.insert(first.clone(), cached(1));
        cache.insert(second.clone(), cached(2));
        assert!(cache.get(&first).is_none());
        assert!(cache.get(&second).is_some());
        assert!(cache.payload_bytes() <= cache.max_payload_bytes());
        assert_eq!(
            cache.payload_bytes(),
            cache.get(&second).unwrap().payload.capacity()
        );

        cache.invalidate(&second);
        assert_eq!(cache.payload_bytes(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn payload_larger_than_budget_is_not_retained() {
        let mut cache = LevelChunkCache::with_limits(10, 2);
        let key = ChunkCacheKey::new(
            MinecraftWorldId::random(),
            ChunkKey::new(0, ChunkPosition::new(4, 5)),
        );
        cache.insert(
            key.clone(),
            CachedChunk {
                payload: Arc::new(vec![1; 3]),
                subchunk_count: 1,
                generation: 0,
                incarnation: 1,
                wire_profile: 2168,
            },
        );
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.payload_bytes(), 0);
    }

    #[test]
    fn reset_invalidates_old_epoch() {
        let world_id = MinecraftWorldId::random();
        let view = ChunkView::new(world_id.clone(), 0, 3, ChunkPosition::new(0, 0));
        let old_epoch = view.read().epoch;
        let key = ChunkKey::new(0, ChunkPosition::new(1, 1));
        view.write().desired_chunks.insert(key);
        let new_epoch = view.reset_subscription(world_id, 0, 3, ChunkPosition::new(4, 4));
        assert_ne!(old_epoch, new_epoch);
        assert!(!view.accepts(old_epoch, &key));
        assert!(!view.is_subscribed(&key));
    }

    /// Teleport/dimension reset must clear in-flight marks and the send lease: old async tasks
    /// cannot occupy the new subscription context after finishing.
    #[test]
    fn reset_clears_inflight_and_send_lease_for_teleport_cleanup() {
        let world_id = MinecraftWorldId::random();
        let view = ChunkView::new(world_id.clone(), 0, 3, ChunkPosition::new(0, 0));
        let epoch = view.read().epoch;
        let key = ChunkKey::new(0, ChunkPosition::new(2, -1));
        {
            let mut data = view.write();
            data.in_flight.insert(key, epoch);
            data.send_lease = Some(epoch);
            data.used_chunks.insert(key);
            data.chunks_sent = 12;
            data.equipment_container_sync_sent = true;
            data.has_spawn_chunks = true;
        }
        view.reset_subscription(world_id, 1, 3, ChunkPosition::new(9, 9));
        let data = view.read();
        assert!(data.in_flight.is_empty(), "reset must clear in_flight");
        assert!(data.send_lease.is_none(), "reset must release send_lease");
        assert!(data.used_chunks.is_empty(), "reset must clear used_chunks");
        assert_eq!(data.chunks_sent, 0, "reset must zero chunks_sent");
        assert!(
            !data.equipment_container_sync_sent,
            "reset must clear equipment sync flag"
        );
        assert!(!data.has_spawn_chunks, "reset must clear spawn flag");
        assert_eq!(data.dimension, 1, "reset must switch dimension");
    }
}
