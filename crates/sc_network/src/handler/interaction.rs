//! Player interaction packet translation: digging/slot-switches enter a per-player
//! bounded FIFO inside the receive loop, while place/container ops stay boundary
//!
//! - `PlayerAction` (0x24) START_BREAK -> [`StartBreakRequest`] (records BreakingState)
//! - `PlayerAction` (0x24) STOP_BREAK -> [`BreakBlockRequest`] (require_breaking_state=true)
//! - `PlayerAction` (0x24) ABORT_BREAK -> [`AbortBreakRequest`] (clears BreakingState)
//! - `PlayerAction` (0x24) CREATIVE_DESTROY -> [`BreakBlockRequest`] (require_breaking_state=false)
//! - `InventoryTransaction` (0x1e) USE_ITEM ClickBlock -> [`PlaceBlockRequest`]
//! - `MobEquipment` (0x1f) hotbar switch -> [`HeldSlotChanged`]
//!
//! Boundary events live in sc_game::interaction (game domain); this handler only decodes;
//! the game-domain SCEventUpdate system (registered after this plugin) consumes them via BlockChangeQueue.

use sc_block::position::BlockPosition;
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;
use sc_utils::schedule::SCEventUpdate;

use crate::protocol::client::container::ContainerClose;
use crate::protocol::client::interact::{Interact, InteractAction};
use crate::protocol::client::transaction::{InventoryTransaction, ItemUseAction, TransactionType};
use crate::protocol::recv::MinecraftPacketReceiver;

pub struct SCInteractionHandlerPlugin;

impl Plugin for SCInteractionHandlerPlugin {
    fn build(&self, app: &App) {
        app.add_systems(
            SCEventUpdate,
            (
                handle_inventory_transaction,
                handle_interact,
                handle_container_close,
            ),
        );
    }
}

fn handle_interact(world: World, mut reader: EventReader<MinecraftPacketReceiver<Interact>>) {
    for event in reader.read() {
        if event.packet.action != InteractAction::OPEN_INVENTORY {
            continue;
        }
        world.send_event(sc_game::interaction::OpenInventoryRequest {
            entity: event.entity,
            target_entity_id: event.packet.target,
        });
    }
}

fn handle_container_close(
    world: World,
    mut reader: EventReader<MinecraftPacketReceiver<ContainerClose>>,
) {
    for event in reader.read() {
        log::debug!(
            "[interaction] ContainerClose: window_id={} container_type={} was_server_initiated={}",
            event.packet.window_id,
            event.packet.container_type,
            event.packet.was_server_initiated
        );
        world.send_event(sc_game::interaction::CloseInventoryRequest {
            entity: event.entity,
            window_id: event.packet.window_id,
            container_type: event.packet.container_type,
            was_server_initiated: event.packet.was_server_initiated,
        });
    }
}

/// Called once per received packet, preserving cross-packet and cross-type order.
pub(crate) fn enqueue_ordered_mining(
    world: &World,
    entity: sc_ecs::entity::EntityId,
    packet: &crate::protocol::MinecraftPackets,
) -> bool {
    use crate::player_connection::{PlayerConnection, PlayerConnectionStatus};
    use crate::protocol::MinecraftPackets;
    use sc_game::interaction::{MiningInbox, OrderedMiningAction};
    if !matches!(packet, MinecraftPackets::PlayerAuthInput(input) if !input.block_actions.is_empty())
        && !matches!(
            packet,
            MinecraftPackets::PlayerAction(_)
                | MinecraftPackets::MobEquipment(_)
                | MinecraftPackets::InventoryTransaction(_)
        )
    {
        return true;
    }
    let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
        return true;
    };
    if !matches!(
        connection.get_status(),
        PlayerConnectionStatus::InGame | PlayerConnectionStatus::Spawned
    ) {
        return true;
    }
    let Some(world_id) = world.get_component::<sc_world::manager::MinecraftWorldId>(&entity) else {
        return true;
    };
    if world.get_component::<MiningInbox>(&entity).is_none() {
        world.add_component(&entity, MiningInbox::default());
    }
    let Some(inbox) = world.get_component::<MiningInbox>(&entity) else {
        return true;
    };
    let epoch = sc_game::interaction::mining_context_epoch(world, &entity);
    translate_mining_packet(packet, |action| {
        inbox.try_push(OrderedMiningAction {
            world_id: world_id.as_ref().clone(),
            context_epoch: epoch,
            action,
        })
    })
}

fn translate_mining_packet(
    packet: &crate::protocol::MinecraftPackets,
    mut admit: impl FnMut(sc_game::interaction::MiningAction) -> bool,
) -> bool {
    use crate::handler::movement::{translate_block_action, BlockActionIntent};
    use crate::protocol::MinecraftPackets;
    use sc_game::interaction::MiningAction;
    let translated = |intent| match intent {
        BlockActionIntent::Start { position, face } => MiningAction::Start { position, face },
        BlockActionIntent::Abort { position } => MiningAction::Abort { position },
        BlockActionIntent::Break {
            position,
            face,
            require_breaking_state,
        } => MiningAction::Break {
            position,
            face,
            require_breaking_state,
        },
    };
    match packet {
        MinecraftPackets::PlayerAuthInput(input) => {
            for action in &input.block_actions {
                if let Some(intent) = translate_block_action(action) {
                    if !admit(translated(intent)) {
                        return false;
                    }
                }
            }
        }
        MinecraftPackets::PlayerAction(action) => {
            let block_action = crate::protocol::client::movement::PlayerBlockAction {
                action_type: action.action,
                block_x: action.block_x,
                block_y: action.block_y,
                block_z: action.block_z,
                face: action.face,
            };
            if let Some(intent) = translate_block_action(&block_action) {
                return admit(translated(intent));
            }
        }
        MinecraftPackets::InventoryTransaction(transaction)
            if transaction.transaction_type == TransactionType::ITEM_USE
                && transaction.action_type == ItemUseAction::BREAK =>
        {
            return admit(MiningAction::Break {
                position: BlockPosition::new(
                    transaction.block_x,
                    transaction.block_y,
                    transaction.block_z,
                ),
                face: transaction.face,
                require_breaking_state: false,
            });
        }
        MinecraftPackets::MobEquipment(equipment) if equipment.window_id == 0 => {
            return admit(MiningAction::HeldSlot {
                slot: equipment.hotbar_slot,
            });
        }
        _ => {}
    }
    true
}

fn handle_inventory_transaction(
    world: World,
    mut reader: EventReader<MinecraftPacketReceiver<InventoryTransaction>>,
) {
    for event in reader.read() {
        let packet = &event.packet;
        log::debug!(
            "[interaction] InventoryTransaction: type={} hasValue={} actionType={} pos=({},{},{}) face={} slot={} item={}",
            packet.transaction_type, packet.has_value, packet.action_type,
            packet.block_x, packet.block_y, packet.block_z, packet.face, packet.slot,
            packet.item.runtime_id
        );
        if packet.transaction_type == TransactionType::ITEM_USE {
            match packet.action_type {
                ItemUseAction::CLICK_BLOCK => {
                    world.send_event(sc_game::interaction::PlaceBlockRequest {
                        entity: event.entity,
                        position: BlockPosition::new(
                            packet.block_x,
                            packet.block_y,
                            packet.block_z,
                        ),
                        face: packet.face,
                        hotbar_slot: packet.slot,
                        item_id: packet.item.runtime_id as i32,
                    });
                }
                // BREAK is translated by the ordered receive path.
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::client::action::{PlayerAction, PlayerActionType};
    use crate::protocol::client::movement::{PlayerAuthInput, PlayerBlockAction, PlayerInputMode};
    use crate::protocol::MinecraftPackets;
    use sc_game::interaction::MiningAction;

    fn input(actions: Vec<PlayerBlockAction>) -> MinecraftPackets {
        let zero = sc_utils::game::structs::position::MinecraftPosition::new(0.0, 0.0, 0.0);
        MinecraftPackets::PlayerAuthInput(PlayerAuthInput {
            item_stack_request: None,
            pitch: 0.0,
            yaw: 0.0,
            position: zero.clone(),
            move_vector_x: 0.0,
            move_vector_z: 0.0,
            head_yaw: 0.0,
            input_data: Default::default(),
            block_actions: actions,
            input_mode: PlayerInputMode::Mouse,
            play_mode: 0,
            interaction_model: 0,
            interact_rotation_x: 0.0,
            interact_rotation_z: 0.0,
            tick: 1,
            delta: zero,
            analog_move_x: 0.0,
            analog_move_y: 0.0,
            camera_orientation_x: 0.0,
            camera_orientation_y: 0.0,
            camera_orientation_z: 0.0,
            raw_move_x: 0.0,
            raw_move_y: 0.0,
        })
    }

    fn action(action_type: i32, x: i32) -> PlayerBlockAction {
        PlayerBlockAction {
            action_type,
            block_x: x,
            block_y: 64,
            block_z: 0,
            face: 1,
        }
    }

    #[test]
    fn auth_input_and_player_action_keep_wire_order_and_abort_position() {
        let first = input(vec![
            action(PlayerActionType::PREDICT_DESTROY_BLOCK, 0),
            action(PlayerActionType::START_BREAK, 1),
        ]);
        let second = MinecraftPackets::PlayerAction(PlayerAction {
            runtime_entity_id: 1,
            action: PlayerActionType::ABORT_BREAK,
            block_x: 1,
            block_y: 64,
            block_z: 0,
            result_x: 0,
            result_y: 0,
            result_z: 0,
            face: -1,
        });
        let mut actions = Vec::new();
        for packet in [&first, &second] {
            assert!(translate_mining_packet(packet, |action| {
                actions.push(action);
                true
            }));
        }
        assert_eq!(actions.len(), 3);
        assert!(
            matches!(actions[0], MiningAction::Break { position, require_breaking_state: true, .. } if position.x == 0)
        );
        assert!(matches!(actions[1], MiningAction::Start { position, .. } if position.x == 1));
        assert!(matches!(actions[2], MiningAction::Abort { position } if position.x == 1));
    }

    #[test]
    fn ordered_packet_admission_stops_at_first_overflow() {
        let packet = input(vec![
            action(PlayerActionType::START_BREAK, 0),
            action(PlayerActionType::ABORT_BREAK, 0),
            action(PlayerActionType::START_BREAK, 1),
        ]);
        let mut count = 0;
        assert!(!translate_mining_packet(&packet, |_| {
            count += 1;
            count < 2
        }));
        assert_eq!(count, 2);
    }
}
