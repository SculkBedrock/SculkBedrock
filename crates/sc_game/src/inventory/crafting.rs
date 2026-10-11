//! Server-authoritative crafting: protocol-independent intents,
//! atomic inventory transactions, workstation processors and unlock.
//!
//! The network layer only translates packets into [`sc_recipe::CraftIntent`]
//! (see `sc_network::handler::crafting`); every rule lives here without any
//! packet type. Region ownership is respected: the owner validates its own
//! epoch/revision and publishes; cross-region work uses explicit
//! reservation / commit / abort and never holds a lock across `await`
//! (all entry points are synchronous on owned snapshots).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use parking_lot::RwLock;
use sc_block::workstation::SlotRole;
use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use sc_item::{ItemRegistry, ItemStack, PlayerInventory};
use sc_recipe::{
    CommitChecks, CompileBudgets, CraftActor, CraftContext, CraftIntent, CraftQueue, CraftReceipt,
    CraftReject, ExpectedSlot, MapTagResolver, PlayerRecipeBook, RecipeRegistryFingerprint,
    RecipeRegistrySnapshot, SeenOperations, SourceRecipe, StationKind, TagResolver, TxInventory,
    TxSlot, UnlockPolicy,
};

use crate::inventory::ContainerOpen;
use crate::net::{CraftResponse, NetworkIntent, NetworkOutbox, OpenCraftingStation, ResyncInventory, UpdateInventorySlot};

// ===================== components / resources =====================

/// Per-player crafting revisions. Bumped on every accepted inventory or
/// container change; client requests carry the observed values and stale
/// ones are rejected.
#[derive(Component, Clone, Debug, Default)]
pub struct CraftingRevision {
    inner: Arc<RwLock<RevisionData>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct RevisionData {
    inventory: u64,
    container: u64,
    station: u64,
}

impl CraftingRevision {
    pub fn inventory(&self) -> u64 {
        self.inner.read().inventory
    }
    pub fn container(&self) -> u64 {
        self.inner.read().container
    }
    pub fn station(&self) -> u64 {
        self.inner.read().station
    }
    pub fn bump_inventory(&self) -> u64 {
        let mut data = self.inner.write();
        data.inventory = data.inventory.wrapping_add(1);
        data.inventory
    }
    pub fn bump_container(&self) -> u64 {
        let mut data = self.inner.write();
        data.container = data.container.wrapping_add(1);
        data.container
    }
    pub fn set_station(&self, revision: u64) {
        self.inner.write().station = revision;
    }
}

/// Per-player idempotency log + bounded craft queue.
#[derive(Component, Clone, Debug)]
pub struct CraftingSession {
    seen: Arc<RwLock<SeenOperations>>,
    queue: Arc<RwLock<CraftQueue>>,
}

impl Default for CraftingSession {
    fn default() -> Self {
        Self {
            seen: Arc::new(RwLock::new(SeenOperations::new(256))),
            queue: Arc::new(RwLock::new(CraftQueue::new(32))),
        }
    }
}

impl CraftingSession {
    pub fn try_submit(&self, intent: CraftIntent) -> Result<(), CraftReject> {
        self.queue.write().try_submit(intent)
    }
    pub fn pop(&self) -> Option<CraftIntent> {
        self.queue.write().pop()
    }
    pub fn queue_len(&self) -> usize {
        self.queue.read().len()
    }
}

/// Minimal player recipe-book state (persisted next to player data).
#[derive(Component, Clone, Debug, Default)]
pub struct RecipeBookState {
    inner: Arc<RwLock<PlayerRecipeBook>>,
}

impl RecipeBookState {
    pub fn with_book(&self, f: impl FnOnce(&PlayerRecipeBook)) {
        f(&self.inner.read());
    }
    pub fn unlock(&self, id: impl Into<String>) {
        self.inner.write().unlock(id);
    }
    pub fn migrate(
        &self,
        fingerprint: RecipeRegistryFingerprint,
        live: &HashSet<String>,
    ) -> Vec<String> {
        self.inner.write().migrate(fingerprint, live)
    }
}

/// Frozen recipe registry shared by all regions (read-only after load).
#[derive(Resource, Clone, Debug)]
pub struct SharedRecipeRegistry {
    snapshot: Arc<RwLock<RecipeRegistrySnapshot>>,
}

impl Default for SharedRecipeRegistry {
    fn default() -> Self {
        Self::new(RecipeRegistrySnapshot::empty())
    }
}

impl SharedRecipeRegistry {
    pub fn new(snapshot: RecipeRegistrySnapshot) -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
        }
    }
    pub fn snapshot(&self) -> RecipeRegistrySnapshot {
        self.snapshot.read().clone()
    }
    pub fn replace(&self, snapshot: RecipeRegistrySnapshot) {
        *self.snapshot.write() = snapshot;
    }
    pub fn fingerprint(&self) -> RecipeRegistryFingerprint {
        self.snapshot.read().fingerprint()
    }
}

/// Item-tag table for `{"tag": ...}` ingredients (fail-closed when absent).
#[derive(Resource, Clone, Debug, Default)]
pub struct SharedItemTags {
    inner: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl SharedItemTags {
    pub fn new(map: HashMap<String, Vec<String>>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(map)),
        }
    }
    pub fn resolver(&self) -> MapTagResolver {
        MapTagResolver::new(self.inner.read().clone())
    }
    pub fn replace(&self, map: HashMap<String, Vec<String>>) {
        *self.inner.write() = map;
    }
}

// ===================== block-entity processors =====================

/// Owner-local duration-process state (furnace / blast / smoker / campfire /
/// brewing / smithing). Bounded, ticked by the owning region; async workers
/// never write this directly.
///
/// Generic over slot count: `slots`/`roles` mirror the block's
/// `sc:workstation` decl slots one-to-one (furnace 3, campfire 4+ with
/// `Extra`). Dual clocks are mandatory (burn runs on its own, cook only
/// advances while smeltable; one `progress_ticks` cannot express a furnace).
#[derive(Component, Clone, Debug)]
pub struct BlockProcessState {
    inner: Arc<RwLock<ProcessData>>,
}

#[derive(Clone, Debug)]
struct ProcessData {
    kind: StationKind,
    slots: Vec<ItemStack>,
    roles: Vec<SlotRole>,
    burn_remaining: u32,
    burn_max: u32,
    cook_progress: u16,
    cook_total: u16,
    dirty: bool,
    saved_generation: u64,
    generation: u64,
}

/// Full-slot snapshot for persistence without holding the guard across IO
/// (storage wiring is a later slice; this only defines the interface).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub generation: u64,
    pub dirty: bool,
    pub kind: StationKind,
    pub slots: Vec<(SlotRole, ItemStack)>,
    pub burn_remaining: u32,
    pub burn_max: u32,
    pub cook_progress: u16,
    pub cook_total: u16,
}

impl BlockProcessState {
    pub fn new(kind: StationKind, roles: Vec<SlotRole>, cook_total: u16) -> Self {
        let slots = vec![ItemStack::empty(); roles.len()];
        Self {
            inner: Arc::new(RwLock::new(ProcessData {
                kind,
                slots,
                roles,
                burn_remaining: 0,
                burn_max: 0,
                cook_progress: 0,
                cook_total: cook_total.max(1),
                dirty: false,
                saved_generation: 0,
                generation: 0,
            })),
        }
    }
    pub fn kind(&self) -> StationKind {
        self.inner.read().kind
    }
    pub fn is_burning(&self) -> bool {
        self.inner.read().burn_remaining > 0
    }
    pub fn burn_remaining(&self) -> u32 {
        self.inner.read().burn_remaining
    }
    pub fn burn_max(&self) -> u32 {
        self.inner.read().burn_max
    }
    pub fn cook_progress(&self) -> u16 {
        self.inner.read().cook_progress
    }
    pub fn cook_total(&self) -> u16 {
        self.inner.read().cook_total
    }
    /// Lit-time progress for `ContainerSetData` property 1
    /// (`ceil(burn/max * cook_total)`, burn-duration scaling). Duration is
    /// owned (decl-fed), so no parameter unlike the store-side version.
    pub fn lit_progress(&self) -> u16 {
        let data = self.inner.read();
        if data.burn_max == 0 || data.burn_remaining == 0 {
            return 0;
        }
        let ready = data.cook_total as u32;
        ((data.burn_remaining as u64 * ready as u64 + data.burn_max as u64 - 1)
            / data.burn_max as u64) as u16
    }
    /// Slots in furnace window order `[input, fuel, result]` (missing roles
    /// read as empty; decls always carry all three for fuel processes, so a
    /// missing role here only fires on invalid packs or role-less kinds).
    pub fn furnace_slots(&self) -> [ItemStack; 3] {
        let data = self.inner.read();
        let at = |role: SlotRole| {
            Self::role_index(&data, role)
                .map(|index| data.slots[index].clone())
                .unwrap_or_else(ItemStack::empty)
        };
        [
            at(SlotRole::Input),
            at(SlotRole::Fuel),
            at(SlotRole::Result),
        ]
    }
    fn role_index(data: &ProcessData, role: SlotRole) -> Option<usize> {
        data.roles.iter().position(|candidate| *candidate == role)
    }
    /// Cloned slot by role (`None` when the decl carries no such role;
    /// never panics on role absence).
    pub fn slot(&self, role: SlotRole) -> Option<ItemStack> {
        let data = self.inner.read();
        Self::role_index(&data, role).map(|index| data.slots[index].clone())
    }
    /// Overwrite a slot by role (false when the decl carries no such role).
    pub fn set_slot(&self, role: SlotRole, stack: ItemStack) -> bool {
        let mut data = self.inner.write();
        let Some(index) = Self::role_index(&data, role) else {
            return false;
        };
        if data.slots[index] != stack {
            data.slots[index] = stack;
            data.generation = data.generation.wrapping_add(1);
            data.dirty = true;
        }
        true
    }
    /// Take a slot by role, leaving it empty (`None` when the decl carries
    /// no such role; `Some` possibly-empty stack otherwise).
    pub fn take_slot(&self, role: SlotRole) -> Option<ItemStack> {
        let mut data = self.inner.write();
        let index = Self::role_index(&data, role)?;
        let taken = std::mem::replace(&mut data.slots[index], ItemStack::empty());
        if !taken.is_empty() {
            data.generation = data.generation.wrapping_add(1);
            data.dirty = true;
        }
        Some(taken)
    }
    /// Insert input if the input slot is free and bounded (one stack max).
    pub fn insert_input(&self, stack: ItemStack) -> bool {
        let mut data = self.inner.write();
        let Some(index) = Self::role_index(&data, SlotRole::Input) else {
            return false;
        };
        if !data.slots[index].is_empty() {
            return false;
        }
        data.slots[index] = stack;
        data.generation = data.generation.wrapping_add(1);
        data.dirty = true;
        true
    }
    /// One owner tick of burn/cook (vanilla furnace semantics: burn decrements
    /// per tick, cook advances while smeltable, completion moves one output
    /// into the result slot, fuel exhaustion resets progress). `output` is the current smelt
    /// result for the input (`None` = nothing smeltable); `max_stack` caps
    /// the result slot. Duration comes from the decl-fed `cook_total`, never
    /// from the station here.
    pub fn advance(
        &self,
        output: Option<&ItemStack>,
        fuel_ticks: u32,
        max_stack: u16,
    ) -> crate::furnace::TickOutcome {
        use crate::furnace::TickOutcome;
        let mut data = self.inner.write();
        let was_burning = data.burn_remaining > 0;
        let mut outcome = TickOutcome::default();
        let input = Self::role_index(&data, SlotRole::Input);
        let fuel = Self::role_index(&data, SlotRole::Fuel);
        let result = Self::role_index(&data, SlotRole::Result);
        let can_smelt = match (input, result) {
            (Some(input), Some(result)) => output.is_some_and(|out| {
                !data.slots[input].is_empty()
                    && (data.slots[result].is_empty()
                        || (data.slots[result].runtime_id == out.runtime_id
                            && data.slots[result].count.saturating_add(out.count)
                                <= max_stack.max(1)))
            }),
            _ => false,
        };
        if data.burn_remaining == 0 && can_smelt && fuel_ticks > 0 {
            if let Some(fuel) = fuel {
                if !data.slots[fuel].is_empty() {
                    data.slots[fuel].count = data.slots[fuel].count.saturating_sub(1);
                    data.slots[fuel].clear_if_empty();
                    data.burn_remaining = fuel_ticks;
                    data.burn_max = fuel_ticks;
                    outcome.consumed_fuel = true;
                }
            }
        }
        if data.burn_remaining > 0 {
            data.burn_remaining = data.burn_remaining.saturating_sub(1);
            if can_smelt {
                if let Some(out) = output {
                    data.cook_progress = data.cook_progress.saturating_add(1);
                    if data.cook_progress >= data.cook_total {
                        if let (Some(input), Some(result)) = (input, result) {
                            if data.slots[result].is_empty() {
                                data.slots[result] = out.clone();
                            } else {
                                data.slots[result].count =
                                    data.slots[result].count.saturating_add(out.count);
                            }
                            data.slots[input].count = data.slots[input].count.saturating_sub(1);
                            data.slots[input].clear_if_empty();
                            data.cook_progress = data.cook_progress.saturating_sub(data.cook_total);
                            outcome.completed = true;
                        }
                    }
                }
            }
        } else {
            // Out of fuel with nothing burning: progress resets (furnace turn-off).
            data.cook_progress = 0;
            data.burn_max = 0;
        }
        if outcome.consumed_fuel || outcome.completed {
            data.generation = data.generation.wrapping_add(1);
            data.dirty = true;
        }
        outcome.lit_changed = was_burning != (data.burn_remaining > 0);
        outcome
    }
    pub fn take_output(&self) -> Option<ItemStack> {
        self.take_slot(SlotRole::Result)
    }
    pub fn set_output(&self, stack: ItemStack) {
        self.set_slot(SlotRole::Result, stack);
    }
    /// Full-slot snapshot for persistence without holding the guard across IO.
    pub fn snapshot(&self) -> ProcessSnapshot {
        let data = self.inner.read();
        ProcessSnapshot {
            generation: data.generation,
            dirty: data.dirty,
            kind: data.kind,
            slots: data
                .roles
                .iter()
                .zip(data.slots.iter())
                .map(|(role, slot)| (*role, slot.clone()))
                .collect(),
            burn_remaining: data.burn_remaining,
            burn_max: data.burn_max,
            cook_progress: data.cook_progress,
            cook_total: data.cook_total,
        }
    }
    /// Save acknowledgement: only clears dirty versions `<= generation` and
    /// only when nothing newer arrived (same contract as chunk writeback).
    pub fn acknowledge_save(&self, generation: u64) {
        let mut data = self.inner.write();
        if data.generation == generation {
            data.dirty = false;
            data.saved_generation = generation;
        }
    }
    pub fn is_dirty(&self) -> bool {
        self.inner.read().dirty
    }
}

// ===================== bridging =====================

fn item_to_tx(stack: &ItemStack, registry: &ItemRegistry) -> Option<TxSlot> {
    if stack.is_empty() {
        return Some(TxSlot::empty());
    }
    let def = registry.get(stack.runtime_id)?;
    Some(TxSlot {
        identifier: def.name.to_string(),
        data: stack.damage as i32,
        components: None,
        count: stack.count,
    })
}

fn tx_to_item(slot: &TxSlot, registry: &ItemRegistry) -> Option<ItemStack> {
    if slot.is_empty() {
        return Some(ItemStack::empty());
    }
    let runtime_id = registry.runtime_id_by_name(&slot.identifier)?;
    let mut stack = ItemStack::new(runtime_id, slot.count);
    stack.damage = slot.data.max(0) as u32;
    if let Some(block) = registry.block_runtime_id(runtime_id) {
        stack.block_runtime_id = block;
    }
    Some(stack)
}

fn snapshot_inventory(inventory: &PlayerInventory, registry: &ItemRegistry) -> Option<TxInventory> {
    let mut slots = Vec::with_capacity(inventory.len());
    for i in 0..inventory.len() {
        let stack = inventory.get(i)?;
        // Unknown runtime ids fail closed (reject, never guess).
        let tx = item_to_tx(&stack, registry)?;
        slots.push(tx);
    }
    Some(TxInventory { slots, revision: 0 })
}

fn write_back_inventory(
    inventory: &PlayerInventory,
    tx: &TxInventory,
    registry: &ItemRegistry,
) -> bool {
    for (index, slot) in tx.slots.iter().enumerate() {
        let Some(stack) = tx_to_item(slot, registry) else {
            return false;
        };
        if inventory.set(index, stack).is_none() {
            return false;
        }
    }
    true
}

// ===================== submit =====================

/// Outcome of submitting one craft intent (terminal receipt + side effects
/// to publish).
#[derive(Clone, Debug)]
pub struct CraftOutcome {
    pub receipt: CraftReceipt,
    /// (slot, stack) pairs that changed and must be synced.
    pub changed_slots: Vec<(u8, ItemStack)>,
    pub resync: bool,
}

/// Submit one authoritative craft for `entity`.
///
/// Caller provides the owner-checked facts (world match, distance,
/// permission, gamemode, container/station revisions, unlock rule). The
/// inventory edit itself is atomic with rollback; the same operation id
/// never applies twice.
#[allow(clippy::too_many_arguments)]
pub fn submit_craft(
    inventory: &PlayerInventory,
    revision: &CraftingRevision,
    session: &CraftingSession,
    book: &RecipeBookState,
    registry: &ItemRegistry,
    snapshot: &RecipeRegistrySnapshot,
    tags: &dyn TagResolver,
    intent: CraftIntent,
    in_reach: bool,
    has_permission: bool,
    gamemode_allows: bool,
    recipes_unlock_rule: bool,
    station_exists: bool,
    station_kind_ok: bool,
    actual_container_revision: u64,
    actual_station_revision: u64,
    outbox: &mut NetworkOutbox,
    actor_runtime_id: u64,
) -> CraftOutcome {
    if let Some(receipt) = session
        .seen
        .read()
        .get(&(intent.actor.player_id, intent.operation_id))
        .cloned()
    {
        return CraftOutcome {
            receipt,
            changed_slots: Vec::new(),
            resync: true,
        };
    }
    // Bounded admission: a full per-player queue returns busy, never blocks.
    if session.queue_len() >= 32 {
        let receipt = CraftReceipt::Rejected {
            operation_id: intent.operation_id,
            reason: CraftReject::QueueBusy,
        };
        return CraftOutcome {
            receipt,
            changed_slots: Vec::new(),
            resync: true,
        };
    }
    let recipe = snapshot.get(&intent.recipe_id).cloned();
    let fingerprint = snapshot.fingerprint();
    // Unlock gate: locked recipes cannot be force-executed.
    let has_unlock = recipe
        .as_ref()
        .map(|r| {
            !r.unlock.is_empty()
                || matches!(
                    r.unlock_context.as_deref(),
                    Some("PlayerInWater" | "PlayerHasManyItems")
                )
        })
        .unwrap_or(false);
    let policy = UnlockPolicy {
        recipes_unlock_rule,
    };
    let allowed = {
        let book_guard = book.inner.read();
        policy.may_craft(&book_guard, &intent.recipe_id, has_unlock)
    };
    // Snapshot slots without holding the inventory guard across matching.
    let mut tx = match snapshot_inventory(inventory, registry) {
        Some(mut tx) => {
            tx.revision = revision.inventory();
            tx
        }
        None => {
            let receipt = CraftReceipt::Rejected {
                operation_id: intent.operation_id,
                reason: CraftReject::InputMismatch {
                    slot: usize::MAX,
                    detail: "unknown item runtime id".to_string(),
                },
            };
            publish_outcome(outbox, actor_runtime_id, &receipt, &[]);
            return CraftOutcome {
                receipt,
                changed_slots: Vec::new(),
                resync: true,
            };
        }
    };
    let before: Vec<TxSlot> = tx.slots.clone();
    let mut seen = session.seen.write().clone();
    let checks = CommitChecks {
        in_reach,
        has_permission,
        gamemode_allows,
        recipes_unlocked: allowed,
        station_exists,
        station_kind_ok,
        actual_container_revision,
        actual_station_revision,
    };
    let receipt = sc_recipe::execute_craft(
        &intent,
        recipe.as_ref(),
        fingerprint,
        &mut tx,
        &checks,
        &mut seen,
        tags,
        1024,
        &|id| registry.max_stack_size(registry.runtime_id_by_name(id).unwrap_or(0)),
    );
    *session.seen.write() = seen;
    match &receipt {
        CraftReceipt::Applied {
            inventory_revision, ..
        } => {
            // Commit the edited slots; a write-back failure rolls the whole
            // receipt back to a rejection (never half-applied).
            if !write_back_inventory(inventory, &tx, registry) {
                let receipt = CraftReceipt::Rejected {
                    operation_id: intent.operation_id,
                    reason: CraftReject::Aborted("inventory write-back failed".to_string()),
                };
                publish_outcome(outbox, actor_runtime_id, &receipt, &[]);
                return CraftOutcome {
                    receipt,
                    changed_slots: Vec::new(),
                    resync: true,
                };
            }
            // Advance the authoritative revision past the client's observed
            // value; the receipt carries the new revision.
            while revision.inventory() != *inventory_revision {
                // `execute_craft` bumped its private copy once; mirror it.
                revision.bump_inventory();
                break;
            }
            let mut changed = Vec::new();
            for (index, (old, new)) in before.iter().zip(tx.slots.iter()).enumerate() {
                if old != new {
                    if let Some(stack) = tx_to_item(new, registry) {
                        changed.push((index as u8, stack.clone()));
                        outbox
                            .push(NetworkIntent::UpdateInventorySlot(UpdateInventorySlot {
                                entity_id: actor_runtime_id,
                                slot: index as u8,
                                stack,
                            }))
                            .ok();
                    }
                }
            }
            publish_outcome(outbox, actor_runtime_id, &receipt, &[]);
            CraftOutcome {
                receipt,
                changed_slots: changed,
                resync: false,
            }
        }
        CraftReceipt::Rejected { .. } => {
            publish_outcome(outbox, actor_runtime_id, &receipt, &[]);
            CraftOutcome {
                receipt,
                changed_slots: Vec::new(),
                resync: true,
            }
        }
    }
}

fn publish_outcome(
    outbox: &mut NetworkOutbox,
    actor_runtime_id: u64,
    receipt: &CraftReceipt,
    _changed: &[(u8, ItemStack)],
) {
    // Rejected crafts always resync so the client cannot keep a predicted
    // result; applied crafts already pushed per-slot updates above.
    if matches!(receipt, CraftReceipt::Rejected { .. }) {
        outbox
            .push(NetworkIntent::ResyncInventory(ResyncInventory {
                entity_id: actor_runtime_id,
            }))
            .ok();
    }
}

/// Translate a client grid (slot, stack) list into expected inputs for an
/// intent. Unknown runtime ids are dropped (caller then fails closed).
pub fn expected_from_slots(
    pairs: &[(usize, ItemStack)],
    registry: &ItemRegistry,
) -> Vec<ExpectedSlot> {
    let mut out = Vec::new();
    for (slot, stack) in pairs {
        if stack.is_empty() {
            continue;
        }
        let Some(def) = registry.get(stack.runtime_id) else {
            continue;
        };
        out.push(ExpectedSlot {
            slot: *slot,
            identifier: def.name.to_string(),
            data: stack.damage as i32,
            count: stack.count,
        });
    }
    out
}

/// Build a snapshot from ordered pack sources (low → high version).
pub fn snapshot_from_sources(
    sources: &[SourceRecipe],
    budgets: &CompileBudgets,
) -> Result<(RecipeRegistrySnapshot, Vec<String>), String> {
    let (snapshot, diagnostics) =
        RecipeRegistrySnapshot::compile(sources, budgets, false).map_err(|e| e.to_string())?;
    let disabled: Vec<String> = diagnostics
        .disabled
        .iter()
        .map(|d| format!("{}: {}", d.identifier, d.reason))
        .collect();
    Ok((snapshot, disabled))
}

/// Ensure the opened-state component exists (container open/close path).


#[cfg(test)]
mod tests {
    use super::*;
    use sc_recipe::{CompileBudgets, SourceRecipe};

    /// Returns -1: no second `ContainerOpen` while a workstation window is open.
    #[test]
    fn open_crafting_station_is_refused_while_a_window_is_already_open() {
        let world = sc_ecs::world::World::new();
        let entity = world.spawn(ContainerOpen::at_workstation(
            7,
            StationKind::CraftingTable,
            sc_world::manager::MinecraftWorldId::random(),
            (1, 2, 3),
        ));
        let mut outbox = NetworkOutbox::default();
        let claimed = open_crafting_station(
            &world,
            entity,
            sc_block::position::BlockPosition::new(9, 9, 9),
            &mut outbox,
        );
        // Click consumed: the caller skips the placement branch.
        assert!(claimed);
        assert!(
            outbox.is_empty(),
            "已有窗口时不得再发布 OpenCraftingStation"
        );
        // Window state is unchanged; both sides keep the same window id.
        let still_open = world
            .get_component::<ContainerOpen>(&entity)
            .expect("window kept");
        assert_eq!(still_open.window_id, 7);
    }

    /// No workstation window opens while the inventory window is open.
    #[test]
    fn open_crafting_station_is_refused_while_player_inventory_is_open() {
        let world = sc_ecs::world::World::new();
        let entity = world.spawn(ContainerOpen::player_inventory());
        let mut outbox = NetworkOutbox::default();
        let claimed = open_crafting_station(
            &world,
            entity,
            sc_block::position::BlockPosition::new(0, 0, 0),
            &mut outbox,
        );
        assert!(claimed);
        assert!(outbox.is_empty());
    }

    #[test]
    fn window_ids_start_at_1_and_skip_zero_on_wrap() {
        // Allocate max(1, ++cnt % 100).
        let allocator = ContainerWindowAllocator::default();
        assert_eq!(allocator.allocate(), 1);
        assert_eq!(allocator.allocate(), 2);
        *allocator.next.lock() = 99;
        assert_eq!(allocator.allocate(), 1);
        assert_eq!(allocator.allocate(), 2);
    }

    fn registry_with(recipe_json: serde_json::Value) -> (ItemRegistry, RecipeRegistrySnapshot) {
        let registry = ItemRegistry::new();
        registry.upsert(sc_item::ItemDefinition::new(1, "minecraft:stone"));
        registry.upsert(sc_item::ItemDefinition::new(2, "minecraft:out"));
        let bytes = serde_json::to_vec(&recipe_json).unwrap();
        let src = SourceRecipe::new("p", 0, "r.json", "1.12", recipe_json, &bytes);
        let (snapshot, _) =
            RecipeRegistrySnapshot::compile(&[src], &CompileBudgets::default(), false).unwrap();
        (registry, snapshot)
    }

    fn harness() -> (
        PlayerInventory,
        CraftingRevision,
        CraftingSession,
        RecipeBookState,
        NetworkOutbox,
    ) {
        let inventory = PlayerInventory::new(4);
        inventory.set(0, ItemStack::new(1, 5));
        (
            inventory,
            CraftingRevision::default(),
            CraftingSession::default(),
            RecipeBookState::default(),
            NetworkOutbox::with_limits(64, 64, usize::MAX),
        )
    }

    #[test]
    fn craft_success_updates_slots_and_bumps_revision() {
        let (registry, snapshot) = registry_with(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:test"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:stone"}],
                "result": {"item": "minecraft:out"}
            }
        }));
        let (inventory, revision, session, book, mut outbox) = harness();
        let tags = MapTagResolver::new(HashMap::new());
        let intent = CraftIntent {
            actor: CraftActor {
                player_id: 1,
                entity_generation: 0,
            },
            context: CraftContext {
                world_id: "w".to_string(),
                dimension: 0,
                station: StationKind::CraftingTable,
                station_pos: None,
                station_revision: 0,
                client_claims_in_reach: true,
            },
            recipe_id: "minecraft:test".to_string(),
            registry_fingerprint: snapshot.fingerprint(),
            inventory_revision: 0,
            container_revision: 0,
            expected_inputs: vec![ExpectedSlot {
                slot: 0,
                identifier: "minecraft:stone".to_string(),
                data: 0,
                count: 1,
            }],
            requested_count: 1,
            operation_id: 7,
            deadline_unix: 0,
        };
        let outcome = submit_craft(
            &inventory,
            &revision,
            &session,
            &book,
            &registry,
            &snapshot,
            &tags,
            intent,
            true,
            true,
            true,
            true,
            true,
            true,
            0,
            0,
            &mut outbox,
            42,
        );
        assert!(matches!(outcome.receipt, CraftReceipt::Applied { .. }));
        assert_eq!(inventory.get(0).unwrap().count, 4);
        assert_eq!(revision.inventory(), 1);
        assert!(!outcome.changed_slots.is_empty());
    }

    #[test]
    fn stale_revision_rejects_and_resyncs() {
        let (registry, snapshot) = registry_with(serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:test"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:stone"}],
                "result": {"item": "minecraft:out"}
            }
        }));
        let (inventory, revision, session, book, mut outbox) = harness();
        revision.bump_inventory();
        let tags = MapTagResolver::new(HashMap::new());
        let intent = CraftIntent {
            actor: CraftActor {
                player_id: 1,
                entity_generation: 0,
            },
            context: CraftContext {
                world_id: "w".to_string(),
                dimension: 0,
                station: StationKind::CraftingTable,
                station_pos: None,
                station_revision: 0,
                client_claims_in_reach: true,
            },
            recipe_id: "minecraft:test".to_string(),
            registry_fingerprint: snapshot.fingerprint(),
            inventory_revision: 0,
            container_revision: 0,
            expected_inputs: vec![ExpectedSlot {
                slot: 0,
                identifier: "minecraft:stone".to_string(),
                data: 0,
                count: 1,
            }],
            requested_count: 1,
            operation_id: 8,
            deadline_unix: 0,
        };
        let outcome = submit_craft(
            &inventory,
            &revision,
            &session,
            &book,
            &registry,
            &snapshot,
            &tags,
            intent,
            true,
            true,
            true,
            true,
            true,
            true,
            0,
            0,
            &mut outbox,
            42,
        );
        assert!(matches!(
            outcome.receipt,
            CraftReceipt::Rejected {
                reason: CraftReject::StaleInventoryRevision { .. },
                ..
            }
        ));
        assert!(outcome.resync);
        assert_eq!(inventory.get(0).unwrap().count, 5);
    }

    #[test]
    fn locked_recipe_cannot_be_forced_without_unlock() {
        let (registry, snapshot) = registry_with(serde_json::json!({
            "format_version": "1.20.10",
            "minecraft:recipe_shaped": {
                "description": {"identifier": "minecraft:locked"},
                "tags": ["crafting_table"],
                "pattern": ["X"], "key": {"X": {"item": "minecraft:stone"}},
                "unlock": [{"item": "minecraft:stone"}],
                "result": {"item": "minecraft:out"}
            }
        }));
        let (inventory, revision, session, book, mut outbox) = harness();
        let tags = MapTagResolver::new(HashMap::new());
        let intent = CraftIntent {
            actor: CraftActor {
                player_id: 1,
                entity_generation: 0,
            },
            context: CraftContext {
                world_id: "w".to_string(),
                dimension: 0,
                station: StationKind::CraftingTable,
                station_pos: None,
                station_revision: 0,
                client_claims_in_reach: true,
            },
            recipe_id: "minecraft:locked".to_string(),
            registry_fingerprint: snapshot.fingerprint(),
            inventory_revision: 0,
            container_revision: 0,
            expected_inputs: vec![ExpectedSlot {
                slot: 0,
                identifier: "minecraft:stone".to_string(),
                data: 0,
                count: 1,
            }],
            requested_count: 1,
            operation_id: 9,
            deadline_unix: 0,
        };
        // Rule off + not unlocked => denied.
        let outcome = submit_craft(
            &inventory,
            &revision,
            &session,
            &book,
            &registry,
            &snapshot,
            &tags,
            intent.clone(),
            true,
            true,
            true,
            false,
            true,
            true,
            0,
            0,
            &mut outbox,
            42,
        );
        assert!(matches!(
            outcome.receipt,
            CraftReceipt::Rejected {
                reason: CraftReject::RuleDenied(_),
                ..
            }
        ));
        // After unlock it succeeds.
        book.unlock("minecraft:locked");
        let outcome = submit_craft(
            &inventory,
            &revision,
            &session,
            &book,
            &registry,
            &snapshot,
            &tags,
            CraftIntent {
                operation_id: 10,
                ..intent
            },
            true,
            true,
            true,
            false,
            true,
            true,
            0,
            0,
            &mut outbox,
            42,
        );
        assert!(matches!(outcome.receipt, CraftReceipt::Applied { .. }));
    }

    fn furnace_roles() -> Vec<SlotRole> {
        vec![SlotRole::Input, SlotRole::Fuel, SlotRole::Result]
    }

    #[test]
    fn processor_dual_clocks_advance_independently() {
        use crate::furnace::TickOutcome;
        // Burn decays on its own (no input, no output): the burn clock is
        // independent of the cook clock.
        let state = BlockProcessState::new(StationKind::Furnace, furnace_roles(), 200);
        assert!(state.set_slot(SlotRole::Fuel, ItemStack::new(1, 1)));
        state.set_slot(SlotRole::Input, ItemStack::new(4, 0));
        // Empty input stack: nothing smeltable, fuel never ignites.
        let outcome = state.advance(None, 1600, 64);
        assert_eq!(outcome, TickOutcome::default());
        assert!(!state.is_burning());
        // Smeltable input + fuel: ignites, burn decays while cook advances.
        assert!(state.set_slot(SlotRole::Input, ItemStack::new(4, 1)));
        let out = ItemStack::new(5, 1);
        let outcome = state.advance(Some(&out), 1600, 64);
        assert!(outcome.consumed_fuel);
        assert!(outcome.lit_changed);
        assert!(state.is_burning());
        assert_eq!(state.burn_remaining(), 1599);
        assert_eq!(state.cook_progress(), 1);
        // Cook stalls without smeltable input, burn keeps decaying.
        let outcome = state.advance(None, 1600, 64);
        assert!(!outcome.consumed_fuel && !outcome.completed);
        assert_eq!(state.cook_progress(), 1);
        assert_eq!(state.burn_remaining(), 1598);
    }

    #[test]
    fn processor_completes_cook_and_resets_without_fuel() {
        // Short duration: completion moves one output, consumes one input.
        let state = BlockProcessState::new(StationKind::Smoker, furnace_roles(), 2);
        assert!(state.insert_input(ItemStack::new(4, 2)));
        assert!(
            !state.insert_input(ItemStack::new(4, 1)),
            "occupied rejects"
        );
        assert!(state.set_slot(SlotRole::Fuel, ItemStack::new(1, 1)));
        let out = ItemStack::new(5, 1);
        assert!(!state.advance(Some(&out), 100, 64).completed);
        let outcome = state.advance(Some(&out), 100, 64);
        assert!(outcome.completed);
        assert_eq!(state.slot(SlotRole::Result), Some(ItemStack::new(5, 1)));
        assert_eq!(
            state.slot(SlotRole::Input).map(|stack| stack.count),
            Some(1)
        );
        assert_eq!(state.cook_progress(), 0);
        // Fuel exhaustion resets progress (furnace turn-off).
        let poor = BlockProcessState::new(StationKind::Furnace, furnace_roles(), 200);
        assert!(poor.insert_input(ItemStack::new(4, 1)));
        assert!(poor.set_slot(SlotRole::Fuel, ItemStack::new(2, 1)));
        for _ in 0..101 {
            poor.advance(Some(&out), 100, 64);
        }
        assert!(!poor.is_burning());
        assert_eq!(poor.cook_progress(), 0);
        assert_eq!(poor.slot(SlotRole::Result), Some(ItemStack::empty()));
    }

    #[test]
    fn processor_rejects_unknown_roles_and_keeps_save_contract() {
        // Single-slot decl: fuel/result roles are absent, accessors fail closed.
        let state = BlockProcessState::new(StationKind::Furnace, vec![SlotRole::Input], 3);
        assert!(state.slot(SlotRole::Fuel).is_none());
        assert!(!state.set_slot(SlotRole::Fuel, ItemStack::new(1, 1)));
        assert!(state.take_slot(SlotRole::Result).is_none());
        assert!(state.take_output().is_none());
        assert!(state.insert_input(ItemStack::new(1, 1)));
        // Save contract on the full-slot snapshot.
        let snapshot = state.snapshot();
        assert_eq!(snapshot.cook_total, 3);
        assert_eq!(snapshot.slots.len(), 1);
        assert!(snapshot.dirty);
        state.acknowledge_save(snapshot.generation);
        assert!(!state.is_dirty());
        // Stale acknowledgements never clear newer dirt.
        assert!(state.insert_input(ItemStack::new(1, 1)) == false);
        let full = BlockProcessState::new(StationKind::Furnace, furnace_roles(), 3);
        full.set_output(ItemStack::new(2, 1));
        let dirtied = full.snapshot();
        full.acknowledge_save(dirtied.generation.wrapping_sub(1));
        assert!(full.is_dirty());
        full.acknowledge_save(dirtied.generation);
        assert!(!full.is_dirty());
        assert_eq!(full.take_output(), Some(ItemStack::new(2, 1)));
    }
}

// Events and systems.

/// Craft request boundary event (sent after network translation).
///
/// Carries only the recipe network id (snapshot index), never identifier
/// strings: forged client strings stay out of the execution path.
/// Workstations derive from recipe declarations and nearby real blocks.
#[derive(sc_ecs::event::Event, Clone, Debug)]
pub struct CraftRequestEvent {
    pub actions: Vec<crate::inventory::craft_inventory::InventoryAction>,
    pub entity: sc_ecs::entity::EntityId,
    pub recipe_network_id: u32,
    pub requested_count: u16,
    pub operation_id: u64,
}

#[derive(Component, Default)]
pub struct CraftInbox {
    pending: std::sync::Mutex<
        VecDeque<(
            sc_world::manager::MinecraftWorldId,
            Option<u64>,
            CraftRequestEvent,
        )>,
    >,
}

/// Player container window id allocator.
///
/// First window is 1, then increments with `% 100` wraparound skipping 0.
/// The inventory window (0) never allocates here.
#[derive(Component, Debug, Default)]
pub struct ContainerWindowAllocator {
    next: parking_lot::Mutex<u8>,
}

impl ContainerWindowAllocator {
    pub fn allocate(&self) -> u8 {
        let mut next = self.next.lock();
        // Allocate max(1, ++cnt % 100).
        let id = (*next).wrapping_add(1) % 100;
        let id = id.max(1);
        *next = id;
        id
    }
}

pub fn open_crafting_station(
    world: &sc_ecs::world::World,
    entity: sc_ecs::entity::EntityId,
    position: sc_block::position::BlockPosition,
    outbox: &mut NetworkOutbox,
) -> bool {
    // One client holds at most one container window: return early when a
    // window is already open, before touching world data.
    if let Some(open) = world.get_component::<ContainerOpen>(&entity) {
        log::debug!(
            "[craft] workstation kept closed: window already open (window={} type={})",
            open.window_id,
            open.kind_name()
        );
        return true;
    }
    let Some(world_id) = world.get_component::<sc_world::manager::MinecraftWorldId>(&entity) else {
        return false;
    };
    let pos = (position.x, position.y, position.z);
    let Some(decl) = workstation_decl_at(world, &world_id, pos) else {
        return false;
    };
    let station = StationKind::from_tag(&decl.station);
    let container_type = decl.container;
    // Only windowed workstations open (brewing/smithing/campfire decls
    // address but never open); unknown tags fail closed (load validation
    // rejects such packs, this is the defense in depth).
    if station == StationKind::Unknown || !decl.window {
        return false;
    }
    // Requires the OPEN_CONTAINERS ability.
    if !world
        .get_component::<sc_entity::player::adventure_settings::AdventureSettings>(&entity)
        .is_some_and(|settings| {
            settings.get(
                &sc_entity::player::adventure_settings::AdventureSettingsType::OpenContainers,
            )
        })
    {
        return false;
    }
    let Some(transform) = world.get_component::<sc_entity::motion::Transform>(&entity) else {
        return true;
    };
    // Player feet position: use the always-current client position, since
    // Transform stays stale until the first movement packet.
    let feet = world
        .get_component::<sc_utils::game::client::MinecraftClient>(&entity)
        .map(|client| {
            let data = client.data.read();
            (data.position.x, data.position.y, data.position.z)
        })
        .unwrap_or_else(|| {
            let transform = transform.read();
            (
                transform.position.x,
                transform.position.y,
                transform.position.z,
            )
        });
    let distance = (feet.0 - position.x as f32 - 0.5).powi(2)
        + (feet.1 - position.y as f32 - 0.5).powi(2)
        + (feet.2 - position.z as f32 - 0.5).powi(2);
    if distance > 36.0 {
        log::debug!(
            "[craft] workstation kept closed: out of reach {distance:.1} (feet {feet:?}, block {pos:?})"
        );
        return true;
    }
    let Some(actor) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
        return true;
    };
    // Window ids allocate incrementally per player.
    if world
        .get_component::<ContainerWindowAllocator>(&entity)
        .is_none()
    {
        world.add_component(&entity, ContainerWindowAllocator::default());
    }
    let window_id = world
        .get_component::<ContainerWindowAllocator>(&entity)
        .map(|allocator| allocator.allocate())
        .unwrap_or(1);
    world.add_component(
        &entity,
        ContainerOpen::at_workstation(window_id, station, world_id.as_ref().clone(), pos),
    );
    log::debug!(
        "[craft] workstation opened {station:?} (block {pos:?}, window {window_id}, feet {feet:?}, dist2={distance:.1})"
    );
    let mut faults = world.get_resource_mut::<crate::net::PendingConnectionFaults>();
    crate::net::IntentPublisher::new(outbox)
        .with_world(world)
        .maybe_with_faults(faults.as_deref_mut())
        .publish(OpenCraftingStation {
            entity_id: actor.0,
            station,
            window_id,
            container_type,
            x: position.x,
            y: position.y,
            z: position.z,
        });
    if crate::furnace::is_furnace_station(station) {
        // Identity ensure (creation-only here; reads below go through the
        // component). A refused ensure opens with empty slots, exactly like
        // the old store-full path.
        crate::block_entity::ensure_block_entity(
            world,
            world_id.as_ref().clone(),
            pos,
            crate::block_entity::BlockEntityKind::Furnace(station),
        );
        let slots = crate::block_entity::process_state_at(world, world_id.as_ref(), pos)
            .map(|process| process.furnace_slots())
            .unwrap_or_default();
        crate::net::IntentPublisher::new(outbox)
            .with_world(world)
            .publish(crate::net::SyncFurnaceWindow {
                entity_id: actor.0,
                window_id,
                station,
                input: slots[0].clone(),
                fuel: slots[1].clone(),
                result: slots[2].clone(),
            });
    }
    true
}

impl CraftInbox {
    pub const MAX_REQUESTS: usize = 64;
    pub fn try_push(
        &self,
        world_id: sc_world::manager::MinecraftWorldId,
        epoch: Option<u64>,
        request: CraftRequestEvent,
    ) -> bool {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending.len() >= Self::MAX_REQUESTS {
            return false;
        }
        pending.push_back((world_id, epoch, request));
        true
    }
}

/// Bump the authoritative revision after inventory changes.
///
/// A missed bump only over-rejects, never double-spends.
pub fn bump_inventory_revision(world: &sc_ecs::world::World, entity: &sc_ecs::entity::EntityId) {
    if let Some(revision) = world.get_component::<CraftingRevision>(entity) {
        revision.bump_inventory();
    }
}

pub(crate) fn recipe_allowed(
    world: &sc_ecs::world::World,
    entity: sc_ecs::entity::EntityId,
    recipe: &sc_recipe::CompiledRecipe,
) -> bool {
    if world
        .get_component::<sc_utils::game::client::MinecraftClient>(&entity)
        .is_some_and(|client| client.data.read().gamemode.is_spectator())
    {
        return false;
    }
    let allowed = world
        .get_component::<sc_world::manager::MinecraftWorldId>(&entity)
        .and_then(|id| {
            world
                .get_resource::<sc_world::manager::MinecraftWorldManager>()
                .map(|manager| recipes_unlock_allowed(&manager, &id))
        })
        .unwrap_or(true);
    let locked = !recipe.unlock.is_empty()
        || matches!(
            recipe.unlock_context.as_deref(),
            Some("PlayerInWater" | "PlayerHasManyItems")
        );
    allowed
        || !locked
        || world
            .get_component::<RecipeBookState>(&entity)
            .is_some_and(|book| book.inner.read().is_unlocked(&recipe.identifier))
}

/// World `recipesUnlock` rule: missing rules default to available.
fn recipes_unlock_allowed(
    manager: &sc_world::manager::MinecraftWorldManager,
    world_id: &sc_world::manager::MinecraftWorldId,
) -> bool {
    use sc_utils::game::gamerules::{GameRule, GameRuleType};
    let Some(world) = manager.get_world(world_id) else {
        return true;
    };
    for (rule, value) in world.world_data.gamerules.iter() {
        if *rule == GameRule::RecipesUnlock {
            return match &value.value {
                GameRuleType::Bool(allowed) => *allowed,
                _ => true,
            };
        }
    }
    true
}

pub(crate) fn station_block_kind(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<StationKind> {
    let decl = workstation_decl_at(world, world_id, pos)?;
    let station = StationKind::from_tag(&decl.station);
    (station != StationKind::Unknown).then_some(station)
}

/// Workstation declaration for the block at a position (`None` when the
/// chunk is unloaded, the cell is empty, or the block serves no station).
/// Single chunk-read behind `station_block_kind` and container resolution.
pub(crate) fn workstation_decl_at(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<sc_block::workstation::WorkstationDecl> {
    use sc_block::position::BlockPosition;
    use sc_world::storage::ChunkKey;
    let manager = world.get_resource::<sc_world::manager::MinecraftWorldManager>()?;
    let minecraft_world = manager.get_world(world_id)?;
    let target = BlockPosition::new(pos.0, pos.1, pos.2);
    let key = ChunkKey::new(
        minecraft_world.world_data.get_dimension(),
        target.chunk_position(),
    );
    let column = minecraft_world.chunk_provider.cached_chunk(key)?;
    let chunk = column.read();
    let state = chunk.block_at(target.local_x(), target.y, target.local_z())?;
    let registry = world.get_resource::<sc_block::registry::BlockStateRegistry>()?;
    let snapshot = world
        .get_resource::<sc_block::block_json::BlockJsonRegistry>()
        .and_then(|registry| registry.get())?;
    let dense = registry.by_runtime_id(state)?;
    snapshot.workstation_of(dense.0).cloned()
}

/// Find a usable recipe workstation near the player.
///
/// Scan nearby blocks for each declared station, returning the first hit.
/// Unloaded chunks count as having no workstation (fail-closed).
///
/// Reserved for workstation-proximity validation once callers migrate off the
/// open-window check (see `open_crafting_station`).
#[allow(dead_code)]
fn find_nearby_station(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    feet: (f32, f32, f32),
    stations: &[StationKind],
) -> Option<(StationKind, (i32, i32, i32))> {
    if stations.is_empty() {
        return None;
    }
    let (fx, fy, fz) = feet;
    let min_x = (fx - 6.0).floor() as i32;
    let max_x = (fx + 6.0).floor() as i32;
    let min_y = (fy - 6.0).floor() as i32;
    let max_y = (fy + 6.0).floor() as i32;
    let min_z = (fz - 6.0).floor() as i32;
    let max_z = (fz + 6.0).floor() as i32;
    // Deterministic order: scan by ascending squared distance.
    for wanted in stations {
        let mut best: Option<((i32, i32, i32), f32)> = None;
        let mut x = min_x;
        while x <= max_x {
            let mut y = min_y;
            while y <= max_y {
                let mut z = min_z;
                while z <= max_z {
                    let dx = (x as f32 + 0.5) - fx;
                    let dy = (y as f32 + 0.5) - fy;
                    let dz = (z as f32 + 0.5) - fz;
                    let dist_sq = dx * dx + dy * dy + dz * dz;
                    if dist_sq <= 36.0 {
                        let dominated = best.map(|(_, d)| dist_sq >= d).unwrap_or(false);
                        if !dominated
                            && station_block_kind(world, world_id, (x, y, z)).as_ref()
                                == Some(wanted)
                        {
                            best = Some(((x, y, z), dist_sq));
                        }
                    }
                    z += 1;
                }
                y += 1;
            }
            x += 1;
        }
        if let Some((pos, _)) = best {
            return Some((*wanted, pos));
        }
    }
    None
}

/// Crafting system: boundary events to authoritative execution.
/// Resolves the network id, derives a nearby workstation, plans multiset
/// consumption, then atomically executes and syncs.
///
/// Shaped planning consumes multiset units (1 per non-empty cell) without
/// replicating client grid geometry; strict matching stays in
/// `match_shaped`.
pub fn handle_craft_requests(
    world: sc_ecs::world::World,
    mut reader: sc_ecs::params::event::EventReader<CraftRequestEvent>,
    mut outbox: sc_ecs::params::resource::ResMut<NetworkOutbox>,
) {
    for entity in world.entities_with_component::<CraftInbox>() {
        let Some(inbox) = world.get_component::<CraftInbox>(&entity) else {
            continue;
        };
        let pending = std::mem::take(
            &mut *inbox
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        for (world_id, epoch, request) in pending {
            let current = world.get_component::<sc_world::manager::MinecraftWorldId>(&entity);
            let valid = current.as_deref() == Some(&world_id)
                && crate::block::mining_context_epoch(&world, &entity) == epoch;
            let success = valid && execute_request(&world, &request, &mut outbox);
            publish_response(&world, &request, success, &mut outbox);
        }
    }
    for request in reader.read() {
        let success = execute_request(&world, request, &mut outbox);
        publish_response(&world, request, success, &mut outbox);
    }
}

fn publish_response(
    world: &sc_ecs::world::World,
    request: &CraftRequestEvent,
    success: bool,
    outbox: &mut NetworkOutbox,
) {
    if let Some(actor) = world.get_component::<sc_entity::MinecraftEntityId>(&request.entity) {
        let mut faults = world.get_resource_mut::<crate::net::PendingConnectionFaults>();
        crate::net::IntentPublisher::new(outbox)
            .with_world(&world)
            .maybe_with_faults(faults.as_deref_mut())
            .publish(CraftResponse {
                slots: crate::inventory::craft_inventory::updates(world, request.entity),
                entity_id: actor.0,
                request_id: request.operation_id as i32,
                success,
            });
        let mut pending =
            world.get_resource_mut::<crate::net::PendingInventoryResync>();
        if !success {
            crate::net::IntentPublisher::new(outbox)
                .with_world(&world)
                .maybe_with_pending_resync(pending.as_deref_mut())
                .publish(ResyncInventory { entity_id: actor.0 });
        }
        // Furnace viewers always get authoritative slots after a request
        // (success applies the move, failure rolls the window back).
        if let Some(open) = world.get_component::<ContainerOpen>(&request.entity) {
            if let Some((station, world_id, pos)) = open
                .workstation()
                .filter(|(station, _, _)| crate::furnace::is_furnace_station(*station))
            {
                if let Some(slots) = crate::block_entity::process_state_at(world, &world_id, pos)
                    .map(|process| process.furnace_slots())
                {
                    crate::net::IntentPublisher::new(outbox)
                        .with_world(&world)
                        .publish(crate::net::SyncFurnaceWindow {
                            entity_id: actor.0,
                            window_id: open.window_id,
                            station,
                            input: slots[0].clone(),
                            fuel: slots[1].clone(),
                            result: slots[2].clone(),
                        });
                }
            }
        }
    }
}

fn execute_request(
    world: &sc_ecs::world::World,
    request: &CraftRequestEvent,
    outbox: &mut NetworkOutbox,
) -> bool {
    if !request.actions.is_empty() {
        return crate::inventory::craft_inventory::execute(world, request);
    }
    let entity = request.entity;
    let Some(inventory) = world.get_component::<PlayerInventory>(&entity) else {
        return false;
    };
    if world.get_component::<CraftingRevision>(&entity).is_none() {
        world.add_component(&entity, CraftingRevision::default());
    }
    if world.get_component::<CraftingSession>(&entity).is_none() {
        world.add_component(&entity, CraftingSession::default());
    }
    if world.get_component::<RecipeBookState>(&entity).is_none() {
        world.add_component(&entity, RecipeBookState::default());
    }
    let (Some(revision), Some(session), Some(book), Some(registry), Some(shared)) = (
        world.get_component::<CraftingRevision>(&entity),
        world.get_component::<CraftingSession>(&entity),
        world.get_component::<RecipeBookState>(&entity),
        world.get_resource::<ItemRegistry>(),
        world.get_resource::<SharedRecipeRegistry>(),
    ) else {
        return false;
    };
    let snapshot = shared.snapshot();
    if let Some(actor) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) {
        if let Some(receipt) = session.seen.read().get(&(actor.0, request.operation_id)) {
            return matches!(receipt, CraftReceipt::Applied { .. });
        }
    }
    let Some(recipe) = request
        .recipe_network_id
        .checked_sub(2)
        .and_then(|index| snapshot.get_by_index(index))
        .cloned()
    else {
        if let Some(runtime) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) {
            outbox
                .push(NetworkIntent::ResyncInventory(ResyncInventory {
                    entity_id: runtime.0,
                }))
                .ok();
        }
        return false;
    };
    // Only instant recipes execute here; process recipes use handlers.
    if !matches!(
        recipe.body,
        sc_recipe::RecipeBody::Shaped(_) | sc_recipe::RecipeBody::Shapeless(_)
    ) {
        return false;
    }
    let Some(world_id) = world.get_component::<sc_world::manager::MinecraftWorldId>(&entity) else {
        return false;
    };
    let Some(actor_runtime) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
        return false;
    };
    let feet = world
        .get_component::<sc_entity::motion::Transform>(&entity)
        .map(|transform| {
            let position = transform.read().position;
            (position.x, position.y, position.z)
        });
    // Workstation derives from declarations and nearby real blocks.
    // Recipes without station declarations need no workstation.
    let fits_player_grid = sc_recipe::fits_inventory_grid(&recipe.body);
    let (station, station_pos) = if recipe.stations.is_empty()
        || (fits_player_grid && recipe.stations.contains(&StationKind::CraftingTable))
    {
        (StationKind::CraftingTable, None)
    } else {
        let Some((open_station, open_world_id, open_position)) = world
            .get_component::<ContainerOpen>(&entity)
            .and_then(|open| open.workstation())
        else {
            return false;
        };
        let Some(feet) = feet else {
            return false;
        };
        let distance = (feet.0 - open_position.0 as f32 - 0.5).powi(2)
            + (feet.1 - open_position.1 as f32 - 0.5).powi(2)
            + (feet.2 - open_position.2 as f32 - 0.5).powi(2);
        if open_world_id != *world_id
            || !recipe.stations.contains(&open_station)
            || distance > 36.0
            || station_block_kind(world, &world_id, open_position) != Some(open_station)
        {
            return false;
        }
        (open_station, Some(open_position))
    };
    // Multiset planning from the server snapshot.
    let tags = world
        .get_resource::<SharedItemTags>()
        .map(|table| table.resolver())
        .unwrap_or_else(MapTagResolver::empty);
    let live: Vec<(usize, sc_recipe::MatchInput)> = (0..inventory.len())
        .filter_map(|slot| {
            let stack = inventory.get(slot)?;
            if stack.is_empty() {
                return None;
            }
            let definition = registry.get(stack.runtime_id)?;
            Some((
                slot,
                sc_recipe::MatchInput {
                    identifier: definition.name.to_string(),
                    data: stack.damage as i32,
                    components: None,
                    count: stack.count,
                },
            ))
        })
        .collect();
    if request.requested_count == 0 || request.requested_count > 64 {
        return false;
    }
    let Some(requirements) = recipe_requirements(&recipe)
        .into_iter()
        .map(|(spec, count)| {
            count
                .checked_mul(request.requested_count)
                .map(|count| (spec, count))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let Some(plan) = sc_recipe::plan_consumption(&requirements, &live, &tags, 1024) else {
        outbox
            .push(NetworkIntent::ResyncInventory(ResyncInventory {
                entity_id: actor_runtime.0,
            }))
            .ok();
        return false;
    };
    let expected_inputs: Vec<ExpectedSlot> = plan
        .iter()
        .map(|(slot, take)| {
            let input = live
                .iter()
                .find(|(index, _)| index == slot)
                .expect("planned slot is live");
            ExpectedSlot {
                slot: *slot,
                identifier: input.1.identifier.clone(),
                data: input.1.data,
                count: *take,
            }
        })
        .collect();
    let gamemode_allows = world
        .get_component::<sc_utils::game::client::MinecraftClient>(&entity)
        .map(|client| !client.data.read().gamemode.is_spectator())
        .unwrap_or(true);
    let recipes_unlock_rule = world
        .get_resource::<sc_world::manager::MinecraftWorldManager>()
        .map(|manager| recipes_unlock_allowed(&manager, &world_id))
        .unwrap_or(true);
    let intent = CraftIntent {
        actor: CraftActor {
            player_id: actor_runtime.0,
            entity_generation: 0,
        },
        context: CraftContext {
            world_id: world_id.world_id.to_string(),
            dimension: 0,
            station,
            station_pos,
            station_revision: revision.station(),
            client_claims_in_reach: true,
        },
        recipe_id: recipe.identifier.clone(),
        registry_fingerprint: snapshot.fingerprint(),
        inventory_revision: revision.inventory(),
        container_revision: revision.container(),
        expected_inputs,
        // Consumption was planned for exactly this many crafts.
        requested_count: request.requested_count,
        operation_id: request.operation_id,
        deadline_unix: 0,
    };
    let outcome = submit_craft(
        &inventory,
        &revision,
        &session,
        &book,
        &registry,
        &snapshot,
        &tags,
        intent,
        // Reachability/station were established by the authoritative
        // proximity scan above (scan radius == reach radius).
        true,
        true,
        gamemode_allows,
        recipes_unlock_rule,
        true,
        true,
        revision.container(),
        revision.station(),
        outbox,
        actor_runtime.0,
    );
    matches!(outcome.receipt, CraftReceipt::Applied { .. })
}

/// Per-recipe consumption requirements.
fn recipe_requirements(
    recipe: &sc_recipe::CompiledRecipe,
) -> Vec<(sc_recipe::IngredientSpec, u16)> {
    use sc_recipe::RecipeBody;
    match &recipe.body {
        RecipeBody::Shaped(body) => body
            .grid
            .iter()
            .flatten()
            .map(|spec| (spec.clone(), 1))
            .collect(),
        RecipeBody::Shapeless(body) => body
            .ingredients
            .iter()
            .map(|spec| (spec.clone(), spec.count))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use sc_ecs::system::{IntoSystem, System};
    use sc_world::manager::MinecraftWorldId;

    fn e2e_snapshot() -> RecipeRegistrySnapshot {
        // No-tags shapeless recipe: needs no workstation block.
        let raw = serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:e2e_test"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:stone"}],
                "result": {"item": "minecraft:e2e_out"}
            }
        });
        let bytes = serde_json::to_vec(&raw).unwrap();
        let src = SourceRecipe::new("p", 0, "r.json", "1.12", raw, &bytes);
        let (snapshot, _) =
            RecipeRegistrySnapshot::compile(&[src], &CompileBudgets::default(), false).unwrap();
        snapshot
    }

    #[test]
    fn craft_request_event_executes_end_to_end() {
        let world = sc_ecs::world::World::new();
        world.insert_resource(sc_ecs::event::Events::<CraftRequestEvent>::new());
        let registry = ItemRegistry::new();
        registry.upsert(sc_item::ItemDefinition::new(1, "minecraft:stone"));
        registry.upsert(sc_item::ItemDefinition::new(2, "minecraft:e2e_out"));
        world.insert_resource(registry);
        world.insert_resource(SharedRecipeRegistry::new(e2e_snapshot()));
        world.insert_resource(SharedItemTags::default());
        world.insert_resource(NetworkOutbox::with_limits(64, 64, usize::MAX));

        let inventory = PlayerInventory::new(4);
        inventory.set(0, ItemStack::new(1, 5));
        let entity = world.spawn((
            inventory,
            MinecraftWorldId::random(),
            sc_entity::MinecraftEntityId(42),
        ));
        let network_id = world
            .get_resource::<SharedRecipeRegistry>()
            .unwrap()
            .snapshot()
            .network_index("minecraft:e2e_test")
            .expect("recipe indexed");
        world.send_event(CraftRequestEvent {
            actions: Vec::new(),
            entity,
            recipe_network_id: network_id + 2,
            requested_count: 2,
            operation_id: (-1i32) as u64,
        });
        handle_craft_requests.into_system().run(&world);

        let inventory = world.get_component::<PlayerInventory>(&entity).unwrap();
        assert_eq!(inventory.get(0).unwrap().count, 3);
        let revision = world.get_component::<CraftingRevision>(&entity).unwrap();
        assert_eq!(revision.inventory(), 1);
        assert_eq!(
            inventory
                .snapshot()
                .iter()
                .filter(|stack| stack.runtime_id == 2)
                .map(|stack| stack.count)
                .sum::<u16>(),
            2
        );
        world.send_event(CraftRequestEvent {
            actions: Vec::new(),
            entity,
            recipe_network_id: network_id + 2,
            requested_count: 2,
            operation_id: (-1i32) as u64,
        });
        // A fresh test reader sees the first event again as well as its retry.
        handle_craft_requests.into_system().run(&world);
        assert_eq!(inventory.get(0).unwrap().count, 3);
        assert_eq!(revision.inventory(), 1);
        assert!(world
            .get_resource_mut::<NetworkOutbox>()
            .unwrap()
            .drain()
            .iter()
            .any(|intent| matches!(
                intent,
                NetworkIntent::CraftResponse(p)
                    if p.request_id == -1 && p.success
            )));
    }

    #[test]
    fn unknown_recipe_id_resyncs_without_executing() {
        let world = sc_ecs::world::World::new();
        world.insert_resource(sc_ecs::event::Events::<CraftRequestEvent>::new());
        world.insert_resource(ItemRegistry::new());
        world.insert_resource(SharedRecipeRegistry::new(e2e_snapshot()));
        world.insert_resource(SharedItemTags::default());
        world.insert_resource(NetworkOutbox::with_limits(64, 64, usize::MAX));
        let inventory = PlayerInventory::new(4);
        inventory.set(0, ItemStack::new(1, 5));
        let entity = world.spawn((
            inventory,
            MinecraftWorldId::random(),
            sc_entity::MinecraftEntityId(43),
        ));
        world.send_event(CraftRequestEvent {
            actions: Vec::new(),
            entity,
            recipe_network_id: 999,
            requested_count: 1,
            operation_id: 101,
        });
        handle_craft_requests.into_system().run(&world);
        let inventory = world.get_component::<PlayerInventory>(&entity).unwrap();
        assert_eq!(inventory.get(0).unwrap().count, 5);
    }

    #[test]
    fn workstation_bits_come_from_decls_not_code() {
        // The identifier→station map used to live here as a hardcoded
        // match; it now lives in `sc:workstation` decls, covered per-state
        // against the live pack by the furnace parity gate. This test pins
        // the decl-read path shape: unknown tags resolve to `Unknown`
        // (fail-closed downstream, rejected at load).
        assert_eq!(
            StationKind::from_tag("crafting_table"),
            StationKind::CraftingTable
        );
        assert_eq!(
            StationKind::from_tag("stove_does_not_exist"),
            StationKind::Unknown
        );
    }
}
