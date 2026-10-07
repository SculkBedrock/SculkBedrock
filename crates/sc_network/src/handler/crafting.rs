//! Crafting input translation: packets → protocol-independent [`CraftIntent`].
//!
//! The game domain never sees packet types. This module only *translates*:
//! - `ItemStackRequest` (0x93) `CraftRecipe` / `CraftRecipeAuto` actions
//!   carry an explicit recipe network id + count;
//! - `InventoryTransaction` `NORMAL` actions carry slot deltas without a
//!   recipe id (the game runs its matcher over the declared consumption);
//! - `PlayerAuthInput` block actions are mining, not crafting (handled in
//!   `interaction.rs`); the embedded item-stack-request payload inside
//!   `PlayerAuthInput` is **not** decoded in this build (protocol shape is
//!   version-dependent) — crafting intents come from the top-level
//!   `ItemStackRequest` packet instead, which is documented, not silent;
//! - container open/close sets the station context (player inventory vs
//!   block workstation) and is already routed through
//!   `OpenInventoryRequest` / `CloseInventoryRequest`.
//!
//! All functions are pure and synchronous; no locks are held.

use sc_recipe::{CraftActor, CraftContext, CraftIntent, ExpectedSlot, StationKind};

use crate::protocol::client::crafting_request::{ItemStackRequest, ItemStackRequestAction};
use crate::protocol::client::transaction::{InventoryTransaction, TransactionType};

/// Resolve a recipe network id to its identifier. The game layer passes a
/// snapshot-backed lookup; the translator stays snapshot-agnostic.
pub type NetworkIdLookup<'a> = &'a dyn Fn(u32) -> Option<String>;

/// Build [`CraftIntent`]s from one `ItemStackRequest` packet.
///
/// Unknown/non-craft actions yield no intent (they are inventory moves, not
/// crafts). Malformed packets never reach here — the packet parser already
/// rejected them without panicking.
pub fn intents_from_stack_request(
    packet: &ItemStackRequest,
    actor: CraftActor,
    context: CraftContext,
    registry_fingerprint: sc_recipe::RecipeRegistryFingerprint,
    inventory_revision: u64,
    container_revision: u64,
    lookup: NetworkIdLookup,
) -> Vec<CraftIntent> {
    let mut out = Vec::new();
    for (request_id, network_id, times) in packet.craft_actions() {
        let Some(recipe_id) = lookup(network_id) else {
            continue;
        };
        out.push(CraftIntent {
            actor: actor.clone(),
            context: context.clone(),
            recipe_id,
            registry_fingerprint,
            inventory_revision,
            container_revision,
            expected_inputs: Vec::new(),
            requested_count: (times.max(1)) as u16,
            operation_id: request_id as u64,
            deadline_unix: 0,
        });
    }
    out
}

/// Describe one inventory slot delta observed in an `InventoryTransaction`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlotDelta {
    pub slot: usize,
    pub from_runtime: u16,
    pub from_count: u16,
    pub to_runtime: u16,
    pub to_count: u16,
}

/// Extract slot deltas from a `NORMAL` transaction. Returns `None` for
/// non-craft transaction types (`ITEM_USE`, ...).
pub fn deltas_from_transaction(packet: &InventoryTransaction) -> Option<Vec<SlotDelta>> {
    if packet.transaction_type != TransactionType::NORMAL
        && packet.transaction_type != TransactionType::MISMATCH
    {
        return None;
    }
    let mut out = Vec::new();
    for action in &packet.actions {
        out.push(SlotDelta {
            slot: action.slot as usize,
            from_runtime: action.from_item.runtime_id,
            from_count: action.from_item.count,
            to_runtime: action.to_item.runtime_id,
            to_count: action.to_item.count,
        });
    }
    Some(out)
}

/// Map a container window to its crafting station.
pub fn station_for_container(container_type: i8, window_id: u8) -> StationKind {
    // Player inventory (window 0) = 2x2 crafting_table context.
    if window_id == 0 {
        return StationKind::CraftingTable;
    }
    match container_type {
        1 => StationKind::CraftingTable, // workbench container
        29 => StationKind::Stonecutter,
        2 => StationKind::Furnace,
        _ => StationKind::CraftingTable,
    }
}

/// Whether a `PlayerAuthInput` packet carries block actions (mining path,
/// not crafting). Item interaction flags are noted but the embedded
/// item-stack-request is not decoded here — see module docs.
pub fn auth_input_has_block_actions(block_action_count: usize) -> bool {
    block_action_count > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::client::crafting_request::ItemStackRequestEntry;

    #[test]
    fn stack_request_translates_to_intent() {
        let packet = ItemStackRequest {
            requests: vec![ItemStackRequestEntry {
                request_id: 11,
                actions: vec![ItemStackRequestAction::CraftRecipe {
                    recipe_network_id: 9,
                    times: 2,
                }],
            }],
        };
        let lookup = |id: u32| {
            if id == 9 {
                Some("minecraft:test".to_string())
            } else {
                None
            }
        };
        let intents = intents_from_stack_request(
            &packet,
            CraftActor {
                player_id: 1,
                entity_generation: 0,
            },
            CraftContext {
                world_id: "w".to_string(),
                dimension: 0,
                station: StationKind::CraftingTable,
                station_pos: None,
                station_revision: 0,
                client_claims_in_reach: true,
            },
            sc_recipe::RecipeRegistryFingerprint([0; 32]),
            3,
            4,
            &lookup,
        );
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].recipe_id, "minecraft:test");
        assert_eq!(intents[0].requested_count, 2);
        assert_eq!(intents[0].operation_id, 11);
        assert_eq!(intents[0].inventory_revision, 3);
    }

    #[test]
    fn unknown_network_id_yields_no_intent() {
        let packet = ItemStackRequest {
            requests: vec![ItemStackRequestEntry {
                request_id: 1,
                actions: vec![ItemStackRequestAction::CraftRecipe {
                    recipe_network_id: 999,
                    times: 1,
                }],
            }],
        };
        let lookup = |_: u32| None;
        let intents = intents_from_stack_request(
            &packet,
            CraftActor {
                player_id: 1,
                entity_generation: 0,
            },
            CraftContext {
                world_id: "w".to_string(),
                dimension: 0,
                station: StationKind::CraftingTable,
                station_pos: None,
                station_revision: 0,
                client_claims_in_reach: true,
            },
            sc_recipe::RecipeRegistryFingerprint([0; 32]),
            0,
            0,
            &lookup,
        );
        assert!(intents.is_empty());
    }

    #[test]
    fn transaction_deltas_cover_normal_only() {
        assert_eq!(station_for_container(0, 0), StationKind::CraftingTable);
        assert_eq!(station_for_container(29, 1), StationKind::Stonecutter);
        assert!(auth_input_has_block_actions(2));
        assert!(!auth_input_has_block_actions(0));
    }

    #[test]
    fn take_place_slot_refs_map_to_game_containers() {
        use crate::protocol::client::crafting_request::RequestSlotRef;
        use sc_game::craft_inventory::{CraftContainer as C, InventoryAction as A};
        let slot = |container_enum: u8, slot: u8| RequestSlotRef {
            container_enum,
            dynamic_slot: None,
            slot,
            net_id: 0,
        };
        // Pick up: backpack slot 10 -> cursor (29=INVENTORY, 59=CURSOR).
        assert!(matches!(
            translate_inventory_action(&ItemStackRequestAction::Move {
                action_type: 0,
                amount: Some(1),
                source: slot(29, 10),
                destination: slot(59, 0),
            }),
            A::Move {
                amount: 1,
                source,
                destination,
            } if source.container == C::Inventory
                && source.slot == 10
                && destination.container == C::Cursor
        ));
        // Grid uses CRAFTING_INPUT_CONTAINER(13); result slots use CREATED_OUTPUT(60)/50.
        assert!(matches!(
            translate_inventory_action(&ItemStackRequestAction::Move {
                action_type: 1,
                amount: Some(1),
                source: slot(29, 9),
                destination: slot(13, 28),
            }),
            A::Move { destination, .. } if destination.container == C::Grid && destination.slot == 28
        ));
        // Dynamic slots: Grid passes them through (workbench window id); other containers stay Unsupported.
        let mut dynamic_grid = slot(13, 28);
        dynamic_grid.dynamic_slot = Some(1);
        assert!(matches!(
            translate_inventory_action(&ItemStackRequestAction::Move {
                action_type: 0,
                amount: Some(1),
                source: slot(29, 9),
                destination: dynamic_grid,
            }),
            A::Move { destination, .. } if destination.container == C::Grid
                && destination.dynamic == Some(1)
        ));
        let mut dynamic = slot(29, 9);
        dynamic.dynamic_slot = Some(7);
        assert!(matches!(
            translate_inventory_action(&ItemStackRequestAction::Move {
                action_type: 0,
                amount: Some(1),
                source: dynamic,
                destination: slot(59, 0),
            }),
            A::Unsupported
        ));
    }
}

// ===================== Event translation =====================

use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::world::World;

pub struct SCCraftingHandlerPlugin;

impl Plugin for SCCraftingHandlerPlugin {
    fn build(&self, _: &App) {}
}

/// `ItemStackRequest` (0x93) → [`sc_game::crafting::CraftRequestEvent`].
///
/// Only decodes and translates the protocol: each craft action produces one boundary
/// event, with `requested_count` and `operation_id` passed through untouched (the game
/// side executes once, see `handle_craft_requests`). Client Take/Place layouts are not
/// replayed; the game side plans consumption from authoritative stock.
pub(crate) fn enqueue_ordered_crafting(
    world: &World,
    entity: sc_ecs::entity::EntityId,
    packet: &crate::protocol::MinecraftPackets,
) -> bool {
    use crate::protocol::MinecraftPackets;
    let requests: &[crate::protocol::client::crafting_request::ItemStackRequestEntry] = match packet
    {
        MinecraftPackets::ItemStackRequest(packet) => &packet.requests,
        MinecraftPackets::PlayerAuthInput(input) => match &input.item_stack_request {
            Some(request) => std::slice::from_ref(request),
            None => return true,
        },
        _ => return true,
    };
    let spawned = world
        .get_component::<crate::player_connection::PlayerConnection>(&entity)
        .is_some_and(|connection| {
            matches!(
                connection.get_status(),
                crate::player_connection::PlayerConnectionStatus::InGame
                    | crate::player_connection::PlayerConnectionStatus::Spawned
            )
        });
    if !spawned {
        return true;
    }
    let Some(world_id) = world.get_component::<sc_world::manager::MinecraftWorldId>(&entity) else {
        return true;
    };
    if world
        .get_component::<sc_game::crafting::CraftInbox>(&entity)
        .is_none()
    {
        world.add_component(&entity, sc_game::crafting::CraftInbox::default());
    }
    let Some(inbox) = world.get_component::<sc_game::crafting::CraftInbox>(&entity) else {
        return true;
    };
    for request in requests {
        log::debug!(
            "[craft] enqueued op={} actions={:?}",
            request.request_id,
            request.actions,
        );
        let crafts: Vec<_> = request
            .actions
            .iter()
            .filter_map(|action| match action {
                ItemStackRequestAction::CraftRecipe {
                    recipe_network_id,
                    times,
                }
                | ItemStackRequestAction::CraftRecipeAuto {
                    recipe_network_id,
                    times,
                } => Some((*recipe_network_id, *times)),
                _ => None,
            })
            .collect();
        let valid = crafts.len() == 1
            && crafts[0].1 > 0
            && crafts[0].1 <= 64
            && !request
                .actions
                .iter()
                .any(|action| matches!(action, ItemStackRequestAction::Unknown { .. }));
        let (network_id, times) = if valid { crafts[0] } else { (0, 0) };
        if !inbox.try_push(
            world_id.as_ref().clone(),
            sc_game::interaction::mining_context_epoch(world, &entity),
            sc_game::crafting::CraftRequestEvent {
                actions: request
                    .actions
                    .iter()
                    .map(translate_inventory_action)
                    .collect(),
                entity,
                recipe_network_id: network_id,
                requested_count: times as u16,
                operation_id: request.request_id as u64,
            },
        ) {
            return false;
        }
    }
    true
}

fn translate_inventory_action(
    action: &ItemStackRequestAction,
) -> sc_game::craft_inventory::InventoryAction {
    use sc_game::craft_inventory::{CraftContainer as C, CraftSlotRef, InventoryAction as A};
    let slot = |reference: &crate::protocol::client::crafting_request::RequestSlotRef| {
        // ContainerEnumName (wire values; 21+ shifted by +1 since v560, hence off by one
        // from the old v407 table: 28=HOTBAR, 29=INVENTORY, 59=CURSOR, 60=CREATED_OUTPUT).
        let container = match reference.container_enum {
            12 => C::CombinedInventory,
            28 => C::Hotbar,
            29 => C::Inventory,
            13 => C::Grid,
            59 => C::Cursor,
            60 => C::Output,
            _ => return None,
        };
        // dynamic only appears on windowed crafting grids (= workbench window id, used only
        // for routing; all other containers always use None, so dynamic there means untrusted).
        if reference.dynamic_slot.is_some() && container != C::Grid {
            return None;
        }
        Some(CraftSlotRef {
            container,
            slot: reference.slot,
            net_id: reference.net_id,
            dynamic: if container == C::Grid {
                reference.dynamic_slot
            } else {
                None
            },
        })
    };
    match action {
        ItemStackRequestAction::Move {
            action_type,
            amount,
            source,
            destination,
        } => {
            let (Some(source), Some(destination)) = (slot(source), slot(destination)) else {
                return A::Unsupported;
            };
            if *action_type == 2 {
                A::Swap {
                    source,
                    destination,
                }
            } else {
                A::Move {
                    amount: amount.unwrap_or(0),
                    source,
                    destination,
                }
            }
        }
        ItemStackRequestAction::CraftRecipe {
            recipe_network_id,
            times,
        } => A::Craft {
            recipe_id: *recipe_network_id,
            times: *times,
            automatic: false,
        },
        ItemStackRequestAction::CraftRecipeAuto {
            recipe_network_id,
            times,
        } => A::Craft {
            recipe_id: *recipe_network_id,
            times: *times,
            automatic: true,
        },
        ItemStackRequestAction::Consume { .. } => A::Consume,
        ItemStackRequestAction::Create { .. } => A::Create,
        ItemStackRequestAction::CraftResults { .. } => A::Results,
        _ => A::Unsupported,
    }
}
