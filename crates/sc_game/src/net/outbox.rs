//! Bounded outbound intent queue (resource on the root world).
//!
//! Game commands push and go; the network plugin drains the admissible
//! prefix each tick and encodes packets. Same-tick FIFO; cross-tick order
//! is not guaranteed.
//!
//! Admission tiers: overwriteable state fits the soft cap; reliable
//! facts may use the hard cap above it. Only when hard and byte caps are
//! also exhausted do reliable facts fail, and callers must roll back,
//! resync, or isolate explicitly, never silently drop.
//!
//! Queue policy (coalescing keys, lifecycle barriers) lives here next to
//! the queue — not in the intent definitions — so intent files stay purely
//! declarative. Adding a coalescible state means adding one arm to
//! [`coalesce_key`]; adding a lifecycle barrier means one arm to
//! [`is_coalescing_barrier`].

use super::intent::player::PlayerMoveMode;
use super::intent::{IntentReliability, NetworkIntent};
use sc_ecs::resource::Resource;
use std::collections::{HashMap, VecDeque};

/// Outbound intent queue (resource on the root world).
///
/// The two tiers keep reliable facts from being squeezed out while the
/// queue stays bounded.
#[derive(Resource, Clone, Debug)]
pub struct NetworkOutbox {
    entries: VecDeque<NetworkIntent>,
    /// Soft cap for overwriteable state.
    max_state_entries: usize,
    /// Hard cap for reliable facts (total queue entries).
    max_entries: usize,
    /// Structural self-size estimate (not an RSS hard cap).
    max_bytes: usize,
    estimated_bytes: usize,
    /// Coalescible key → queue sequence for O(1) replacement lookup.
    coalesced: HashMap<CoalesceKey, u64>,
    base_sequence: u64,
    next_sequence: u64,
    state_rejected: u64,
    fact_rejected: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum CoalesceKey {
    MovePlayer(u64),
    MoveEntity(u64),
    EntityMotion(u64),
    ItemMove(u128, u64),
    ItemStack(u128, u64),
    InventorySlot(u64, u8),
    FurnaceProgress(u64, u8),
}

impl Default for NetworkOutbox {
    fn default() -> Self {
        Self::with_limits(
            Self::DEFAULT_MAX_ENTRIES,
            Self::DEFAULT_MAX_FACT_ENTRIES,
            Self::DEFAULT_MAX_RETAINED_BYTES,
        )
    }
}

impl NetworkOutbox {
    /// Default soft cap for overwriteable state.
    pub const DEFAULT_MAX_ENTRIES: usize = 8192;
    /// Default hard cap for reliable facts (twice the soft cap).
    pub const DEFAULT_MAX_FACT_ENTRIES: usize = 16384;
    /// Default structural byte cap.
    pub const DEFAULT_MAX_RETAINED_BYTES: usize = 8 * 1024 * 1024;

    pub fn with_capacity(max_entries: usize) -> Self {
        let state_entries = max_entries.max(1);
        Self::with_limits(
            state_entries,
            state_entries.saturating_mul(2).max(state_entries + 1),
            Self::DEFAULT_MAX_RETAINED_BYTES,
        )
    }

    pub fn with_limits(max_state_entries: usize, max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_state_entries: max_state_entries.max(1),
            max_entries: max_entries.max(max_state_entries).max(1),
            max_bytes: max_bytes.max(1),
            estimated_bytes: 0,
            coalesced: HashMap::new(),
            base_sequence: 0,
            next_sequence: 0,
            state_rejected: 0,
            fact_rejected: 0,
        }
    }

    /// Admit one outbound intent.
    /// `Ok(())` means queued (or merged over same-key state).
    /// Errors mean budget exhaustion: callers must tell overwriteable
    /// state from reliable facts.
    pub fn push(&mut self, intent: NetworkIntent) -> Result<(), OutboxPushError> {
        if is_coalescing_barrier(&intent) {
            // State updates must not overwrite values across lifecycle or
            // context boundaries (spawn/remove/teleport/inventory reset).
            self.coalesced.clear();
        }
        if let Some(key) = coalesce_key(&intent) {
            if let Some(sequence) = self.coalesced.get(&key).copied() {
                let recent_enough = sequence >= self.base_sequence
                    && self.next_sequence.saturating_sub(sequence) <= 256;
                if recent_enough {
                    let offset = (sequence - self.base_sequence) as usize;
                    if let Some(existing) = self.entries.get_mut(offset) {
                        *existing = intent;
                        return Ok(());
                    }
                }
            }
        }

        let bytes = intent_estimated_bytes_for(&intent);
        let rejection = if !self.admits(intent.reliability())
            || self.estimated_bytes.saturating_add(bytes) > self.max_bytes
        {
            match intent.reliability() {
                IntentReliability::ReliableFact => {
                    self.fact_rejected = self.fact_rejected.saturating_add(1);
                    Some(OutboxPushError::FactBudget)
                }
                IntentReliability::OverwritableState => {
                    self.state_rejected = self.state_rejected.saturating_add(1);
                    Some(OutboxPushError::StateBudget)
                }
            }
        } else {
            None
        };
        if rejection.is_some() {
            log::debug!(
                "[network] outbox admission rejected {} (pending={} state_budget={} fact_budget={} bytes={}/{})",
                intent.kind(),
                self.entries.len(),
                self.max_state_entries,
                self.max_entries,
                self.estimated_bytes,
                self.max_bytes
            );
            return Err(rejection.expect("rejection recorded above"));
        }

        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("network outbox sequence exhausted");
        if let Some(key) = coalesce_key(&intent) {
            self.coalesced.insert(key, sequence);
        }
        self.entries.push_back(intent);
        self.estimated_bytes = self.estimated_bytes.saturating_add(bytes);
        Ok(())
    }

    /// Whether the current budget still admits a category.
    /// Uses the same caps as `push`, so rejected reliable facts can pick
    /// explicit handling without moving intents.
    pub fn admits(&self, reliability: IntentReliability) -> bool {
        let within_bytes = self
            .estimated_bytes
            .saturating_add(intent_estimated_bytes())
            <= self.max_bytes;
        match reliability {
            // Reliable facts may occupy the reserved headroom above the
            // overwritable-state budget; they are never evicted to make room.
            IntentReliability::ReliableFact => {
                self.entries.len() < self.max_entries && within_bytes
            }
            IntentReliability::OverwritableState => {
                self.entries.len() < self.max_state_entries && within_bytes
            }
        }
    }

    /// Queued entry count (including undrained entries).
    pub fn pending(&self) -> usize {
        self.entries.len()
    }

    pub fn admits_intent(&self, intent: &NetworkIntent) -> bool {
        self.admits(intent.reliability())
            && self
                .estimated_bytes
                .saturating_add(intent_estimated_bytes_for(intent))
                <= self.max_bytes
    }

    pub fn prefix_estimated_bytes(&self, count: usize) -> usize {
        self.entries
            .iter()
            .take(count)
            .map(intent_estimated_bytes_for)
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Structural self-size estimate (not RSS).
    pub fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub fn max_state_entries(&self) -> usize {
        self.max_state_entries
    }

    /// Cumulative admission rejections (metrics only).
    pub fn admission_stats(&self) -> OutboxAdmissionStats {
        OutboxAdmissionStats {
            state_rejected: self.state_rejected,
            fact_rejected: self.fact_rejected,
            pending: self.entries.len(),
        }
    }

    /// Admissible prefix length within entry/byte budgets.
    /// Returns at least 1 for a non-empty queue so one oversize intent
    /// cannot stall progress forever.
    pub fn admitted_prefix_len(&self, max_entries: usize, max_bytes: usize) -> usize {
        let entry_limit = max_entries.max(1);
        let entry_bytes = intent_estimated_bytes();
        let byte_limit = if max_bytes == 0 {
            entry_bytes
        } else {
            max_bytes
        };
        let mut taken = 0usize;
        let mut bytes = 0usize;
        for intent in self.entries.iter().take(entry_limit) {
            let next = bytes.saturating_add(intent_estimated_bytes_for(intent));
            if taken > 0 && next > byte_limit {
                break;
            }
            bytes = next;
            taken += 1;
        }
        debug_assert_eq!(entry_bytes, intent_estimated_bytes());
        taken
    }

    /// Drain the admissible prefix within budgets; undrained intents
    /// stay for later ticks. Callers must hold send quotas first so
    /// drained-but-unsendable facts never silently drop.
    pub fn take_admitted_prefix(
        &mut self,
        max_entries: usize,
        max_bytes: usize,
    ) -> Vec<NetworkIntent> {
        let count = self.admitted_prefix_len(max_entries, max_bytes);
        if count == 0 {
            return Vec::new();
        }
        let mut taken = Vec::with_capacity(count);
        for _ in 0..count {
            let Some(intent) = self.entries.pop_front() else {
                break;
            };
            if let Some(key) = coalesce_key(&intent) {
                if self.coalesced.get(&key) == Some(&self.base_sequence) {
                    self.coalesced.remove(&key);
                }
            }
            self.base_sequence = self
                .base_sequence
                .checked_add(1)
                .expect("network outbox sequence exhausted");
            self.estimated_bytes = self
                .estimated_bytes
                .saturating_sub(intent_estimated_bytes_for(&intent));
            taken.push(intent);
        }
        taken
    }

    /// Drain everything (tests and shutdown diagnostics).
    pub fn drain(&mut self) -> Vec<NetworkIntent> {
        let entries = self.entries.drain(..).collect();
        self.coalesced.clear();
        self.base_sequence = 0;
        self.next_sequence = 0;
        self.estimated_bytes = 0;
        entries
    }
}

/// Outbox admission rejection cause (terminal receipt of push).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboxPushError {
    /// Overwriteable state rejected: the queue is full with nothing to
    /// evict. Callers may safely ignore; state regenerates next tick.
    StateBudget,
    /// Reliable fact rejected: all caps exhausted. Callers must roll
    /// back, resync, or isolate explicitly, never silently drop.
    FactBudget,
}

/// Outbox admission counters (observability).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OutboxAdmissionStats {
    pub state_rejected: u64,
    pub fact_rejected: u64,
    pub pending: usize,
}

/// Per-intent structural byte estimate (variable payloads add up).
/// All current variants hold fixed `Copy` data, so the estimate equals
/// struct size; heap-carrying variants must count real owned buffers.
pub fn intent_estimated_bytes_for(intent: &NetworkIntent) -> usize {
    intent_estimated_bytes()
        + match intent {
            NetworkIntent::CraftResponse(payload) => {
                payload.slots.capacity()
                    * std::mem::size_of::<crate::inventory::craft_inventory::CraftSlotUpdate>()
            }
            _ => 0,
        }
}

/// Per-intent estimated bytes (constant part).
pub fn intent_estimated_bytes() -> usize {
    std::mem::size_of::<NetworkIntent>()
}

fn coalesce_key(intent: &NetworkIntent) -> Option<CoalesceKey> {
    match intent {
        NetworkIntent::MovePlayer(payload) if payload.mode == PlayerMoveMode::Normal => {
            Some(CoalesceKey::MovePlayer(payload.entity_id))
        }
        NetworkIntent::MoveEntityAbsolute(payload) => {
            Some(CoalesceKey::MoveEntity(payload.entity_id))
        }
        NetworkIntent::SetEntityMotion(payload) => {
            Some(CoalesceKey::EntityMotion(payload.entity_id))
        }
        NetworkIntent::MoveItemEntity(payload) => Some(CoalesceKey::ItemMove(
            payload.world_id.world_id.as_u128(),
            payload.runtime_id,
        )),
        NetworkIntent::UpdateItemStackSize(payload) => Some(CoalesceKey::ItemStack(
            payload.world_id.world_id.as_u128(),
            payload.runtime_id,
        )),
        NetworkIntent::UpdateInventorySlot(payload) => {
            Some(CoalesceKey::InventorySlot(payload.entity_id, payload.slot))
        }
        NetworkIntent::SyncFurnaceProgress(payload) => {
            Some(CoalesceKey::FurnaceProgress(
                payload.entity_id,
                payload.window_id,
            ))
        }
        _ => None,
    }
}

fn is_coalescing_barrier(intent: &NetworkIntent) -> bool {
    match intent {
        NetworkIntent::MovePlayer(payload)
            if matches!(
                payload.mode,
                PlayerMoveMode::Reset | PlayerMoveMode::Teleport
            ) =>
        {
            true
        }
        NetworkIntent::RemoveEntity(_)
        | NetworkIntent::SpawnItemEntity(_)
        | NetworkIntent::DespawnItemEntity(_)
        | NetworkIntent::TakeItemEntity(_)
        | NetworkIntent::ResyncInventory(_)
        | NetworkIntent::OpenPlayerInventory(_)
        | NetworkIntent::ClosePlayerInventory(_)
        | NetworkIntent::CraftResponse(_)
        | NetworkIntent::HurtEntity(_)
        | NetworkIntent::SyncHealth(_)
        | NetworkIntent::OpenCraftingStation(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::intent::*;
    use super::*;
    use sc_world::manager::MinecraftWorldId;

    fn sound(x: f32) -> NetworkIntent {
        NetworkIntent::PlaySound(PlaySound {
            world_id: MinecraftWorldId::random(),
            cue: SoundCue::BlockBreak,
            data: 0,
            x,
            y: 0.0,
            z: 0.0,
        })
    }

    fn motion(entity_id: u64, x: f32) -> NetworkIntent {
        NetworkIntent::SetEntityMotion(SetEntityMotion {
            entity_id,
            x,
            y: 0.0,
            z: 0.0,
        })
    }

    fn fact(entity_id: u64) -> NetworkIntent {
        NetworkIntent::RemoveEntity(RemoveEntity { entity_id })
    }

    fn move_player(entity_id: u64, x: f32, mode: PlayerMoveMode) -> NetworkIntent {
        NetworkIntent::MovePlayer(MovePlayer {
            entity_id,
            x,
            y: 0.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: true,
            mode,
        })
    }

    #[test]
    fn overwritable_state_is_rejected_at_the_state_budget() {
        let mut outbox = NetworkOutbox::with_capacity(2);
        assert!(outbox.push(sound(1.0)).is_ok());
        assert!(outbox.push(sound(2.0)).is_ok());
        // The third presentation cue is refused, not swapped for an older one:
        // the already-queued state stays the authoritative pending set.
        assert_eq!(outbox.push(sound(3.0)), Err(OutboxPushError::StateBudget));
        assert_eq!(outbox.pending(), 2);
        let stats = outbox.admission_stats();
        assert_eq!(stats.state_rejected, 1);
        assert_eq!(stats.fact_rejected, 0);
        let intents = outbox.drain();
        assert_eq!(intents.len(), 2);
        assert!(matches!(
            intents[0],
            NetworkIntent::PlaySound(PlaySound {
                cue: SoundCue::BlockBreak,
                ..
            })
        ));
        assert!(matches!(
            intents[1],
            NetworkIntent::PlaySound(PlaySound {
                cue: SoundCue::BlockBreak,
                ..
            })
        ));
    }

    #[test]
    fn reliable_facts_use_the_reserved_headroom_and_are_never_evicted() {
        let mut outbox = NetworkOutbox::with_limits(2, 4, usize::MAX);
        let state = motion(9, 1.0);

        assert!(outbox.push(fact(1)).is_ok());
        assert!(outbox.push(fact(2)).is_ok());
        // State is refused at the soft budget...
        assert_eq!(
            outbox.push(state.clone()),
            Err(OutboxPushError::StateBudget)
        );
        // ...while facts still fit in the reserved headroom.
        assert!(outbox.push(fact(3)).is_ok());
        assert!(outbox.push(fact(4)).is_ok());
        // Hard fact budget exhausted: the caller receives an explicit receipt.
        assert_eq!(outbox.push(fact(5)), Err(OutboxPushError::FactBudget));
        assert_eq!(outbox.admission_stats().fact_rejected, 1);

        let intents = outbox.drain();
        let kept: Vec<u64> = intents
            .iter()
            .map(|intent| match intent {
                NetworkIntent::RemoveEntity(payload) => payload.entity_id,
                other => panic!("unexpected intent {other:?}"),
            })
            .collect();
        assert_eq!(kept, vec![1, 2, 3, 4], "no accepted fact is dropped");
    }

    #[test]
    fn byte_budget_refuses_both_classes_with_distinct_receipts() {
        let entry_bytes = intent_estimated_bytes();
        let mut outbox = NetworkOutbox::with_limits(64, 64, entry_bytes * 2);
        assert!(outbox.push(motion(1, 0.0)).is_ok());
        assert!(outbox.push(motion(2, 0.0)).is_ok());
        assert_eq!(outbox.estimated_bytes(), entry_bytes * 2);
        assert_eq!(
            outbox.push(NetworkIntent::PlayParticle(PlayParticle {
                world_id: MinecraftWorldId::random(),
                cue: ParticleCue::BlockBreak,
                data: 0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            })),
            Err(OutboxPushError::StateBudget)
        );
        assert_eq!(outbox.push(fact(3)), Err(OutboxPushError::FactBudget));
    }

    #[test]
    fn admitted_prefix_is_bounded_and_leaves_the_remainder_queued() {
        let mut outbox = NetworkOutbox::with_limits(64, 64, usize::MAX);
        for entity_id in 0..6u64 {
            assert!(outbox.push(fact(entity_id)).is_ok());
        }
        let entry_bytes = intent_estimated_bytes();

        let count = outbox.admitted_prefix_len(10, entry_bytes * 3);
        assert_eq!(count, 3);
        let taken = outbox.take_admitted_prefix(10, entry_bytes * 3);
        assert_eq!(taken.len(), 3);
        assert_eq!(outbox.pending(), 3, "unadmitted intents stay queued");
        assert_eq!(outbox.estimated_bytes(), entry_bytes * 3);
        assert!(matches!(
            outbox.entries.front(),
            Some(NetworkIntent::RemoveEntity(_))
        ));

        // An isolated oversize request still makes progress on one entry.
        assert_eq!(outbox.admitted_prefix_len(10, 1), 1);
    }

    #[test]
    fn takes_keep_the_coalescing_index_consistent() {
        let mut outbox = NetworkOutbox::with_limits(8, 8, usize::MAX);
        assert!(outbox.push(fact(1)).is_ok());
        assert!(outbox.push(motion(7, 1.0)).is_ok());
        assert!(outbox.push(motion(7, 2.0)).is_ok());

        let taken = outbox.take_admitted_prefix(1, usize::MAX);
        assert_eq!(taken.len(), 1);
        assert!(matches!(taken[0], NetworkIntent::RemoveEntity(_)));
        assert!(
            outbox.coalesced.contains_key(&CoalesceKey::EntityMotion(7)),
            "the motion key is still queued and must stay coalescible"
        );

        // The newest motion value still replaces the queued one in place.
        assert!(outbox.push(motion(7, 3.0)).is_ok());
        assert!(matches!(
            outbox.entries.front(),
            Some(NetworkIntent::SetEntityMotion(payload)) if payload.x == 3.0
        ));
    }

    #[test]
    fn intent_reliability_separates_state_from_facts() {
        let world_id = MinecraftWorldId::random();
        let state_kinds = [
            move_player(1, 0.0, PlayerMoveMode::Normal),
            NetworkIntent::MoveEntityAbsolute(MoveEntityAbsolute {
                entity_id: 1,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                yaw: 0.0,
                pitch: 0.0,
                head_yaw: 0.0,
                on_ground: true,
            }),
            motion(1, 0.0),
            sound(0.0),
            NetworkIntent::PlayParticle(PlayParticle {
                world_id: world_id.clone(),
                cue: ParticleCue::BlockBreak,
                data: 0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
            NetworkIntent::BlockBreakProgress(BlockBreakProgress {
                world_id: world_id.clone(),
                cue: BlockBreakProgressCue::Start,
                data: 0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }),
        ];
        let fact_kinds = [
            move_player(1, 0.0, PlayerMoveMode::Teleport),
            move_player(1, 0.0, PlayerMoveMode::Reset),
            fact(1),
            NetworkIntent::UpdateBlock(UpdateBlock {
                world_id: world_id.clone(),
                dimension: 0,
                incarnation: 1,
                generation: 1,
                x: 0,
                y: 0,
                z: 0,
                runtime_id: 1,
                flags: 0,
                layer: 0,
            }),
            NetworkIntent::SpawnItemEntity(SpawnItemEntity {
                world_id: world_id.clone(),
                runtime_id: 1,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                motion_x: 0.0,
                motion_y: 0.0,
                motion_z: 0.0,
                stack: sc_item::ItemStack::new(1, 1),
            }),
            NetworkIntent::DespawnItemEntity(DespawnItemEntity {
                world_id: world_id.clone(),
                runtime_id: 1,
            }),
            NetworkIntent::UpdateItemStackSize(UpdateItemStackSize {
                world_id: world_id.clone(),
                runtime_id: 1,
                count: 2,
            }),
            NetworkIntent::TakeItemEntity(TakeItemEntity {
                world_id: world_id.clone(),
                runtime_id: 1,
                target_entity_id: 2,
            }),
            NetworkIntent::UpdateInventorySlot(UpdateInventorySlot {
                entity_id: 1,
                slot: 0,
                stack: sc_item::ItemStack::new(1, 1),
            }),
            NetworkIntent::ResyncInventory(ResyncInventory { entity_id: 1 }),
            NetworkIntent::OpenPlayerInventory(OpenPlayerInventory { entity_id: 1 }),
            NetworkIntent::ClosePlayerInventory(ClosePlayerInventory {
                entity_id: 1,
                window_id: 0,
                container_type: -1,
            }),
        ];

        for intent in state_kinds {
            assert!(
                intent.is_overwritable_state(),
                "{} must be overwritable state",
                intent.kind()
            );
        }
        for intent in fact_kinds {
            assert!(
                !intent.is_overwritable_state(),
                "{} must be a reliable fact",
                intent.kind()
            );
        }
    }

    #[test]
    fn outbox_coalesces_latest_motion_state() {
        let mut outbox = NetworkOutbox::with_capacity(8);
        assert!(outbox.push(motion(7, 1.0)).is_ok());
        assert!(outbox.push(motion(7, 2.0)).is_ok());
        let intents = outbox.drain();
        assert_eq!(intents.len(), 1);
        assert!(matches!(
            intents[0],
            NetworkIntent::SetEntityMotion(ref payload) if payload.x == 2.0
        ));
    }

    #[test]
    fn outbox_coalescing_index_obeys_existing_recent_window() {
        let mut outbox = NetworkOutbox::with_capacity(400);
        assert!(outbox.push(motion(7, 1.0)).is_ok());
        for _ in 0..255 {
            assert!(outbox.push(sound(0.0)).is_ok());
        }
        assert!(outbox.push(motion(7, 2.0)).is_ok());
        assert_eq!(outbox.pending(), 256);
        assert!(matches!(
            outbox.entries.front(),
            Some(NetworkIntent::SetEntityMotion(payload)) if payload.x == 2.0
        ));

        let mut outbox = NetworkOutbox::with_capacity(400);
        assert!(outbox.push(motion(7, 1.0)).is_ok());
        for _ in 0..256 {
            assert!(outbox.push(sound(0.0)).is_ok());
        }
        assert!(outbox.push(motion(7, 2.0)).is_ok());
        assert_eq!(outbox.pending(), 258);
        assert!(matches!(
            outbox.entries.front(),
            Some(NetworkIntent::SetEntityMotion(payload)) if payload.x == 1.0
        ));
        assert!(matches!(
            outbox.entries.back(),
            Some(NetworkIntent::SetEntityMotion(payload)) if payload.x == 2.0
        ));
    }

    #[test]
    fn teleport_and_lifecycle_barriers_prevent_motion_coalescing() {
        let mut outbox = NetworkOutbox::with_capacity(8);
        assert!(outbox
            .push(move_player(7, 1.0, PlayerMoveMode::Normal))
            .is_ok());
        assert!(outbox
            .push(move_player(7, 2.0, PlayerMoveMode::Teleport))
            .is_ok());
        assert!(outbox
            .push(move_player(7, 3.0, PlayerMoveMode::Normal))
            .is_ok());

        let intents = outbox.drain();
        assert_eq!(intents.len(), 3);
        assert!(matches!(
            &intents[0],
            NetworkIntent::MovePlayer(payload)
                if payload.x == 1.0 && payload.mode == PlayerMoveMode::Normal
        ));
        assert!(matches!(
            &intents[1],
            NetworkIntent::MovePlayer(payload)
                if payload.x == 2.0 && payload.mode == PlayerMoveMode::Teleport
        ));
        assert!(matches!(
            &intents[2],
            NetworkIntent::MovePlayer(payload)
                if payload.x == 3.0 && payload.mode == PlayerMoveMode::Normal
        ));
    }

    #[test]
    fn outbox_coalescing_index_stays_consistent_after_prefix_take() {
        let mut outbox = NetworkOutbox::with_capacity(2);
        assert!(outbox.push(motion(7, 1.0)).is_ok());
        assert!(outbox.push(sound(0.0)).is_ok());
        // The queue never evicts: the state budget refuses instead, so the
        // coalescing key of a queued intent must survive a full queue.
        assert_eq!(outbox.push(sound(0.0)), Err(OutboxPushError::StateBudget));
        assert!(outbox.coalesced.contains_key(&CoalesceKey::EntityMotion(7)));
        assert!(outbox.push(motion(7, 2.0)).is_ok());
        assert!(outbox.push(motion(7, 3.0)).is_ok());
        assert_eq!(outbox.pending(), 2);
        // Coalescing keeps the newest value at the queued slot and preserves
        // the order of the still-queued intents.
        assert!(matches!(
            outbox.entries.front(),
            Some(NetworkIntent::SetEntityMotion(payload)) if payload.x == 3.0
        ));
        assert!(matches!(
            outbox.entries.back(),
            Some(NetworkIntent::PlaySound(_))
        ));
    }

    #[test]
    fn health_facts_are_reliable_never_coalesced_and_bar_motion() {
        let mut outbox = NetworkOutbox::with_capacity(8);
        assert!(outbox.push(motion(7, 1.0)).is_ok());
        assert!(outbox
            .push(NetworkIntent::SyncHealth(SyncHealth { entity_id: 7 }))
            .is_ok());
        assert!(outbox.push(motion(7, 2.0)).is_ok());
        assert!(outbox
            .push(NetworkIntent::HurtEntity(HurtEntity {
                entity_id: 7,
                fatal: false,
            }))
            .is_ok());

        let intents = outbox.drain();
        assert_eq!(intents.len(), 4);
        // Health facts are reliable (never StateBudget-refused here) and
        // never coalesced away: both motion updates survive around them.
        assert!(matches!(
            &intents[0],
            NetworkIntent::SetEntityMotion(p) if p.x == 1.0
        ));
        assert!(matches!(&intents[1], NetworkIntent::SyncHealth(_)));
        assert!(matches!(
            &intents[2],
            NetworkIntent::SetEntityMotion(p) if p.x == 2.0
        ));
        assert!(matches!(&intents[3], NetworkIntent::HurtEntity(_)));
        for intent in &intents[1..] {
            if matches!(
                intent,
                NetworkIntent::SyncHealth(_) | NetworkIntent::HurtEntity(_)
            ) {
                assert_eq!(intent.reliability(), IntentReliability::ReliableFact);
            }
        }
    }
}
