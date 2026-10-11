//! Command network handlers:
//! - `command_request`: CommandRequest packet -> RawCommandInput event in sc_command;
//! - `command_feedback`: CommandFeedback event from sc_command -> CommandOutput packet;
//! - `gamemode_command`: /gamemode execution (player game-mode switch).
//! - `give_command`: /give execution (testing grant + slot sync).
//!
//! Command parsing and permission gates live in sc_command; /gamemode state changes
//! and protocol sync live in the network layer, since sc_command must not depend on
//! sc_entity/sc_network (would be a cyclic dependency). This module has full

use sc_command::events::{
    CommandFeedback, CommandInvocation, CommandOrigin, CommandRequestContext, RawCommandInput,
};
use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;

use crate::player_connection::{PlayerConnection, PlayerConnectionStatus};
use crate::protocol::client::command::CommandRequest;
use crate::protocol::recv::MinecraftPacketReceiver;
use crate::protocol::server::command::{CommandOutput, CommandOutputMessage};
use crate::utils::ConnectionThreadManager;

pub(crate) fn command_request(
    world: World,
    mut event_reader: EventReader<MinecraftPacketReceiver<CommandRequest>>,
) {
    for event in event_reader.read() {
        let entity = event.entity;
        let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
            continue;
        };
        if connection.get_status() != PlayerConnectionStatus::InGame {
            continue;
        }
        world.send_event(RawCommandInput {
            origin: CommandOrigin::Player(entity),
            raw: event.packet.command.clone(),
            request: Some(CommandRequestContext {
                origin_type: 0, // Player-issued command origin (PLAYER)
                uuid: event.packet.uuid,
                request_id: event.packet.request_id.clone(),
            }),
        });
    }
}

pub(crate) fn command_feedback(world: World, mut event_reader: EventReader<CommandFeedback>) {
    for feedback in event_reader.read() {
        let CommandOrigin::Player(entity) = feedback.origin else {
            continue; // Console feedback is handled by the sc_command logging system
        };
        let Some(request) = feedback.request.clone() else {
            continue;
        };
        let messages = feedback.messages.clone();
        let success = feedback.success;
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            if let Some(connection) = world.get_component::<PlayerConnection>(&entity) {
                // CommandOutputPacket with the origin echoed back:
                // CommandOriginData(PLAYER, uuid, requestId, playerId=-1), ALL_OUTPUT.
                // (Previously fell back to TextPacket after an origin-encoding mismatch; the
                // standard receipt path is restored.)
                let _ = connection
                    .send_packet(
                        CommandOutput {
                            origin_type: "Player".to_string(),
                            uuid: request.uuid,
                            request_id: request.request_id,
                            player_id: -1,
                            output_type: CommandOutput::TYPE_ALL_OUTPUT,
                            success_count: success as u32,
                            messages: messages
                                .into_iter()
                                .map(|text| CommandOutputMessage {
                                    success,
                                    // Non-translated text goes in message_id: shown verbatim when no key matches.
                                    message_id: text,
                                    parameters: Vec::new(),
                                })
                                .collect(),
                            data_set: String::new(),
                        },
                        true,
                    )
                    .await;
            }
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}

/// Runs /gamemode: switches a player game mode and syncs the client.
///
/// Usage: `/gamemode <survival|creative|adventure|spectator> [player]`.
/// Accepts vanilla aliases (`Gamemode::from_str`: 0/s, 1/c, 2/a, 6/sp).
/// Without a target, switches self (player-issued); console must name a target.
/// Changing someone else requires the invoker to be OP (`MinecraftClient.data.is_op`);
/// changing self is allowed to ease unit/integration testing.
pub(crate) fn gamemode_command(world: World, mut reader: EventReader<CommandInvocation>) {
    use sc_entity::player::adventure_settings::{AdventureSettings, AdventureSettingsType};
    use sc_utils::components::DisplayName;
    use sc_utils::game::client::MinecraftClient;
    use sc_utils::game::gamemode::Gamemode;

    for invocation in reader.read() {
        if invocation.command != "gamemode" {
            continue;
        }
        let Some(mode) = invocation
            .args
            .first()
            .and_then(|arg| Gamemode::from_str(arg))
        else {
            world.send_event(CommandFeedback::error_from(
                invocation,
                "Usage: /gamemode <survival|creative|adventure|spectator> [player]",
            ));
            continue;
        };

        // Target resolution: a name finds an online player by DisplayName; unnamed means the invoker.
        let target = if let Some(name) = invocation.args.get(1) {
            find_player_by_name(&world, name)
        } else if let CommandOrigin::Player(entity) = invocation.origin {
            Some(entity)
        } else {
            None
        };
        let Some(target) = target else {
            let message = if invocation.args.len() >= 2 {
                format!("Player '{}' not found.", invocation.args[1])
            } else {
                "Specify a player: /gamemode <mode> <player>".to_string()
            };
            world.send_event(CommandFeedback::error_from(invocation, message));
            continue;
        };

        // Changing others requires OP; changing self is allowed.
        if let CommandOrigin::Player(invoker) = invocation.origin {
            if invoker != target
                && !world
                    .get_component::<MinecraftClient>(&invoker)
                    .is_some_and(|client| client.data.read().is_op)
            {
                world.send_event(CommandFeedback::error_from(
                    invocation,
                    "You do not have permission to change another player's game mode.",
                ));
                continue;
            }
        }

        let target_name = world
            .get_component::<DisplayName>(&target)
            .map(|name| name.0.clone())
            .unwrap_or_else(|| format!("{target:?}"));
        let Some(client) = world.get_component::<MinecraftClient>(&target) else {
            world.send_event(CommandFeedback::error_from(
                invocation,
                format!("Player '{target_name}' is not ready."),
            ));
            continue;
        };
        if client.data.read().gamemode == mode {
            world.send_event(CommandFeedback::success_from(
                invocation,
                format!("{target_name} is already in {} mode.", mode.as_str()),
            ));
            continue;
        }
        client.data.write().gamemode = mode;
        if let Some(settings) = world.get_component::<AdventureSettings>(&target) {
            settings
                .set(
                    AdventureSettingsType::WorldImmutable,
                    mode.is_adventure() || mode.is_spectator(),
                )
                .set(
                    AdventureSettingsType::WorldBuilder,
                    !mode.is_adventure() && !mode.is_spectator(),
                )
                .set(
                    AdventureSettingsType::AllowFlight,
                    mode.is_creative() || mode.is_spectator(),
                )
                .set(AdventureSettingsType::NoClip, mode.is_spectator())
                .set(AdventureSettingsType::Flying, mode.is_spectator());
        }

        world.send_event(CommandFeedback::success_from(
            invocation,
            format!("Set game mode to {} for {}.", mode.as_str(), target_name),
        ));

        // Protocol sync: SetPlayerGameType(0x3e) + UpdateAbilities/UpdateAdventureSettings.
        let world_clone = world.clone();
        let manager_world = world.clone();
        let gamemode_id = mode.to_i32();
        let handle = SCECSAsync::runtime().spawn(async move {
            let Some(connection) = world_clone.get_component::<PlayerConnection>(&target) else {
                return;
            };
            let _ = connection
                .send_packet(
                    crate::protocol::server::game::SetPlayerGameType {
                        gamemode: gamemode_id,
                    },
                    true,
                )
                .await;
            if let Some(settings) = world_clone.get_component::<AdventureSettings>(&target) {
                use crate::utils::adventure_settings::SCNetworkAdventureSettings;
                settings.update().await;
            }
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(target, handle);
        }
    }
}

/// Finds an online player by name (case-insensitive).
fn find_player_by_name(world: &World, name: &str) -> Option<sc_ecs::entity::EntityId> {
    use sc_utils::components::DisplayName;
    for entity in world.entities_with_component::<PlayerConnection>() {
        if world
            .get_component::<DisplayName>(&entity)
            .is_some_and(|display| display.0.eq_ignore_ascii_case(name))
        {
            return Some(entity);
        }
    }
    None
}

/// Runs /give: testing grant of version-pack items with authoritative slot sync.
///
/// Players: `/give <item> [count]` targets self; console: `give <player>
/// <item> [count]` targets by display name (case-insensitive). Bare names
/// default to the minecraft namespace. Count clamps to 1..=64.
///
/// Lives here (not in a version-pack plugin) for two reasons: the grant
/// touches entity components, which do not cross the cdylib boundary
/// (`Any` downcast fails plugin-side), and plugin schedule systems only
/// started running after the label-identity fix. Permission stays `Any`
/// until the player permission component lands; tighten to Host/Admin then.
pub(crate) fn give_command(world: World, mut reader: EventReader<CommandInvocation>) {
    use sc_command::events::CommandOrigin;
    for invocation in reader.read() {
        if invocation.command != "give" {
            continue;
        }
        // Players give to themselves; console names a target player.
        let (target, item_index) = match invocation.origin {
            CommandOrigin::Player(entity) => (Some(entity), 0),
            CommandOrigin::Console => match invocation.args.as_slice() {
                [_, _, ..] => {
                    let name = &invocation.args[0];
                    match find_player_by_name(&world, name) {
                        Some(entity) => (Some(entity), 1),
                        None => {
                            world.send_event(CommandFeedback::error_from(
                                invocation,
                                format!("no online player named {name}"),
                            ));
                            continue;
                        }
                    }
                }
                _ => {
                    world.send_event(CommandFeedback::error_from(
                        invocation,
                        "Usage: /give <player> <item> [count]",
                    ));
                    continue;
                }
            },
        };
        let (Some(target), Some(name)) = (target, invocation.args.get(item_index)) else {
            world.send_event(CommandFeedback::error_from(
                invocation,
                "Usage: /give <item> [count]",
            ));
            continue;
        };
        let count: u16 = invocation
            .args
            .get(item_index + 1)
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(1)
            .clamp(1, 64);
        give_to(&world, invocation, target, name, count);
    }
}

fn give_to(
    world: &World,
    invocation: &CommandInvocation,
    target: sc_ecs::entity::EntityId,
    name: &str,
    count: u16,
) {
    // Bare names default to the minecraft namespace (`coal` ==
    // `minecraft:coal`), matching vanilla command UX.
    let identifier = if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    };
    let (Some(inventory), Some(items)) = (
        world.get_component::<sc_item::PlayerInventory>(&target),
        world.get_resource::<sc_item::ItemRegistry>(),
    ) else {
        world.send_event(CommandFeedback::error_from(
            invocation,
            "give failed: inventory or item registry missing",
        ));
        return;
    };
    let Some(runtime_id) = items.runtime_id_by_name(&identifier) else {
        world.send_event(CommandFeedback::error_from(
            invocation,
            format!("unknown item: {identifier}"),
        ));
        return;
    };
    let stack = sc_item::ItemStack::new(runtime_id, count);
    let (leftover, changed) = inventory.insert_tracked(stack, &items);
    // Authoritative slot sync (same intent the pickup path uses, so the
    // client sees new items without reopening the inventory).
    let runtime = world
        .get_component::<sc_entity::MinecraftEntityId>(&target)
        .map(|actor| actor.0);
    if let (Some(runtime), Some(mut outbox)) = (
        runtime,
        world.get_resource_mut::<sc_game::net::NetworkOutbox>(),
    ) {
        let mut publisher = sc_game::net::IntentPublisher::new(&mut outbox).with_world(world);
        for slot in changed {
            if let Some(current) = inventory.get(slot) {
                publisher.publish(sc_game::net::UpdateInventorySlot {
                    entity_id: runtime,
                    slot: slot as u8,
                    stack: current,
                });
            }
        }
    }
    if leftover > 0 {
        world.send_event(CommandFeedback::error_from(
            invocation,
            format!("inventory full, {leftover} x {identifier} left"),
        ));
    } else {
        log::info!("[Server] gave {count} x {identifier}");
        world.send_event(CommandFeedback::success_from(
            invocation,
            format!("gave {count} x {identifier}"),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_ecs::app::App;

    /// The live /give failure: dispatch ran but no feedback and no items ever
    /// appeared. Drives the real host chain (dispatch on SCConnectionUpdate ->
    /// give on SCEventUpdate) and requires player feedback plus inventory.
    #[test]
    fn give_flows_from_raw_input_to_feedback_and_inventory() {
        use sc_command::events::RawCommandInput;
        use sc_ecs::event::reader::ManualEventReader;
        use sc_ecs::event::Events;

        let app = App::new();
        app.add_plugins(sc_utils::schedule::SCSchedulePlugin);
        app.add_plugins(sc_command::SCCommandPlugin);
        app.add_systems(sc_utils::schedule::SCEventUpdate, give_command);

        let items = sc_item::ItemRegistry::new();
        items.upsert(sc_item::ItemDefinition::new(5, "minecraft:potato"));
        app.world().insert_resource(items);
        app.world()
            .insert_resource(sc_game::net::NetworkOutbox::with_limits(64, 64, usize::MAX));

        let entity = app.world().spawn(sc_item::PlayerInventory::new(36));
        app.world()
            .add_component(&entity, sc_entity::MinecraftEntityId(7));

        // Build queued plugins (registers schedules/events), like the
        // production runner does before the tick loop. The first update
        // runs startup schedules only, and each tick swaps event buffers,
        // so the input must be sent after a warmup tick: otherwise it is
        // dropped before the main schedules ever see it.
        app.finish();
        app.cleanup();
        app.update();
        app.world().send_event(RawCommandInput {
            origin: sc_command::events::CommandOrigin::Player(entity),
            raw: "/give potato 2".to_string(),
            request: None,
        });
        app.update();
        app.update();

        let invocations = app
            .world()
            .get_resource::<Events<sc_command::events::CommandInvocation>>()
            .expect("invocation events");
        let mut ireader = ManualEventReader::default();
        let seen: Vec<String> = ireader
            .read(&invocations)
            .map(|event| event.command.clone())
            .collect();
        assert!(
            seen.iter().any(|command| command == "give"),
            "dispatch must produce a give invocation, saw {seen:?}",
        );

        let feedback = app
            .world()
            .get_resource::<Events<CommandFeedback>>()
            .expect("feedback events");
        let mut reader = ManualEventReader::default();
        let messages: Vec<String> = reader
            .read(&feedback)
            .flat_map(|event| event.messages.clone())
            .collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("gave 2 x minecraft:potato")),
            "player must see give feedback, got {messages:?}",
        );

        let inventory = app
            .world()
            .get_component::<sc_item::PlayerInventory>(&entity)
            .expect("inventory");
        let stack = inventory.get(0).expect("given slot");
        assert_eq!(stack.runtime_id, 5);
        assert_eq!(stack.count, 2);
    }
}
