//! Server-authoritative crafting: protocol-independent intents,
//! atomic inventory transactions, workstation processors and unlock.
//!
//! The network layer only translates packets into [`sc_recipe::CraftIntent`]
//! (see `sc_network::handler::crafting`); every rule lives here without any
//! packet type. Region ownership is respected: the owner validates its own
//! epoch/revision and publishes; cross-region work uses explicit
//! reservation / commit / abort and never holds a lock across `await`
//! (all entry points are synchronous on owned snapshots).

// Test-only imports: unit/e2e tests live in this module (`use super::*`)
// while all implementation moved to `registry`/`session`/`station`/`tx`.
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use sc_block::workstation::SlotRole;
#[cfg(test)]
use sc_item::{ItemRegistry, ItemStack, PlayerInventory};
#[cfg(test)]
use sc_recipe::{
    CompileBudgets, CraftActor, CraftContext, CraftIntent, CraftReceipt, CraftReject,
    ExpectedSlot, MapTagResolver, RecipeRegistrySnapshot, SourceRecipe, StationKind,
};

#[cfg(test)]
use crate::inventory::ContainerOpen;
#[cfg(test)]
use crate::net::{NetworkIntent, NetworkOutbox};

pub mod registry;
pub mod session;
pub mod station;
pub mod tx;

pub use registry::{SharedItemTags, SharedRecipeRegistry};
pub use session::{CraftingRevision, CraftingSession, RecipeBookState};
pub use station::{
    open_crafting_station, BlockProcessState, ContainerWindowAllocator, ProcessSnapshot,
};
pub(crate) use station::{station_block_kind, workstation_decl_at};
pub use tx::{
    bump_inventory_revision, expected_from_slots, handle_craft_requests, snapshot_from_sources,
    submit_craft, CraftInbox, CraftOutcome, CraftRequestEvent,
};
pub(crate) use tx::recipe_allowed;


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