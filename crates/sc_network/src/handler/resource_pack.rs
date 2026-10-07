use log::debug;
use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;
use sc_log::t_log;
use sc_packloader::pack::pack_manager::ResourcePackManager;
use sc_raknet::utils::to_address_token;
use uuid::Uuid;

use crate::events::player::CreatePlayer;
use crate::player_connection::{PlayerConnection, PlayerConnectionStatus};
use crate::protocol::client::resource_packs::{
    ResourcePackChunkRequest, ResourcePackClientResponse,
};
use crate::protocol::recv::MinecraftPacketReceiver;
use crate::protocol::server::resource_packs::{
    ResourcePackChunkData, ResourcePackDataInfo, ResourcePackInfo, ResourcePackStack,
};
use crate::utils::ConnectionThreadManager;

pub(crate) const RESOURCE_PACK_CHUNK_SIZE: usize = 8192;

/// Whether the resource-pack phase forces acceptance (default false).
fn force_resource_packs(world: &World) -> bool {
    world
        .get_resource::<crate::SCNetworkSettings>()
        .map(|settings| settings.force_resource_packs)
        .unwrap_or(false)
}

pub(crate) async fn send_resource_pack_info(
    world: &World,
    connection: &PlayerConnection,
    protocol_version: u32,
) {
    let behavior_packs = world
        .get_resource::<ResourcePackManager>()
        .map(|manager| manager.get_behavior_packs())
        .unwrap_or_default();
    let resource_packs = world
        .get_resource::<ResourcePackManager>()
        .map(|manager| manager.get_resource_packs())
        .unwrap_or_default();
    // must_accept comes from server config (default false);
    // world template id/version is UUID(0,0)/"0.0.0".
    let _ = connection
        .send_packet(
            ResourcePackInfo {
                protocol_version,
                must_accept: force_resource_packs(world),
                scripting: false,
                force_disable_vibrant_visuals: false,
                world_template_id: Uuid::nil(),
                world_template_version: "0.0.0".to_string(),
                has_addon_packs: false,
                force_server_packs: false,
                behavior_packs,
                resource_packs,
            },
            true,
        )
        .await;
}

pub(crate) fn resource_pack_client_response(
    world: World,
    mut event_reader: EventReader<MinecraftPacketReceiver<ResourcePackClientResponse>>,
) {
    for event in event_reader.read() {
        let packet = event.packet.clone();
        let entity = event.entity;
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            if let Some(connection) = world.get_component::<PlayerConnection>(&entity) {
                if connection.get_status() != PlayerConnectionStatus::ResourcePack {
                    return;
                }
                let protocol_version = connection.get_protocol_version();
                match packet.response_status {
                    ResourcePackClientResponse::STATUS_REFUSED => {
                        let _ = connection
                            .disconnect("disconnectionScreen.noReason", false)
                            .await;
                    }
                    ResourcePackClientResponse::STATUS_SEND_PACKS => {
                        let pack_manager = world.get_resource::<ResourcePackManager>();
                        for entry in &packet.entries {
                            let uuid = match Uuid::parse_str(&entry.uuid) {
                                Ok(uuid) => uuid,
                                Err(_) => {
                                    let _ = connection
                                        .disconnect("disconnectionScreen.resourcePack", false)
                                        .await;
                                    return;
                                }
                            };
                            let resource_pack = match pack_manager.as_ref().and_then(|pm| {
                                pm.packs
                                    .iter()
                                    .find(|p| p.manifest.information.uuid == uuid)
                            }) {
                                Some(pack) => pack,
                                None => {
                                    let _ = connection
                                        .disconnect("disconnectionScreen.resourcePack", false)
                                        .await;
                                    return;
                                }
                            };
                            let info = &resource_pack.manifest.information;
                            let pack_size = resource_pack.pack_data.len() as u64;
                            let chunk_count = (pack_size + RESOURCE_PACK_CHUNK_SIZE as u64 - 1)
                                / RESOURCE_PACK_CHUNK_SIZE as u64;
                            let _ = connection
                                .send_packet(
                                    ResourcePackDataInfo {
                                        pack_id: info.uuid,
                                        pack_version: info.version.to_string(),
                                        max_chunk_size: RESOURCE_PACK_CHUNK_SIZE as u32,
                                        chunk_count: chunk_count as u32,
                                        compressed_pack_size: pack_size,
                                        sha256: info.sha256.clone(),
                                        is_premium: false,
                                        pack_type: 6,
                                    },
                                    true,
                                )
                                .await;
                        }
                    }
                    ResourcePackClientResponse::STATUS_HAVE_ALL_PACKS => {
                        let behavior_pack_stack = world
                            .get_resource::<ResourcePackManager>()
                            .map(|pm| pm.get_behavior_packs())
                            .unwrap_or_default();
                        let resource_pack_stack = world
                            .get_resource::<ResourcePackManager>()
                            .map(|pm| pm.get_resource_packs())
                            .unwrap_or_default();
                        // Experiments come from server config (empty plus
                        // previouslyToggled=false when none are enabled);
                        // baseGameVersion is always "*".
                        let experiments = Vec::new();
                        let _ = connection
                            .send_packet(
                                ResourcePackStack {
                                    protocol_version,
                                    must_accept: force_resource_packs(&world),
                                    behavior_pack_stack,
                                    resource_pack_stack,
                                    experiments,
                                    game_version: "*".to_string(),
                                    is_has_editor_packs: false,
                                },
                                true,
                            )
                            .await;
                    }
                    ResourcePackClientResponse::STATUS_COMPLETED => {
                        connection.clear_chunk_requests();
                        connection.set_status(PlayerConnectionStatus::PreSpawn);
                        world.send_event(CreatePlayer { entity });
                    }
                    _ => {}
                }
            }
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}

pub(crate) fn resource_pack_chunk_request(
    world: World,
    mut event_reader: EventReader<MinecraftPacketReceiver<ResourcePackChunkRequest>>,
) {
    for event in event_reader.read() {
        let packet = event.packet.clone();
        let entity = event.entity;
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            if let Some(connection) = world.get_component::<PlayerConnection>(&entity) {
                let sem = connection.resource_pack_chunk_semaphore.clone();
                let Ok(_permit) = sem.acquire().await else {
                    log::warn!("{}", t_log!("console.resourcepack.worker_stopped"));
                    return;
                };
                if connection.get_status() != PlayerConnectionStatus::ResourcePack {
                    return;
                }
                let pack_manager = world.get_resource::<ResourcePackManager>();
                let resource_pack = match pack_manager.as_ref().and_then(|pm| {
                    pm.packs
                        .iter()
                        .find(|p| p.manifest.information.uuid == packet.pack_id)
                }) {
                    Some(pack) => pack,
                    None => {
                        let _ = connection
                            .disconnect("disconnectionScreen.resourcePack", false)
                            .await;
                        return;
                    }
                };
                let offset = RESOURCE_PACK_CHUNK_SIZE * packet.chunk_index as usize;
                let chunk_data = resource_pack.get_pack_chunk(offset, RESOURCE_PACK_CHUNK_SIZE);
                let _ = connection
                    .send_packet(
                        ResourcePackChunkData {
                            pack_id: resource_pack.manifest.information.uuid,
                            pack_version: resource_pack.manifest.information.version.to_string(),
                            chunk_index: packet.chunk_index,
                            progress: offset as u64,
                            data: chunk_data,
                        },
                        true,
                    )
                    .await;
                debug!(
                    "[{}] resource pack chunk {} sent",
                    to_address_token(connection.address()),
                    packet.chunk_index
                );
            }
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}
