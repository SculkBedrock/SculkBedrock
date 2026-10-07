use log::{debug, warn};
use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::entity::EntityId;
use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;
use sc_log::t_log;
use sc_utils::components::DisplayName;
use sc_utils::game::client::MinecraftClient;

use crate::client::MinecraftClientNetwork;

use crate::player_connection::{PlayerConnection, PlayerConnectionStatus};
use crate::protocol::client::login::SetLocalPlayerAsInitialized;
use crate::protocol::client::world::{RequestChunkRadius, ServerboundLoadingScreen};
use crate::protocol::recv::MinecraftPacketReceiver;
use crate::protocol::server::chunk::ChunkRadiusUpdated;
use crate::protocol::server::misc::{PlayerList, PlayerListEntry};
use crate::protocol::server::player::AddPlayer;
use crate::utils::ConnectionThreadManager;
use sc_entity::MinecraftEntityId;
use sc_world::chunk::ChunkPosition;
use sc_world::chunk_view::{ChunkSendSettings, ChunkView};
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};

/// Handle a client chunk radius request (RequestChunkRadius).
///
/// Only attach/update the ChunkView (the chunk pipeline takes over sending);
/// reply with ChunkRadiusUpdated plus a NetworkChunkPublisherUpdate.
pub(crate) fn request_chunk_radius(
    world: World,
    mut event_reader: EventReader<MinecraftPacketReceiver<RequestChunkRadius>>,
) {
    for event in event_reader.read() {
        let entity = event.entity;
        let (view_distance, spawn_threshold) = world
            .get_resource::<ChunkSendSettings>()
            .map(|settings| (settings.view_distance, settings.spawn_threshold))
            .unwrap_or((10, 56));
        // Clamp radius to max(2, min(requested, viewDistance)).
        let requested = event.packet.radius.clamp(2, view_distance.max(2));
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
                return;
            };
            if !connection.get_status().accepts_chunk_radius() {
                debug!(
                    "[Player] RequestChunkRadius ignored in status {:?}",
                    connection.get_status()
                );
                return;
            }
            connection.set_chunk_radius(requested);
            let Some(client) = world.get_component::<MinecraftClient>(&entity) else {
                return;
            };
            let Some(world_id) = world.get_component::<MinecraftWorldId>(&entity) else {
                return;
            };
            let dimension = {
                let Some(manager) = world.get_resource::<MinecraftWorldManager>() else {
                    return;
                };
                let Some(minecraft_world) = manager.get_world(world_id.as_ref()) else {
                    return;
                };
                minecraft_world.world_data.get_dimension()
            };
            let pos = client.data.read().position;
            let center = ChunkPosition::from_world(pos.x.floor() as i32, pos.z.floor() as i32);
            debug!(
                "[Player] RequestChunkRadius: requested={} (clamped), pos=({},{},{})",
                requested, pos.x, pos.y, pos.z
            );

            // Attach/update the ChunkView (the chunk pipeline takes over sending).
            match world.get_component::<ChunkView>(&entity) {
                Some(view) => {
                    let context_changed = {
                        let data = view.read();
                        data.world_id != *world_id.as_ref() || data.dimension != dimension
                    };
                    if context_changed {
                        // §11.4: a world/dimension switch is an ordered barrier on
                        // this connection. Old-context preparations are refused
                        // before they consume a cipher counter, while everything
                        // already admitted keeps its place in the send queue.
                        if let Some(connection) = world.get_component::<PlayerConnection>(&entity) {
                            connection.begin_context_barrier();
                        }
                        view.reset_subscription(
                            world_id.as_ref().clone(),
                            dimension,
                            requested,
                            center,
                        );
                    } else {
                        // A requested-radius adjustment changes the view, not
                        // the world/context. Preserve used and in-flight overlap.
                        view.write().update_view(requested, center);
                    }
                }
                None => {
                    world.add_component(
                        &entity,
                        ChunkView::new(world_id.as_ref().clone(), dimension, requested, center),
                    );
                }
            }

            if let Some(available) = world
                .get_component::<ChunkView>(&entity)
                .and_then(|view| view.diagnose_unreachable_spawn_threshold(spawn_threshold))
            {
                warn!(
                    "{}",
                    t_log!(
                        "console.chunk.spawn_unreachable",
                        threshold = spawn_threshold,
                        entity = format!("{entity:?}"),
                        requested = requested,
                        available = available
                    )
                );
            }

            // Radius acknowledgement is sent immediately; the view planner
            // queues a versioned publisher update on its next pass.
            let _ = connection
                .send_packet(ChunkRadiusUpdated { radius: requested }, true)
                .await;
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}

/// Modern Bedrock clients send ServerboundLoadingScreen (0x138) during the
/// chunk phase. Treat it as a typed state-machine signal so it is no longer
/// skipped as an unknown high packet.
pub(crate) fn serverbound_loading_screen(
    world: World,
    mut event_reader: EventReader<MinecraftPacketReceiver<ServerboundLoadingScreen>>,
) {
    for event in event_reader.read() {
        if let Some(connection) = world.get_component::<PlayerConnection>(&event.entity) {
            debug!(
                "[Player] ServerboundLoadingScreen received: status={:?}, payload={}B",
                connection.get_status(),
                event.packet.payload.len()
            );
        }
    }
}

/// Client initialization complete (SetLocalPlayerAsInitialized): enter InGame
/// and trigger the player-visibility broadcast (PlayerList + AddPlayer).
pub(crate) fn set_local_player_as_initialized(
    world: World,
    mut event_reader: EventReader<MinecraftPacketReceiver<SetLocalPlayerAsInitialized>>,
) {
    for event in event_reader.read() {
        if let Some(connection) = world.get_component::<PlayerConnection>(&event.entity) {
            if connection.get_status().accepts_local_player_initialized() {
                connection.set_status(PlayerConnectionStatus::InGame);
                if let Some(view) = world.get_component::<ChunkView>(&event.entity) {
                    let mut data = view.inner.write();
                    data.next_order_run = 0;
                }
                debug!("[Player] SetLocalPlayerAsInitialized accepted; chunk pipeline unlocked");
                // Player entered the game: broadcast bidirectional visibility.
                let world = world.clone();
                SCECSAsync::runtime().spawn(broadcast_player_spawn(world, event.entity));
            }
        }
    }
}

/// Player-visibility orchestration:
/// 1) Broadcast to other same-world players: PlayerList ADD + AddPlayer;
/// 2) To the new player: PlayerList ADD + AddPlayer for each existing player.
/// Data comes entirely from the connection layer.
async fn broadcast_player_spawn(world: World, entity: EntityId) {
    let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
        return;
    };
    let Some(client) = world.get_component::<MinecraftClient>(&entity) else {
        return;
    };
    let Some(mc_id) = world.get_component::<MinecraftEntityId>(&entity) else {
        return;
    };
    let Some(world_id) = world.get_component::<MinecraftWorldId>(&entity) else {
        return;
    };
    let Some(display_name) = world.get_component::<DisplayName>(&entity) else {
        return;
    };
    let connection_data = connection.get_data();
    let uuid = connection_data.uuid;
    let xuid = connection_data.xuid.clone();
    let skin = connection_data.skin.clone();
    let build_platform = connection_data.device_os.index();
    let username = display_name.0.clone();
    let runtime_id = mc_id.0;
    let (x, y, z) = {
        let data = client.data.read();
        (data.position.x, data.position.y, data.position.z)
    };
    let new_world_id = world_id.as_ref().clone();

    // New-player PlayerList ADD entry (sent to all other players).
    let new_player_list = PlayerList {
        list_type: PlayerList::TYPE_ADD,
        entries: vec![PlayerListEntry {
            uuid,
            entity_id: runtime_id as i64,
            name: username.clone(),
            xuid: xuid.clone(),
            platform_chat_id: String::new(),
            build_platform,
            skin: skin.clone(),
            ..Default::default()
        }],
    };
    let new_add_player = AddPlayer {
        uuid,
        username: username.clone(),
        entity_runtime_id: runtime_id,
        platform_chat_id: String::new(),
        x,
        y,
        z,
        speed_x: 0.0,
        speed_y: 0.0,
        speed_z: 0.0,
        pitch: 0.0,
        yaw: 0.0,
        head_yaw: 0.0,
        game_type: 0,
        player_permission: 0,
        command_permission: 0,
        device_id: String::new(),
        build_platform,
    };

    // Collect other same-world players (all online players except the new one).
    let mut others: Vec<EntityId> = Vec::new();
    for other in world.entities_with_component::<PlayerConnection>() {
        if other == entity {
            continue;
        }
        if let Some(other_world_id) = world.get_component::<MinecraftWorldId>(&other) {
            if other_world_id.world_id == new_world_id.world_id {
                others.push(other);
            }
        }
    }

    // Send attributes only after client initialization completes
    // (UpdateAttributes), before the visibility broadcast.
    let _ = client.sync_attributes().await;

    // 1) Broadcast the new player to others (PlayerList ADD + AddPlayer).
    for other in &others {
        let Some(other_connection) = world.get_component::<PlayerConnection>(other) else {
            continue;
        };
        let _ = other_connection
            .send_packet(new_player_list.clone(), true)
            .await;
        let _ = other_connection
            .send_packet(new_add_player.clone(), true)
            .await;
    }

    // 2) To the new player: PlayerList ADD + AddPlayer for each existing player.
    for other in &others {
        let Some(other_connection) = world.get_component::<PlayerConnection>(other) else {
            continue;
        };
        let Some(other_client) = world.get_component::<MinecraftClient>(other) else {
            continue;
        };
        let Some(other_mc_id) = world.get_component::<MinecraftEntityId>(other) else {
            continue;
        };
        let Some(other_display) = world.get_component::<DisplayName>(other) else {
            continue;
        };
        let other_conn_data = other_connection.get_data();
        let (ox, oy, oz) = {
            let data = other_client.data.read();
            (data.position.x, data.position.y, data.position.z)
        };
        let _ = connection
            .send_packet(
                PlayerList {
                    list_type: PlayerList::TYPE_ADD,
                    entries: vec![PlayerListEntry {
                        uuid: other_conn_data.uuid,
                        entity_id: other_mc_id.0 as i64,
                        name: other_display.0.clone(),
                        xuid: other_conn_data.xuid.clone(),
                        platform_chat_id: String::new(),
                        build_platform: other_conn_data.device_os.index(),
                        skin: other_conn_data.skin.clone(),
                        ..Default::default()
                    }],
                },
                true,
            )
            .await;
        let _ = connection
            .send_packet(
                AddPlayer {
                    uuid: other_conn_data.uuid,
                    username: other_display.0.clone(),
                    entity_runtime_id: other_mc_id.0,
                    platform_chat_id: String::new(),
                    x: ox,
                    y: oy,
                    z: oz,
                    speed_x: 0.0,
                    speed_y: 0.0,
                    speed_z: 0.0,
                    pitch: 0.0,
                    yaw: 0.0,
                    head_yaw: 0.0,
                    game_type: 0,
                    player_permission: 0,
                    command_permission: 0,
                    device_id: String::new(),
                    build_platform: other_conn_data.device_os.index(),
                },
                true,
            )
            .await;
    }
}
