//! 玩家交互数据包转换：挖掘/切换槽位在接收循环内进入每玩家的有界 FIFO，
//! 放置/容器操作则走边界事件。
//!
//! - `PlayerAction`（0x24）START_BREAK -> [`StartBreakRequest`]（记录 BreakingState）
//! - `PlayerAction`（0x24）STOP_BREAK -> [`BreakBlockRequest`]（require_breaking_state=true）
//! - `PlayerAction`（0x24）ABORT_BREAK -> [`AbortBreakRequest`]（清除 BreakingState）
//! - `PlayerAction`（0x24）CREATIVE_DESTROY -> [`BreakBlockRequest`]（require_breaking_state=false）
//! - `PlayerAction`（0x24）RESPAWN -> 游戏侧 `RespawnRequest`（死亡界面按钮）
//! - `Respawn`（0x2d）客户端请求 -> 游戏侧 `RespawnRequest`（当前客户端的死亡界面
//!   按钮：零坐标 + 出生进度 state）
//! - `InventoryTransaction`（0x1e）USE_ITEM ClickBlock -> [`PlaceBlockRequest`]
//! - `MobEquipment`（0x1f）快捷栏切换 -> [`HeldSlotChanged`]
//!
//! 边界事件位于 sc_game::interaction（游戏域）；本处理器只负责解码；游戏域的
//! SCEventUpdate 系统（在本插件之后注册）通过 BlockChangeQueue 消费它们。
//!
//! Player interaction packet translation: digging/slot-switches enter a per-player
//! bounded FIFO inside the receive loop, while place/container ops stay boundary
//! events.
//!
//! - `PlayerAction` (0x24) START_BREAK -> [`StartBreakRequest`] (records BreakingState)
//! - `PlayerAction` (0x24) STOP_BREAK -> [`BreakBlockRequest`] (require_breaking_state=true)
//! - `PlayerAction` (0x24) ABORT_BREAK -> [`AbortBreakRequest`] (clears BreakingState)
//! - `PlayerAction` (0x24) CREATIVE_DESTROY -> [`BreakBlockRequest`] (require_breaking_state=false)
//! - `PlayerAction` (0x24) RESPAWN -> game `RespawnRequest` (death-screen button)
//! - `Respawn` (0x2d) client request -> game `RespawnRequest` (death-screen
//!   button on current clients: zero position + spawn-progress state)
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
                handle_player_action_respawn,
                handle_player_action_swimming,
                handle_respawn_packet,
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
        if let Some(conn) = world.get_component::<crate::player_connection::PlayerConnection>(&event.entity) {
            conn.clear_container_window(event.packet.window_id);
        }
        world.send_event(sc_game::interaction::CloseInventoryRequest {
            entity: event.entity,
            window_id: event.packet.window_id,
            container_type: event.packet.container_type,
            was_server_initiated: event.packet.was_server_initiated,
        });
    }
}

/// 每收到一个数据包调用一次，保持跨数据包与跨类型的顺序。
///
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

/// 死亡界面的重生按钮：`PlayerAction(RESPAWN)` 转换为与协议无关的游戏请求；
/// 游戏侧在执行前先校验 `Dead`，因此多余或重复的点击是无害的。
///
/// Death-screen respawn button: `PlayerAction(RESPAWN)` becomes a
/// protocol-independent game request; the game validates `Dead` before
/// acting, so stray/duplicate clicks are harmless.
fn handle_player_action_respawn(
    world: World,
    mut reader: EventReader<MinecraftPacketReceiver<crate::protocol::client::action::PlayerAction>>,
) {
    use crate::protocol::client::action::PlayerActionType;
    for event in reader.read() {
        if event.packet.action != PlayerActionType::RESPAWN {
            continue;
        }
        log::debug!("[interaction] RespawnRequest from {:?}", event.entity);
        world.send_event(sc_game::respawn::RespawnRequest {
            entity: event.entity,
        });
    }
}

/// 回退/旧版游泳开关：`PlayerAction(START_SWIMMING / STOP_SWIMMING)` 转换为
/// 游戏侧的 `PlayerInput` 并入队，等待移动系统消费。
///
/// Fallback / legacy swimming toggle: `PlayerAction(START_SWIMMING / STOP_SWIMMING)`
/// translated into game-side `PlayerInput` and enqueued for movement drain.
fn handle_player_action_swimming(
    world: World,
    mut reader: EventReader<MinecraftPacketReceiver<crate::protocol::client::action::PlayerAction>>,
) {
    use crate::protocol::client::action::PlayerActionType;
    use sc_entity::motion::{Position, Transform};
    use sc_game::movement::{PlayerMovement, MAX_PENDING_INPUT};
    use sc_game::net::PlayerInput;

    for event in reader.read() {
        let is_start = event.packet.action == PlayerActionType::START_SWIMMING;
        let is_stop = event.packet.action == PlayerActionType::STOP_SWIMMING;
        if !is_start && !is_stop {
            continue;
        }
        let Some(movement) = world.get_component::<PlayerMovement>(&event.entity) else {
            continue;
        };
        let (pos, rot) = if let Some(transform) = world.get_component::<Transform>(&event.entity) {
            let t = transform.read();
            (t.position, t.rotation)
        } else {
            let state = movement.state.read();
            (
                state.last_position.unwrap_or_else(|| Position::new(0.0, 0.0, 0.0)),
                state.last_rotation,
            )
        };
        let input = PlayerInput {
            feet_x: pos.x,
            feet_y: pos.y,
            feet_z: pos.z,
            yaw: rot.yaw,
            pitch: rot.pitch,
            head_yaw: rot.head_yaw,
            move_x: 0.0,
            move_z: 0.0,
            sneaking: false,
            jumping: false,
            sprinting: false,
            flying: false,
            start_swimming: is_start,
            stop_swimming: is_stop,
            tick: 0,
        };
        let mut state = movement.state.write();
        if state.input_buffer.len() < MAX_PENDING_INPUT {
            state.push_input(input, MAX_PENDING_INPUT);
        }
    }
}

/// 死亡界面的重生按钮（数据包形式）：客户端发送位置为零、并带上自身出生进度
/// state 的 `Respawn`。任意 state 都视为重生请求；游戏侧在执行前先校验 `Dead`。
///
/// Death-screen respawn button (packet form): the client sends `Respawn`
/// with a zero position and its spawn-progress state. Any state acts as the
/// request; the game validates `Dead` before acting.
fn handle_respawn_packet(
    world: World,
    mut reader: EventReader<MinecraftPacketReceiver<crate::protocol::server::misc::Respawn>>,
) {
    for event in reader.read() {
        log::debug!(
            "[interaction] RespawnRequest packet state={} from {:?}",
            event.packet.state,
            event.entity
        );
        world.send_event(sc_game::respawn::RespawnRequest {
            entity: event.entity,
        });
    }
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
                        click_y: packet.click_y,
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
