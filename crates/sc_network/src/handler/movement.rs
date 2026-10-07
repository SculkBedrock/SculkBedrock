//! Network-side movement input handling (game logic lives in `sc_game::movement`; this module only translates).
//!
//! `player_auth_input` (SCConnectionUpdate): decodes `PlayerAuthInput` ->
//! translates to game-side [`sc_game::net::PlayerInput`] -> pushes into the player
//! `PlayerMovement` (sc_game) input buffer.
//! State gating is kept (only InGame is accepted); authoritative validation is
//! consumed by game-side `movement_input_drain` (inbound boundary in docs/multi_world_ecs.md).

use log::warn;
use sc_block::position::BlockPosition;
use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;
use sc_game::movement::{PlayerMovement, MAX_PENDING_INPUT};
use sc_game::net::PlayerInput;
use std::sync::Arc;

use crate::player_connection::{PlayerConnection, PlayerConnectionStatus};
use crate::protocol::client::action::PlayerActionType;
use crate::protocol::client::movement::{PlayerAuthInput, PlayerBlockAction};
use crate::protocol::recv::MinecraftPacketReceiver;
use sc_log::t_log;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockActionIntent {
    Start {
        position: BlockPosition,
        face: u8,
    },
    Abort {
        position: BlockPosition,
    },
    Break {
        position: BlockPosition,
        face: u8,
        require_breaking_state: bool,
    },
}

pub(crate) fn translate_block_action(action: &PlayerBlockAction) -> Option<BlockActionIntent> {
    let position = BlockPosition::new(action.block_x, action.block_y, action.block_z);
    if action.action_type == PlayerActionType::ABORT_BREAK {
        return Some(BlockActionIntent::Abort { position });
    }
    let face = u8::try_from(action.face).ok()?;

    match action.action_type {
        PlayerActionType::START_BREAK => Some(BlockActionIntent::Start { position, face }),
        PlayerActionType::ABORT_BREAK => Some(BlockActionIntent::Abort { position }),
        PlayerActionType::STOP_BREAK | PlayerActionType::PREDICT_DESTROY_BLOCK => {
            Some(BlockActionIntent::Break {
                position,
                face,
                require_breaking_state: true,
            })
        }
        PlayerActionType::CREATIVE_PLAYER_DESTROY_BLOCK => Some(BlockActionIntent::Break {
            position,
            face,
            require_breaking_state: false,
        }),
        PlayerActionType::CONTINUE_DESTROY_BLOCK => {
            Some(BlockActionIntent::Start { position, face })
        }
        _ => None,
    }
}

pub(crate) fn player_auth_input(
    world: World,
    mut event_reader: EventReader<MinecraftPacketReceiver<PlayerAuthInput>>,
) {
    for event in event_reader.read() {
        let Some(connection) = world.get_component::<PlayerConnection>(&event.entity) else {
            continue;
        };
        if !matches!(
            connection.get_status(),
            PlayerConnectionStatus::InGame
                | PlayerConnectionStatus::Spawned
                | PlayerConnectionStatus::AwaitingClientInitialization,
        ) {
            continue;
        }
        // Ensure the component exists (normally mounted by create_player; re-mounted here as a fallback).
        let movement = match world.get_component::<PlayerMovement>(&event.entity) {
            Some(movement) => movement,
            None => {
                warn!("{}", t_log!("console.movement.missing_component"));
                let movement = PlayerMovement::new();
                world.add_component(&event.entity, movement.clone());
                Arc::new(movement)
            }
        };
        // Protocol -> game-side input values (inbound boundary translation).
        let packet = &event.packet;
        let feet = packet.feet_position();
        let input = PlayerInput {
            feet_x: feet.x,
            feet_y: feet.y,
            feet_z: feet.z,
            yaw: packet.yaw,
            pitch: packet.pitch,
            head_yaw: packet.head_yaw,
            move_x: packet.move_vector_x,
            move_z: packet.move_vector_z,
            sneaking: packet.sneaking(),
            jumping: packet.jumping(),
            sprinting: packet.sprinting(),
            flying: packet.flying(),
            tick: packet.tick,
        };
        // Hold the state lock briefly (never across await, never on hot resources).
        {
            let mut state = movement.state.write();
            if state.input_buffer.len() >= MAX_PENDING_INPUT {
                warn!(
                    "{}",
                    t_log!("console.movement.input_overflow", max = MAX_PENDING_INPUT)
                );
            } else {
                state.push_input(input, MAX_PENDING_INPUT);
            }
        }

        // Mining actions are admitted by the receive loop, before packet-type
        // event readers can reorder them.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(action_type: i32) -> PlayerBlockAction {
        PlayerBlockAction {
            action_type,
            block_x: 1,
            block_y: 64,
            block_z: -2,
            face: 3,
        }
    }

    #[test]
    fn translates_player_auth_input_break_actions() {
        assert!(matches!(
            translate_block_action(&action(PlayerActionType::START_BREAK)),
            Some(BlockActionIntent::Start { .. })
        ));
        assert_eq!(
            translate_block_action(&action(PlayerActionType::ABORT_BREAK)),
            Some(BlockActionIntent::Abort {
                position: BlockPosition::new(1, 64, -2)
            })
        );
        assert!(matches!(
            translate_block_action(&action(PlayerActionType::PREDICT_DESTROY_BLOCK)),
            Some(BlockActionIntent::Break {
                require_breaking_state: true,
                ..
            })
        ));
    }

    #[test]
    fn ignores_non_break_player_auth_input_actions() {
        assert_eq!(
            translate_block_action(&action(PlayerActionType::DROP_ITEM)),
            None
        );
    }
}
