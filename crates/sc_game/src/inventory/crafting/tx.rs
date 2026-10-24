//! Craft transactions: snapshot bridging, submit path and request pipeline.
//!
//! Moved verbatim from `crafting.rs` (P4 split); re-exported from [`super`]
//! so existing paths keep compiling.

use super::*;
use std::collections::VecDeque;
use sc_ecs::component::Component;
use sc_item::{ItemRegistry, ItemStack, PlayerInventory};
use sc_recipe::{
    CommitChecks, CompileBudgets, CraftActor, CraftContext, CraftIntent, CraftReceipt,
    CraftReject, ExpectedSlot, MapTagResolver, RecipeRegistrySnapshot, SourceRecipe, StationKind,
    TagResolver, TxInventory, TxSlot, UnlockPolicy,
};
use crate::inventory::ContainerOpen;
use crate::net::{
    CraftResponse, NetworkIntent, NetworkOutbox, ResyncInventory, UpdateInventorySlot,
};

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

/// 提交一次合成意图的结果：终止回执与待发布的副作用。
///
/// Outcome of submitting one craft intent: the terminal receipt and side effects to publish.
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

/// 合成请求边界事件，携带配方网络 ID 与工作站信息。
///
/// Craft request boundary event carrying the recipe network ID and workstation information.
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
        // Chest viewers always get authoritative slots after a request
        // (success applies the move, failure rolls the window back).
        if let Some(open) = world.get_component::<ContainerOpen>(&request.entity) {
            if let Some((world_id, (x, y, z), paired)) = open.chest_info() {
                let slots = crate::inventory::chest::load_chest_slots(
                    world,
                    &world_id,
                    (x, y, z),
                    paired,
                );
                crate::net::IntentPublisher::new(outbox)
                    .with_world(&world)
                    .publish(crate::net::SyncChestWindow {
                        entity_id: actor.0,
                        window_id: open.window_id,
                        slots: slots.clone(),
                    });
                for other_entity in world.entities_with_component::<ContainerOpen>() {
                    if other_entity == request.entity {
                        continue;
                    }
                    let Some(other_open) = world.get_component::<ContainerOpen>(&other_entity) else {
                        continue;
                    };
                    if let Some((other_world, (ox, oy, oz), other_paired)) = other_open.chest_info() {
                        if other_world == world_id
                            && ((ox, oy, oz) == (x, y, z)
                                || Some((ox, oy, oz)) == paired
                                || other_paired == Some((x, y, z))
                                || (paired.is_some() && other_paired == paired))
                        {
                            if let Some(other_actor) =
                                world.get_component::<sc_entity::MinecraftEntityId>(&other_entity)
                            {
                                crate::net::IntentPublisher::new(outbox)
                                    .with_world(&world)
                                    .publish(crate::net::SyncChestWindow {
                                        entity_id: other_actor.0,
                                        window_id: other_open.window_id,
                                        slots: slots.clone(),
                                    });
                            }
                        }
                    }
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