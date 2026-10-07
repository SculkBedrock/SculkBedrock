pub(crate) mod chunk_encoder;
mod client;
pub mod events;
pub mod handler;
pub mod packet;
pub mod packet_hooks;
pub mod player_connection;
pub mod protocol;
pub mod utils;

use crate::chunk_encoder::ChunkEncodeExecutor;
use crate::events::connection::AcceptConnection;
use crate::handler::chunk_pipeline::ChunkPayloadSendBudget;
use crate::handler::connection::SCConnectionHandlerPlugin;
use crate::handler::crafting::SCCraftingHandlerPlugin;
use crate::handler::interaction::SCInteractionHandlerPlugin;
use crate::handler::network::SCNetworkHandlerPlugin;
use crate::handler::player::SCPlayerHandlerPlugin;
use crate::protocol::ProtocolInfo;
use crate::utils::compression_algorithm::CompressionAlgorithm;
use log::{info, warn};
use parking_lot::RwLock;
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::params::event::EventReader;
use sc_ecs::params::resource::Res;
use sc_ecs::resource::Resource;
use sc_ecs::schedule::Last;
use sc_ecs::world::World;
use sc_log::t_log;
use sc_packloader::version_control::SCVersionPack;
use sc_raknet::server::Listener;
use sc_utils::event::{SCExit, SCExitReason, SCExitType};
use sc_utils::game::structs::minecraft_version::{MinecraftVersion, MinecraftVersions};
use sc_utils::game::structs::motd::Motd;
use sc_utils::game::structs::protocol_versions::MinecraftProtocolVersions;
use sc_utils::game::structs::server_properties::ServerProperties;
use sc_utils::schedule::{SCLoad, SCPostLoad};
use std::sync::Arc;

#[derive(Resource)]
pub struct SCNetworkSettings {
    pub ipv4_port: u16,
    pub ipv6_port: u16,
    pub server_motd: Arc<RwLock<Motd>>,
    pub compression_algorithm: CompressionAlgorithm,
    /// Whether clients must accept resource packs (default false).
    pub force_resource_packs: bool,
}

pub struct SCNetworkPlugin;

impl Plugin for SCNetworkPlugin {
    fn build(&self, app: &App) {
        app.add_plugins(SCConnectionHandlerPlugin)
            .add_plugins(SCNetworkHandlerPlugin)
            .add_plugins(SCPlayerHandlerPlugin)
            .add_plugins(SCInteractionHandlerPlugin)
            .add_plugins(SCCraftingHandlerPlugin)
            // init_network needs SCVersionPack, inserted in SCPreLoad,
            // so this must run in SCLoad or later.
            .add_systems(SCLoad, (init_network, init_protocol))
            .add_systems(SCPostLoad, init_listener)
            .add_systems(Last, shutdown_chunk_encoder);
    }
}

fn init_network(
    world: World,
    server_properties: Res<ServerProperties>,
    version_pack: Res<SCVersionPack>,
) {
    let protocol_version = version_pack.manifest.protocol_version;
    let minecraft_version = version_pack.manifest.network_version().to_string();
    let encoder = match ChunkEncodeExecutor::new(
        ChunkEncodeExecutor::default_worker_count(),
        protocol_version,
    ) {
        Ok(encoder) => encoder,
        Err(error) => {
            log::error!(
                "{}",
                t_log!("console.network.encoder_workers_fail", error = error)
            );
            world.send_event(SCExit::new(
                SCExitReason::Error(Box::new(error)),
                SCExitType::Shutdown,
            ));
            return;
        }
    };
    let settings = SCNetworkSettings {
        ipv4_port: server_properties.ipv4_port,
        ipv6_port: server_properties.ipv6_port,
        server_motd: Arc::new(RwLock::new(
            server_properties.to_motd(protocol_version, minecraft_version),
        )),
        compression_algorithm: if !server_properties.enable_snappy {
            CompressionAlgorithm::Zlib
        } else {
            CompressionAlgorithm::Snappy
        },
        force_resource_packs: server_properties.force_resource_packs,
    };
    world.insert_resource(settings);
    world.insert_resource(encoder);
    world.insert_resource(ChunkPayloadSendBudget::default());
    // Outbound packet hook registry.
    world.insert_resource(packet_hooks::PacketSendHooks::default());
}

fn shutdown_chunk_encoder(world: World, mut reader: EventReader<SCExit>) {
    if reader.read().next().is_none() {
        return;
    }
    if let Some(encoder) = world
        .get_resource::<ChunkEncodeExecutor>()
        .map(|resource| (*resource).clone())
    {
        encoder.shutdown();
        info!("{}", t_log!("console.network.encoder_stopped"));
    }
}

pub(crate) fn init_protocol(version_pack: Res<SCVersionPack>) {
    let minecraft_network_version = version_pack.manifest.network_version().to_string();
    let minecraft_version = match MinecraftVersion::from_str(&minecraft_network_version) {
        Ok(version) => version,
        Err(error) => {
            log::error!(
                "{}",
                t_log!(
                    "console.network.protocol_info_fail",
                    version = minecraft_network_version,
                    error = error
                )
            );
            return;
        }
    };
    if let Err(existing) = ProtocolInfo::new_global(
        MinecraftProtocolVersions::from_vec(
            crate::protocol::version::SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        ),
        MinecraftVersions::single(minecraft_version),
        minecraft_network_version,
    ) {
        log::warn!(
            "{}",
            t_log!(
                "console.network.protocol_info_exists",
                protocol = existing.protocol_versions.get_max_version().unwrap_or(0)
            )
        );
    }
}

fn init_listener(world: World, settings: Res<SCNetworkSettings>) {
    let ipv4_port = settings.ipv4_port;
    let server_motd = settings.server_motd.clone();
    drop(settings);
    SCECSAsync::runtime().spawn(async move {
        info!("{}", t_log!("console.ipv4", port = ipv4_port));
        let listener = match Listener::bind(format!("0.0.0.0:{ipv4_port}"), server_motd).await {
            Ok(listener) => listener,
            Err(error) => {
                log::error!(
                    "{}",
                    t_log!(
                        "console.network.bind_ipv4_fail",
                        port = ipv4_port,
                        error = error
                    )
                );
                return;
            }
        };
        if let Err(error) = listener.start().await {
            log::error!(
                "{}",
                t_log!(
                    "console.network.listen_ipv4_fail",
                    port = ipv4_port,
                    error = error
                )
            );
            return;
        }
        world.insert_resource(listener.clone());

        loop {
            match listener.accept().await {
                Ok(connection) => {
                    world.send_event(AcceptConnection {
                        connection: Some(connection),
                    });
                }
                Err(error) => {
                    warn!(
                        "{}",
                        t_log!("console.network.accept_stopped", error = error)
                    );
                    break;
                }
            }
        }
    });
}
