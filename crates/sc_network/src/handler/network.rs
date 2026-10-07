use log::{debug, warn};
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;
use sc_log::{t, t_log};
use sc_raknet::utils::to_address_token;
use sc_utils::game::structs::server_properties::ServerProperties;
use sc_utils::schedule::SCConnectionUpdate;

use crate::player_connection::{PlayerConnection, PlayerConnectionData, PlayerConnectionStatus};
use crate::protocol::client::handshake::{ClientToServerHandshake, RequestNetworkSettings};
use crate::protocol::client::login::Login;
use crate::protocol::recv::MinecraftPacketReceiver;
use crate::protocol::server::handshake::{NetworkSettings, ServerToClientHandshake};
use crate::protocol::server::login::PlayStatus;
use crate::protocol::ProtocolInfo;
use crate::utils::encryption::MinecraftEncryption;
use crate::utils::ConnectionThreadManager;
use crate::SCNetworkSettings;

/// SCExit (server shutdown) -> sends a Disconnect packet to every online player and drops connections.
/// Runs in Last (before SCGamePlugin on_exit_shutdown_regions):
/// clients see "server closed" first, then region threads stop, logs flush, and exit runs.
fn broadcast_disconnect_on_exit(world: World, mut reader: EventReader<sc_utils::event::SCExit>) {
    if reader.read().next().is_none() {
        return;
    }
    let players = world.entities_with_component::<PlayerConnection>();
    if players.is_empty() {
        return;
    }
    log::info!(
        "{}",
        t_log!("console.server.shutdown_players", count = players.len())
    );
    let reason = t!("console.server.shutdown_reason").into_owned();
    let mut handles = Vec::new();
    for entity in players {
        let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
            continue;
        };
        let connection = connection.clone();
        let reason = reason.clone();
        handles.push(SCECSAsync::runtime().spawn(async move {
            let _ = connection.disconnect(&reason, false).await;
        }));
    }
    // Give connection threads a moment to emit Disconnect (immediate send + close).
    // exit(0) is triggered by SCGamePlugin on_exit_shutdown_regions after this system.
    std::thread::sleep(std::time::Duration::from_millis(250));
    for handle in handles {
        let _ = handle;
    }
}

pub struct SCNetworkHandlerPlugin;

impl Plugin for SCNetworkHandlerPlugin {
    fn build(&self, app: &App) {
        app.add_systems(
            SCConnectionUpdate,
            (
                request_network_settings,
                login,
                client_to_server_handshake,
                crate::handler::resource_pack::resource_pack_client_response,
                crate::handler::resource_pack::resource_pack_chunk_request,
                crate::handler::world::request_chunk_radius,
                crate::handler::world::serverbound_loading_screen,
                crate::handler::world::set_local_player_as_initialized,
                crate::handler::command::command_request,
                crate::handler::movement::player_auth_input,
            ),
        );
        // Player command feedback routes through SCEventUpdate: CommandFeedback emitted
        // by builtin/plugin command systems converts to CommandOutput packets in the same tick.
        app.add_systems(
            sc_utils::schedule::SCEventUpdate,
            (
                crate::handler::command::gamemode_command,
                crate::handler::command::command_feedback,
            ),
        );
        // Outbound intents -> protocol packet mapping (Last: after PostUpdate, once the game domain pushed this tick intents).
        // Game-logic systems (movement/physics/broadcast/blocks) live in sc_game, see SCGamePlugin.
        app.add_systems(
            sc_ecs::schedule::Last,
            crate::handler::outbox::outbox_to_packets,
        );
        // Graceful shutdown: SCExit (stop command/Ctrl+C/signal) -> Disconnect packets to all players.
        // Registered before SCGamePlugin on_exit_shutdown_regions (Last) -> broadcast first, then exit.
        app.add_systems(sc_ecs::schedule::Last, broadcast_disconnect_on_exit);
        // Fault-isolation consumer: game-registered terminal states disconnect here (budgeted per tick).
        app.add_systems(
            sc_utils::schedule::SCConnectionUpdate,
            crate::handler::faults::disconnect_faulted_connections,
        );
        // Chunk pipeline: subscription reorder + budgeted per-tick sending.
        app.insert_resource(crate::handler::chunk_pipeline::PipelineTick::default())
            .add_systems(
                sc_utils::schedule::SCChunkReorder,
                crate::handler::chunk_pipeline::order_chunks,
            )
            .add_systems(
                sc_utils::schedule::SCChunkSend,
                crate::handler::chunk_pipeline::send_next_chunk,
            );
    }
}

pub(crate) fn request_network_settings(
    world: World,
    mut event_reader: EventReader<MinecraftPacketReceiver<RequestNetworkSettings>>,
) {
    for event in event_reader.read() {
        let packet = event.packet.clone();
        let entity = event.entity;
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            let Some(connection) = world
                .get_component::<PlayerConnection>(&entity)
                .map(|connection| connection.clone())
            else {
                log::warn!(
                    "{}",
                    t_log!("console.login.ignored", packet = "RequestNetworkSettings")
                );
                return;
            };
            let Some(compression_algorithm) = world
                .get_resource::<SCNetworkSettings>()
                .map(|settings| settings.compression_algorithm)
            else {
                log::error!(
                    "{}",
                    t_log!(
                        "console.login.failed_missing",
                        packet = "RequestNetworkSettings",
                        what = "network settings"
                    )
                );
                let _ = connection
                    .disconnect(&t!("console.login.invalid_data"), false)
                    .await;
                return;
            };
            debug!(
                "[{}] RequestNetworkSettings Packet",
                to_address_token(connection.address())
            );

            // Compare protocol versions before negotiating compression,
            // rejecting mismatches with PlayStatus LOGIN_FAILED_* and a disconnect (LoginHandler checks again).
            let info = ProtocolInfo::global();
            let (min_version, max_version) = (
                info.as_ref()
                    .and_then(|info| info.protocol_versions.get_min_version())
                    .unwrap_or(0),
                info.as_ref()
                    .and_then(|info| info.protocol_versions.get_max_version())
                    .unwrap_or(u32::MAX),
            );
            let version = packet.protocol_version;
            let mut disconnect_reason = "";
            let status = if version < min_version {
                disconnect_reason = "disconnectionScreen.outdatedClient";
                PlayStatus::LOGIN_FAILED_CLIENT
            } else if version > max_version {
                disconnect_reason = "disconnectionScreen.outdatedServer";
                PlayStatus::LOGIN_FAILED_SERVER
            } else {
                PlayStatus::LOGIN_SUCCESS
            };
            if status != PlayStatus::LOGIN_SUCCESS {
                let _ = connection.send_packet(PlayStatus { status }, true).await;
                let _ = connection.disconnect(disconnect_reason, false).await;
                return;
            }

            connection.set_status(PlayerConnectionStatus::Logging);
            connection.set_protocol_version(version);
            let _ = connection
                .send_packet(
                    NetworkSettings {
                        compression_threshold: 1,
                        compression_algorithm,
                        client_throttle_enabled: false,
                        client_throttle_threshold: 0,
                        client_throttle_scalar: 0.0,
                    },
                    true,
                )
                .await;

            connection.enable_compression(compression_algorithm);
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}

pub(crate) fn login(world: World, mut event_reader: EventReader<MinecraftPacketReceiver<Login>>) {
    for event in event_reader.read() {
        let packet = event.packet.clone();
        let entity = event.entity;
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            let Some(connection) = world
                .get_component::<PlayerConnection>(&entity)
                .map(|connection| connection.clone())
            else {
                log::warn!("{}", t_log!("console.login.ignored", packet = "Login"));
                return;
            };
            if connection.get_status() == PlayerConnectionStatus::Logging {
                debug!("[{}] Login Packet", to_address_token(connection.address()));
                let auth_type = packet.auth_type;
                let signed = packet.signed;
                let Some(properties) = world
                    .get_resource::<ServerProperties>()
                    .map(|properties| (*properties).clone())
                else {
                    log::error!(
                        "{}",
                        t_log!(
                            "console.login.failed_missing",
                            packet = "Login",
                            what = "server properties"
                        )
                    );
                    let _ = connection
                        .disconnect(&t!("console.login.invalid_data"), false)
                        .await;
                    return;
                };
                if properties.xbox_auth {
                    if !auth_type.is_authenticated() || !signed {
                        debug!(
                            "Login Packet >> xbox auth failed >> auth_type: {:?}, signed: {}",
                            auth_type, signed
                        );
                        let _ = connection
                            .disconnect("disconnectionScreen.notAuthenticated", false)
                            .await;
                        return;
                    }
                }
                let temp_username = packet.username.as_str();
                if temp_username == "rcon" || temp_username == "console" {
                    let _ = connection
                        .disconnect("disconnectionScreen.invalidName", false)
                        .await;
                    return;
                }

                connection.set_data(PlayerConnectionData::from_login(&packet));
                let identity_public_key = packet.identity_public_key;
                connection.set_status(PlayerConnectionStatus::Handshaking);

                let Some(key_pair) = MinecraftEncryption::get_key_pair() else {
                    warn!(
                        "{}",
                        t_log!(
                            "console.login.keypair_fail",
                            addr = to_address_token(connection.address())
                        )
                    );
                    let _ = connection
                        .disconnect(&t!("console.login.invalid_data"), false)
                        .await;
                    return;
                };
                let token = MinecraftEncryption::create_token();
                let Some(client_key) = MinecraftEncryption::parse_key(identity_public_key.as_str())
                else {
                    warn!(
                        "{}",
                        t_log!(
                            "console.login.identity_key",
                            addr = to_address_token(connection.address())
                        )
                    );
                    let _ = connection
                        .disconnect("disconnectionScreen.notAuthenticated", false)
                        .await;
                    return;
                };
                let Some(secret_key) =
                    MinecraftEncryption::get_secret_key(&key_pair.0, &client_key, token)
                else {
                    warn!(
                        "{}",
                        t_log!(
                            "console.login.derive_key",
                            addr = to_address_token(connection.address())
                        )
                    );
                    let _ = connection
                        .disconnect(&t!("console.login.invalid_data"), false)
                        .await;
                    return;
                };
                let Some(jwt) = MinecraftEncryption::create_handshake_jwt(&key_pair, token) else {
                    warn!(
                        "{}",
                        t_log!(
                            "console.login.handshake",
                            addr = to_address_token(connection.address())
                        )
                    );
                    let _ = connection
                        .disconnect(&t!("console.login.invalid_data"), false)
                        .await;
                    return;
                };

                let _ = connection
                    .send_packet(ServerToClientHandshake { jwt }, true)
                    .await;
                if let Err(error) = connection.enable_encryption(secret_key) {
                    warn!(
                        "{}",
                        t_log!(
                            "console.login.enable_encryption",
                            addr = to_address_token(connection.address()),
                            error = error
                        )
                    );
                    let _ = connection
                        .disconnect(&t!("console.login.invalid_data"), false)
                        .await;
                }
            }
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}

pub(crate) fn client_to_server_handshake(
    world: World,
    mut event_reader: EventReader<MinecraftPacketReceiver<ClientToServerHandshake>>,
) {
    for event in event_reader.read() {
        let entity = event.entity;
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            let Some(connection) = world
                .get_component::<PlayerConnection>(&entity)
                .map(|connection| connection.clone())
            else {
                log::warn!(
                    "{}",
                    t_log!("console.login.ignored", packet = "ClientToServerHandshake")
                );
                return;
            };
            if connection.get_status() == PlayerConnectionStatus::Handshaking {
                debug!(
                    "[{}] ClientToServerHandshake Packet",
                    to_address_token(connection.address())
                );

                let Some(protocol_info) = ProtocolInfo::global() else {
                    log::error!(
                        "{}",
                        t_log!(
                            "console.login.failed_missing",
                            packet = "ClientToServerHandshake",
                            what = "protocol info"
                        )
                    );
                    let _ = connection
                        .disconnect(&t!("console.login.invalid_data"), false)
                        .await;
                    return;
                };
                let (Some(min_version), Some(max_version)) = (
                    protocol_info.protocol_versions.get_min_version(),
                    protocol_info.protocol_versions.get_max_version(),
                ) else {
                    log::error!("{}", t_log!("console.login.handshake_empty"));
                    let _ = connection
                        .disconnect(&t!("console.login.invalid_data"), false)
                        .await;
                    return;
                };
                let protocol_version = connection.get_protocol_version();
                let mut disconnect_reason = "";
                let status = if protocol_version < min_version {
                    disconnect_reason = "disconnectionScreen.outdatedClient";
                    PlayStatus::LOGIN_FAILED_CLIENT
                } else if protocol_version > max_version {
                    disconnect_reason = "disconnectionScreen.outdatedServer";
                    PlayStatus::LOGIN_FAILED_SERVER
                } else {
                    PlayStatus::LOGIN_SUCCESS
                };

                let _ = connection.send_packet(PlayStatus { status }, true).await;
                if status != PlayStatus::LOGIN_SUCCESS {
                    let _ = connection.disconnect(disconnect_reason, false).await;
                    return;
                }

                connection.set_status(PlayerConnectionStatus::ResourcePack);
                crate::handler::resource_pack::send_resource_pack_info(
                    &world,
                    connection.as_ref(),
                    protocol_version,
                )
                .await;
            }
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}
