//! Backpressure handling for outbound reliable facts.
//!
//! Two-level admission lets overwriteable state be rejected, while reliable
//! facts fail only when hard fact/byte caps are also exhausted.
//! This module gives the only legal handling for that terminal state:
//!
//! - block authority facts register an authoritative column refresh;
//! - inventory sync facts register a bounded player resync set, merged
//!   into one `ResyncInventory` next tick;
//! - other facts without an equivalent resync protocol record counts and
//!   rate-limited diagnostics, returning
//!   [`FactBackpressure::Unrecoverable`].
//!
//! Boundary: the game domain never touches protocol packets; it only
//! registers semantic resync requests.

use std::collections::BTreeSet;

use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_log::t_log;
use sc_world::chunk::ChunkPosition;
use sc_world::chunk_view::ChunkView;
use sc_world::manager::MinecraftWorldId;
use sc_world::storage::ChunkKey;

use crate::net::{IntentReliability, NetworkIntent, NetworkOutbox};
use crate::net_faults::{escalate_fact_to_connection_fault, PendingConnectionFaults};

/// Explicit handling when a reliable fact cannot enqueue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactBackpressure {
    /// Column refresh registered: the client receives the latest snapshot.
    ColumnRefreshRequested,
    /// Player inventory resync registered: full state overwrites rejected slots.
    InventoryResyncRequested,
    /// No resync path exists; the caller must record the terminal state.
    Unrecoverable,
}

/// Bounded player inventory resync request set (resource).
///
/// Rejected slot updates fold into one `ResyncInventory` per player: the
/// authoritative full state covers rejected slots, so the set only needs
/// bounded dedup.
#[derive(Resource, Clone, Debug)]
pub struct PendingInventoryResync {
    runtime_ids: BTreeSet<u64>,
    max_pending: usize,
    rejected: u64,
}

impl Default for PendingInventoryResync {
    fn default() -> Self {
        Self::with_limits(Self::DEFAULT_MAX_PENDING)
    }
}

impl PendingInventoryResync {
    pub const DEFAULT_MAX_PENDING: usize = 4096;

    pub fn with_limits(max_pending: usize) -> Self {
        Self {
            runtime_ids: BTreeSet::new(),
            max_pending: max_pending.max(1),
            rejected: 0,
        }
    }

    /// Register one authoritative inventory resync.
    pub fn request(&mut self, runtime_id: u64) -> bool {
        if self.runtime_ids.contains(&runtime_id) {
            return true;
        }
        if self.runtime_ids.len() >= self.max_pending {
            self.rejected = self.rejected.saturating_add(1);
            log::error!(
                "{}",
                t_log!(
                    "console.game.resync_limit",
                    max = self.max_pending,
                    entity = runtime_id,
                    total = self.rejected
                )
            );
            return false;
        }
        self.runtime_ids.insert(runtime_id);
        true
    }

    pub fn pending(&self) -> usize {
        self.runtime_ids.len()
    }

    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Drain current requests (the caller requeues).
    pub fn take_all(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.runtime_ids).into_iter().collect()
    }

    /// Requeue requests: consume on success, keep for later ticks on failure.
    pub fn retain_pending(&mut self, runtime_id: u64) {
        if !self.runtime_ids.contains(&runtime_id) {
            self.runtime_ids.insert(runtime_id);
        }
    }
}

/// Escalate rejected inventory facts to authoritative full resync.
pub fn escalate_inventory_fact(
    pending: Option<&mut PendingInventoryResync>,
    runtime_id: u64,
) -> FactBackpressure {
    let accepted = pending.is_some_and(|pending| pending.request(runtime_id));
    if accepted {
        FactBackpressure::InventoryResyncRequested
    } else {
        record_unrecoverable_fact("inventory sync");
        FactBackpressure::Unrecoverable
    }
}

/// Escalate rejected block authority facts to column refresh.
///
/// Registration failure (no matching view) is not an error: no subscribed
/// players means no view needs convergence.
pub fn escalate_block_fact(
    world: &World,
    world_id: &MinecraftWorldId,
    dimension: i32,
    position: ChunkPosition,
) -> FactBackpressure {
    if request_column_refresh(world, world_id, dimension, position) {
        FactBackpressure::ColumnRefreshRequested
    } else {
        FactBackpressure::Unrecoverable
    }
}

/// Unified outbound intent publisher: one entry point for admission.
///
/// All game-domain producers publish through it, so no call site can
/// forget to handle admission rejection:
///
/// - Overwriteable state rejections are safely ignored (regenerated).
/// - Rejected `UpdateBlock` registers column refresh.
/// - Rejected inventory sync registers bounded full resync.
/// - Other rejected reliable facts count as unrecoverable.
pub struct IntentPublisher<'a> {
    outbox: &'a mut NetworkOutbox,
    world: Option<&'a World>,
    pending_resync: Option<&'a mut PendingInventoryResync>,
    /// Terminal-state escalation for facts without an authoritative resync.
    faults: Option<&'a mut PendingConnectionFaults>,
}

impl<'a> IntentPublisher<'a> {
    pub fn new(outbox: &'a mut NetworkOutbox) -> Self {
        Self {
            outbox,
            world: None,
            pending_resync: None,
            faults: None,
        }
    }

    /// Allow escalating block facts to column refresh.
    pub fn with_world(mut self, world: &'a World) -> Self {
        self.world = Some(world);
        self
    }

    /// Allow escalating inventory facts to full resync.
    pub fn with_pending_resync(mut self, pending: &'a mut PendingInventoryResync) -> Self {
        self.pending_resync = Some(pending);
        self
    }

    /// Allow missing resources (inventory facts then have no fallback).
    pub fn maybe_with_pending_resync(
        mut self,
        pending: Option<&'a mut PendingInventoryResync>,
    ) -> Self {
        self.pending_resync = pending;
        self
    }

    /// Allow escalating facts without resync paths to isolation.
    pub fn with_faults(mut self, faults: &'a mut PendingConnectionFaults) -> Self {
        self.faults = Some(faults);
        self
    }

    /// Allow missing resources.
    pub fn maybe_with_faults(mut self, faults: Option<&'a mut PendingConnectionFaults>) -> Self {
        self.faults = faults;
        self
    }

    /// Whether current budget still admits a category.
    pub fn admits(&self, reliability: IntentReliability) -> bool {
        self.outbox.admits(reliability)
    }

    /// Publish one intent, handling admission rejection by category.
    pub fn publish(&mut self, intent: NetworkIntent) {
        // Judge budget with the same caps first; rejected intents stay owned
        // here for handling without extra clones.
        if self.outbox.admits_intent(&intent) {
            if let Err(error) = self.outbox.push(intent) {
                // Concurrent queue mutation is the only mismatch source here.
                log::error!("{}", t_log!("console.game.admission_mismatch", error = format!("{error:?}")));
            }
            return;
        }
        self.escalate(intent);
    }

    fn escalate(&mut self, intent: NetworkIntent) {
        let outcome = match intent {
            NetworkIntent::UpdateBlock {
                ref world_id,
                dimension,
                x,
                z,
                ..
            } => match self.world {
                Some(world) => {
                    escalate_block_fact(world, world_id, dimension, ChunkPosition::from_world(x, z))
                }
                None => {
                    record_unrecoverable_fact("UpdateBlock");
                    FactBackpressure::Unrecoverable
                }
            },
            NetworkIntent::UpdateInventorySlot { entity_id, .. }
            | NetworkIntent::ResyncInventory { entity_id } => {
                escalate_inventory_fact(self.pending_resync.as_deref_mut(), entity_id)
            }
            NetworkIntent::CorrectBlockPrediction {
                ref world_id,
                dimension,
                x,
                z,
                ..
            } => match self.world {
                Some(world) => {
                    escalate_block_fact(world, world_id, dimension, ChunkPosition::from_world(x, z))
                }
                None => {
                    record_unrecoverable_fact("CorrectBlockPrediction");
                    FactBackpressure::Unrecoverable
                }
            },
            ref other => {
                record_unrecoverable_fact(other.kind());
                // Terminal escalation: a fact with no authoritative resync path
                // leaves the client with state the server can no longer correct.
                // Isolate that connection instead of leaving it inconsistent.
                match intent_fault_runtime_id(other) {
                    Some(runtime_id) => {
                        escalate_fact_to_connection_fault(
                            self.faults.as_deref_mut(),
                            runtime_id,
                            other.kind(),
                        );
                    }
                    None => log::error!(
                        "{}",
                        t_log!("console.game.fact_no_path", kind = other.kind())
                    ),
                }
                FactBackpressure::Unrecoverable
            }
        };
        log::warn!(
            "{}",
            t_log!(
                "console.game.fact_rejected",
                kind = intent.kind(),
                outcome = format!("{outcome:?}")
            )
        );
    }
}

/// Register authoritative refresh for affected columns.
pub fn request_column_refresh(
    world: &World,
    world_id: &MinecraftWorldId,
    dimension: i32,
    position: ChunkPosition,
) -> bool {
    let key = ChunkKey::new(dimension, position);
    let mut refreshed = false;
    for entity in world.entities_with_component::<ChunkView>() {
        let matches_world = world
            .get_component::<MinecraftWorldId>(&entity)
            .is_some_and(|id| id.as_ref() == world_id);
        if !matches_world {
            continue;
        }
        let Some(view) = world.get_component::<ChunkView>(&entity) else {
            continue;
        };
        let mut data = view.write();
        // Only refresh subscribed columns with a sent baseline.
        if data.dimension != dimension {
            continue;
        }
        if data.delivery_ledger.contains_key(&key) || data.desired_chunks.contains(&key) {
            data.request_refresh(key);
            refreshed = true;
        }
    }
    refreshed
}

/// Record a reliable-fact terminal state with no resync path.
pub fn record_unrecoverable_fact(kind: &str) {
    UNRECOVERABLE.with(|counters| {
        let mut counters = counters.borrow_mut();
        counters.total = counters.total.saturating_add(1);
        if counters.total - counters.last_reported < UNRECOVERABLE_LOG_INTERVAL {
            return;
        }
        counters.last_reported = counters.total;
        log::error!(
            "{}",
            t_log!(
                "console.game.fact_lost",
                kind = kind,
                total = counters.total
            )
        );
    });
}

const UNRECOVERABLE_LOG_INTERVAL: u64 = 100;

#[derive(Default)]
struct UnrecoverableCounters {
    total: u64,
    last_reported: u64,
}

thread_local! {
    static UNRECOVERABLE: std::cell::RefCell<UnrecoverableCounters> =
        std::cell::RefCell::new(UnrecoverableCounters::default());
}

/// Requeue rejected inventory sync facts each tick ahead of producers,
/// preserving fact order.
pub fn flush_pending_inventory_resync(world: World) {
    let Some(mut pending) = world.get_resource_mut::<PendingInventoryResync>() else {
        return;
    };
    if pending.pending() == 0 {
        return;
    }
    let Some(mut outbox) = world.get_resource_mut::<crate::net::NetworkOutbox>() else {
        return;
    };
    for runtime_id in pending.take_all() {
        match outbox.push(NetworkIntent::ResyncInventory {
            entity_id: runtime_id,
        }) {
            Ok(()) => {}
            Err(error) => {
                // Budget still unavailable: keep requests for next tick.
                pending.retain_pending(runtime_id);
                log::debug!("[outbox] inventory resync for {runtime_id} deferred: {error:?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{IntentReliability, NetworkOutbox, OutboxPushError};
    use sc_ecs::world::World;
    use sc_world::chunk_view::AdmittedChunkBaseline;

    fn world_with_view(world_id: MinecraftWorldId, dimension: i32) -> (World, EntityIdAlias) {
        let world = World::new();
        let view = ChunkView::new(world_id.clone(), dimension, 2, ChunkPosition::new(0, 0));
        let entity = world.spawn((world_id, view.clone()));
        (world, entity)
    }

    type EntityIdAlias = sc_ecs::entity::EntityId;

    #[test]
    fn rejected_block_fact_requests_a_column_refresh_for_subscribed_views() {
        let world_id = MinecraftWorldId::random();
        let (world, entity) = world_with_view(world_id.clone(), 0);
        let view = world
            .get_component::<ChunkView>(&entity)
            .expect("chunk view");
        let key = ChunkKey::new(0, ChunkPosition::new(3, -4));
        {
            let mut data = view.write();
            data.desired_chunks.insert(key);
            data.used_chunks.insert(key);
            data.delivery_ledger.insert(
                key,
                AdmittedChunkBaseline {
                    incarnation: 1,
                    generation: 4,
                },
            );
        }

        assert!(request_column_refresh(
            &world,
            &world_id,
            0,
            ChunkPosition::new(3, -4)
        ));
        let data = view.read();
        assert!(data.refresh_required.contains(&key));
        assert!(
            data.load_queue.contains(&(25u64, key)),
            "a refreshed column must be re-queued by the planner"
        );
    }

    #[test]
    fn column_refresh_ignores_unsubscribed_or_foreign_contexts() {
        let world_id = MinecraftWorldId::random();
        let (world, entity) = world_with_view(world_id.clone(), 0);
        let view = world
            .get_component::<ChunkView>(&entity)
            .expect("chunk view");
        let unsubscribed = ChunkKey::new(0, ChunkPosition::new(9, 9));
        assert!(!request_column_refresh(
            &world,
            &world_id,
            0,
            ChunkPosition::new(9, 9)
        ));
        assert!(!view.read().refresh_required.contains(&unsubscribed));

        // Another world instance must not be refreshed.
        let key = ChunkKey::new(0, ChunkPosition::new(0, 0));
        view.write().used_chunks.insert(key);
        assert!(!request_column_refresh(
            &world,
            &MinecraftWorldId::random(),
            0,
            ChunkPosition::new(0, 0)
        ));
        assert!(view.read().refresh_required.contains(&key) == false);
    }

    #[test]
    fn publisher_escalates_block_facts_to_column_refresh() {
        let world_id = MinecraftWorldId::random();
        let (world, entity) = world_with_view(world_id.clone(), 0);
        let view = world
            .get_component::<ChunkView>(&entity)
            .expect("chunk view");
        let key = ChunkKey::new(0, ChunkPosition::new(1, 1));
        {
            let mut data = view.write();
            data.desired_chunks.insert(key);
            data.used_chunks.insert(key);
            data.delivery_ledger.insert(
                key,
                AdmittedChunkBaseline {
                    incarnation: 7,
                    generation: 2,
                },
            );
        }
        // A queue whose single entry slot is already taken is exactly the
        // saturation case under test: any further fact gets an explicit
        // rejection instead of evicting the queued one.
        let mut outbox = NetworkOutbox::with_limits(1, 1, usize::MAX);
        assert!(outbox
            .push(NetworkIntent::RemoveEntity { entity_id: 1 })
            .is_ok());

        IntentPublisher::new(&mut outbox)
            .with_world(&world)
            .publish(NetworkIntent::UpdateBlock {
                world_id: world_id.clone(),
                dimension: 0,
                incarnation: 7,
                generation: 3,
                x: 16,
                y: 64,
                z: 16,
                runtime_id: 1,
                flags: 0,
                layer: 0,
            });

        assert_eq!(
            outbox.admission_stats().fact_rejected,
            0,
            "rejected facts never reach push twice"
        );
        assert!(view.read().refresh_required.contains(&key));
    }

    #[test]
    fn publisher_escalates_inventory_facts_to_a_bounded_resync_request() {
        let mut outbox = NetworkOutbox::with_limits(1, 1, usize::MAX);
        assert!(outbox
            .push(NetworkIntent::RemoveEntity { entity_id: 1 })
            .is_ok());
        let mut pending = PendingInventoryResync::with_limits(2);

        IntentPublisher::new(&mut outbox)
            .with_pending_resync(&mut pending)
            .publish(NetworkIntent::UpdateInventorySlot {
                entity_id: 42,
                slot: 3,
                stack: sc_item::ItemStack::new(1, 2),
            });
        IntentPublisher::new(&mut outbox)
            .with_pending_resync(&mut pending)
            .publish(NetworkIntent::ResyncInventory { entity_id: 42 });

        assert_eq!(pending.pending(), 1, "same entity folds into one request");
        assert_eq!(outbox.pending(), 1, "only the pre-existing fact remains");
    }

    #[test]
    fn publisher_does_not_touch_the_queue_for_overwritable_state() {
        let mut outbox = NetworkOutbox::with_limits(1, 1, usize::MAX);
        assert!(outbox
            .push(NetworkIntent::RemoveEntity { entity_id: 1 })
            .is_ok());
        let world = World::new();
        IntentPublisher::new(&mut outbox)
            .with_world(&world)
            .publish(NetworkIntent::SetEntityMotion {
                entity_id: 5,
                x: 1.0,
                y: 0.0,
                z: 0.0,
            });

        assert_eq!(outbox.admission_stats().state_rejected, 0);
        assert_eq!(outbox.pending(), 1);
    }

    #[test]
    fn pending_inventory_resync_is_bounded_and_reports_overflow() {
        let mut pending = PendingInventoryResync::with_limits(2);
        assert!(pending.request(1));
        assert!(pending.request(2));
        assert!(!pending.request(3));
        assert!(!pending.request(4));
        assert_eq!(pending.rejected(), 2);
        assert_eq!(pending.pending(), 2);

        let taken = pending.take_all();
        assert_eq!(taken, vec![1, 2]);
        assert_eq!(pending.pending(), 0);
        assert!(pending.request(5));
    }

    #[test]
    fn flush_retries_deferred_resync_until_the_budget_allows_it() {
        let world = World::new();
        world.insert_resource(NetworkOutbox::with_limits(1, 1, usize::MAX));
        let mut pending = PendingInventoryResync::default();
        pending.request(77);
        world.insert_resource(pending);

        // The single entry slot is already taken by a fact that never leaves
        // the outbox in this test, so the request must survive the flush.
        world
            .get_resource_mut::<NetworkOutbox>()
            .expect("outbox")
            .push(NetworkIntent::RemoveEntity { entity_id: 1 })
            .expect("first fact");
        flush_pending_inventory_resync(world.clone());
        assert_eq!(
            world
                .get_resource::<PendingInventoryResync>()
                .expect("pending")
                .pending(),
            1
        );
        assert!(matches!(
            world
                .get_resource_mut::<NetworkOutbox>()
                .expect("outbox")
                .drain()
                .first(),
            Some(NetworkIntent::RemoveEntity { entity_id: 1 })
        ));
    }

    #[test]
    fn unrecoverable_entity_facts_isolate_the_affected_connection() {
        let mut outbox = NetworkOutbox::with_limits(1, 1, usize::MAX);
        assert!(outbox
            .push(NetworkIntent::RemoveEntity { entity_id: 1 })
            .is_ok());
        let mut faults = PendingConnectionFaults::with_limits(4);

        IntentPublisher::new(&mut outbox)
            .with_faults(&mut faults)
            .publish(NetworkIntent::DespawnItemEntity {
                world_id: MinecraftWorldId::random(),
                runtime_id: 77,
            });

        let requested = faults.take_all();
        assert_eq!(requested.len(), 1);
        assert_eq!(requested[0].runtime_id, 77);
        assert_eq!(requested[0].reason, "DespawnItemEntity");
    }

    #[test]
    fn block_and_inventory_facts_never_isolate_a_connection() {
        // These two classes have authoritative resync paths, so a saturated
        // outbox must never disconnect a player.
        let mut faults = PendingConnectionFaults::with_limits(4);
        let mut outbox = NetworkOutbox::with_limits(1, 1, usize::MAX);
        assert!(outbox
            .push(NetworkIntent::RemoveEntity { entity_id: 1 })
            .is_ok());
        IntentPublisher::new(&mut outbox)
            .with_world(&World::new())
            .with_faults(&mut faults)
            .publish(NetworkIntent::UpdateInventorySlot {
                entity_id: 5,
                slot: 0,
                stack: sc_item::ItemStack::new(1, 1),
            });
        assert_eq!(faults.pending(), 0, "inventory facts resync instead");
        assert_eq!(outbox.pending(), 1);
    }

    #[test]
    fn admits_matches_the_push_receipt() {
        let mut outbox = NetworkOutbox::with_limits(1, 1, usize::MAX);
        assert!(outbox.admits(IntentReliability::OverwritableState));
        assert!(outbox.admits(IntentReliability::ReliableFact));
        assert_eq!(
            outbox.push(NetworkIntent::RemoveEntity { entity_id: 1 }),
            Ok(())
        );
        assert!(!outbox.admits(IntentReliability::OverwritableState));
        assert!(!outbox.admits(IntentReliability::ReliableFact));
        assert_eq!(
            outbox.push(NetworkIntent::SetEntityMotion {
                entity_id: 2,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            Err(OutboxPushError::StateBudget)
        );
    }
}

/// The connection that must be isolated for an unrecoverable fact, if the
/// intent identifies one.
///
/// Entity lifecycle facts (`SpawnItemEntity`, `DespawnItemEntity`,
/// `UpdateItemStackSize`, `RemoveEntity`) carry the runtime id of the affected
/// entity; `TakeItemEntity` names the collector instead, because a stale ghost
/// item actor is the collector's problem.
pub fn intent_fault_runtime_id(intent: &NetworkIntent) -> Option<u64> {
    match intent {
        NetworkIntent::CraftResponse { entity_id, .. } => Some(*entity_id),
        NetworkIntent::CorrectBlockPrediction { entity_id, .. } => Some(*entity_id),
        NetworkIntent::OpenCraftingStation { entity_id, .. } => Some(*entity_id),
        NetworkIntent::RemoveEntity { entity_id }
        | NetworkIntent::SpawnItemEntity {
            runtime_id: entity_id,
            ..
        }
        | NetworkIntent::DespawnItemEntity {
            runtime_id: entity_id,
            ..
        }
        | NetworkIntent::UpdateItemStackSize {
            runtime_id: entity_id,
            ..
        }
        | NetworkIntent::TakeItemEntity {
            target_entity_id: entity_id,
            ..
        } => Some(*entity_id),
        _ => None,
    }
}
