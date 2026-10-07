use crate::client::MinecraftClientNetwork;
use crate::events::player::{CreatePlayer, PlayerLogin, PlayerSpawn};
use crate::player_connection::{PlayerConnection, PlayerConnectionStatus};
use log::{debug, info};
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::entity::EntityId;
use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;
use sc_entity::motion::{PhysicsBody, Position, Rotation, Transform};
use sc_entity::player::adventure_settings::AdventureSettings;
use sc_entity::{MinecraftEntityId, SpawnEntity};
use sc_eventbus::param::SCEventReader;
use sc_eventbus::SCSendEvent;
use sc_game::movement::PlayerMovement;
use sc_item::PlayerInventory;
use sc_log::{t, t_log};
use sc_nbt::{NbtValue, SCNBTServer};
use sc_packloader::definitions::attribute::DefaultEntityAttributes;
use sc_packloader::version_control::runtime::MinecraftRuntimeManager;
use sc_utils::components::DisplayName;
use sc_utils::game::client::MinecraftClient;
use sc_utils::game::structs::position::MinecraftPosition;
use sc_utils::game::structs::server::Server;
use sc_utils::game::structs::server_properties::ServerProperties;
use sc_utils::material::Material;
use sc_utils::schedule::{SCConnectionUpdate, SCEventUpdate};
use sc_utils::world::r#type::WorldType;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use std::time::{SystemTime, UNIX_EPOCH};

/// All InGame players in the same world (excludes `exclude`).
pub(crate) fn same_world_players(
    world: &World,
    world_id: &MinecraftWorldId,
    exclude: Option<&EntityId>,
) -> Vec<EntityId> {
    let mut out = Vec::new();
    for other in world.entities_with_component::<PlayerConnection>() {
        if exclude.map(|e| e == &other).unwrap_or(false) {
            continue;
        }
        let Some(connection) = world.get_component::<PlayerConnection>(&other) else {
            continue;
        };
        if !matches!(
            connection.get_status(),
            PlayerConnectionStatus::InGame | PlayerConnectionStatus::Spawned,
        ) {
            continue;
        }
        let same_world = world
            .get_component::<MinecraftWorldId>(&other)
            .map(|id| id.as_ref() == world_id)
            .unwrap_or(false);
        if same_world {
            out.push(other);
        }
    }
    out
}
use crate::protocol::client::transaction::{write_instance_item, ItemData};
use crate::protocol::server::creative::{CreativeContent, CreativeItem, CreativeItemGroup};
use crate::protocol::server::entity::AvailableEntityIdentifiers;
use crate::protocol::server::item::{ItemComponent, ItemComponentEntry};
use crate::protocol::server::login::StartGame;
use crate::protocol::server::misc::{
    AvailableCommands, EntityMetadataEntry, GameRulesChanged, PlayerList, PlayerListEntry,
    SetCommandsEnabled, SetDifficulty, SetEntityData, SetSpawnPosition, SetTime,
};
use crate::protocol::server::modern::{
    CameraAimAssistPresets, CameraPresets, PlayerFog, TrimData, VoxelShapes,
};
use crate::protocol::server::world::{BiomeDefinition, BiomeDefinitionList};
use crate::utils::adventure_settings::SCNetworkAdventureSettings;
use crate::utils::ConnectionThreadManager;

pub struct SCPlayerHandlerPlugin;

/// Login-time constant packet cache (pre-serialized bytes shared across players).
///
/// ItemComponent / CreativeContent / BiomeDefinitionList /
/// AvailableEntityIdentifiers depend only on startup data, so they are
/// constant for all players: built and serialized once on the first login,
/// then reused as shared `Arc` bytes via
/// [`PlayerConnection::send_raw_packet`]. This avoids per-player rebuilds
/// (thousands of NBT entries) and duplicate multi-MB copies during
/// concurrent logins.
///
/// Call [`LoginPacketCache::clear`] if a runtime system ever rewrites these
/// sources (no such path exists currently).
#[derive(sc_ecs::resource::Resource, Default)]
pub struct LoginPacketCache {
    item_component: Option<std::sync::Arc<Vec<u8>>>,
    creative_content: Option<std::sync::Arc<Vec<u8>>>,
    biome_definition_list: Option<std::sync::Arc<Vec<u8>>>,
    entity_identifiers: Option<std::sync::Arc<Vec<u8>>>,
}

impl LoginPacketCache {
    fn get_or_build(
        slot: &mut Option<std::sync::Arc<Vec<u8>>>,
        build: impl FnOnce() -> Vec<u8>,
    ) -> std::sync::Arc<Vec<u8>> {
        if let Some(cached) = slot {
            return cached.clone();
        }
        let bytes = std::sync::Arc::new(build());
        *slot = Some(bytes.clone());
        bytes
    }

    pub fn item_component(&mut self, build: impl FnOnce() -> Vec<u8>) -> std::sync::Arc<Vec<u8>> {
        Self::get_or_build(&mut self.item_component, build)
    }

    pub fn creative_content(&mut self, build: impl FnOnce() -> Vec<u8>) -> std::sync::Arc<Vec<u8>> {
        Self::get_or_build(&mut self.creative_content, build)
    }

    pub fn biome_definition_list(
        &mut self,
        build: impl FnOnce() -> Vec<u8>,
    ) -> std::sync::Arc<Vec<u8>> {
        Self::get_or_build(&mut self.biome_definition_list, build)
    }

    pub fn entity_identifiers(
        &mut self,
        build: impl FnOnce() -> Vec<u8>,
    ) -> std::sync::Arc<Vec<u8>> {
        Self::get_or_build(&mut self.entity_identifiers, build)
    }

    /// Clear all cached packets (call when runtime data changes; no callers).
    pub fn clear(&mut self) {
        self.item_component = None;
        self.creative_content = None;
        self.biome_definition_list = None;
        self.entity_identifiers = None;
    }
}

/// Debug: per-packet delay for the login sequence (SC_LOGIN_SEQ_DELAY_MS in ms; 0 = no delay).
/// Independent from SC_SEND_DELAY_MS to avoid stacking two delays.
async fn login_seq_delay() {
    static DELAY_MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let ms = *DELAY_MS.get_or_init(|| {
        std::env::var("SC_LOGIN_SEQ_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    });
    if ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
}

/// Whether to send legacy login state packets (spawn/time/gamerule/player-list).
/// Disabled: that state now goes through StartGame and the first-spawn phase.
/// Keeping the group behind one switch prevents duplicate state packets while
/// retaining the typed implementation for older protocol adapters.
fn send_legacy_login_state_packets() -> bool {
    false
}

impl Plugin for SCPlayerHandlerPlugin {
    fn build(&self, app: &App) {
        app.insert_resource(LoginPacketCache::default())
            .add_systems(SCConnectionUpdate, create_player)
            .add_systems(SCEventUpdate, (player_spawn, player_login));
    }
}

/// Build CreativeContent (0x91) from the version-pack creative_items.json:
/// groups keep category/name/icon, each item group_id is the data file
/// group_index; items resolve names via the runtime table and unresolvable
/// items are skipped (creative_net_id stays dense). Returns an empty table
/// when data is missing.
fn build_creative_content(
    world: &World,
    runtime_manager: &MinecraftRuntimeManager,
) -> CreativeContent {
    use base64::Engine;
    let Some(registry) = world.get_resource::<sc_item::ItemRegistry>() else {
        return CreativeContent::empty();
    };
    let Some(data) = runtime_manager.creative_items() else {
        return CreativeContent::empty();
    };

    // Resolve a name to an item definition (runtime id via the runtime table).
    let resolve = |name: &str| -> Option<sc_item::ItemDefinition> {
        let runtime_id = runtime_manager.get_runtime_id(name)?;
        registry.get(runtime_id as u16)
    };
    // Item definition to network slot bytes (count=1, may carry damage/nbt_b64).
    let slot_bytes =
        |def: &sc_item::ItemDefinition, damage: u32, nbt_b64: Option<&str>| -> Option<Vec<u8>> {
            // nbt_b64 is bare network NBT (0x0a root...); wrap it as full ItemExtraData:
            // [lu16 0xFFFF has_nbt][u8 nbt_version=1][NBT][li32 can_place_on=0][li32 can_destroy=0],
            // then the instance/descriptor writer adds the varint length prefix.
            let user_data = nbt_b64
                .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
                .map(|nbt_bytes| {
                    let mut ud: Vec<u8> = Vec::with_capacity(nbt_bytes.len() + 11);
                    ud.extend_from_slice(&0xFFFFu16.to_le_bytes()); // has_nbt = true
                    ud.push(1); // nbt version
                    ud.extend_from_slice(&nbt_bytes); // Bare network NBT (no length prefix)
                    ud.extend_from_slice(&0i32.to_le_bytes()); // can_place_on
                    ud.extend_from_slice(&0i32.to_le_bytes()); // can_destroy
                    ud
                });
            let item_data = ItemData {
                runtime_id: def.runtime_id,
                count: 1,
                damage,
                has_net_id: false,
                net_id: 0,
                block_runtime_id: def.default_block_runtime_id.unwrap_or(0),
                user_data,
            };
            let mut writer = sc_binary::ByteWriter::new();
            write_instance_item(&mut writer, &item_data).ok()?;
            Some(writer.into())
        };

    // 1. Groups: keep all (item.group_index indexes the groups array directly).
    //    Fall back to the first resolvable item in the group for icons, then to an air slot.
    let air_slot = resolve("minecraft:air")
        .and_then(|def| slot_bytes(&def, 0, None))
        .unwrap_or_default();
    let mut groups: Vec<CreativeItemGroup> = Vec::with_capacity(data.groups.len());
    for group in &data.groups {
        let icon_slot = group
            .icon
            .as_ref()
            .and_then(|icon| resolve(&icon.id))
            .and_then(|def| slot_bytes(&def, 0, None))
            .or_else(|| {
                data.items
                    .iter()
                    .find(|entry| entry.group_index as usize == groups.len())
                    .and_then(|entry| resolve(&entry.id))
                    .and_then(|def| slot_bytes(&def, 0, None))
            })
            .unwrap_or_else(|| air_slot.clone());
        groups.push(CreativeItemGroup {
            category: group.creative_category,
            name: group.name.clone(),
            icon_data: icon_slot,
        });
    }

    // 2. Items: skip out-of-range group_index (bad data).
    let mut items: Vec<CreativeItem> = Vec::with_capacity(data.items.len());
    for entry in &data.items {
        let group_id = entry.group_index;
        if group_id < 0 || group_id as usize >= groups.len() {
            continue;
        }
        let Some(def) = resolve(&entry.id) else {
            continue;
        };
        let Some(slot) = slot_bytes(&def, entry.damage as u32, entry.nbt_b64.as_deref()) else {
            continue;
        };
        items.push(CreativeItem {
            creative_net_id: (items.len() + 1) as u32,
            slot_data: slot,
            group_id: group_id as u32,
        });
    }
    CreativeContent { groups, items }
}

/// Build Bedrock ItemRegistry/ItemComponent from version-pack semantic data.
///
/// One item definition per runtime entry; the component-based flag derives
/// from the actual component NBT payload. This keeps the packet data-driven
/// with no hardcoded packet bytes.
fn build_item_component(runtime_manager: &MinecraftRuntimeManager) -> ItemComponent {
    let entries = runtime_manager
        .runtime_entries()
        .into_iter()
        .map(|(name, runtime_id)| {
            let data = runtime_manager
                .get_item_spawner_by_ident(&name)
                .map(|item| item.to_network_component_tag())
                .unwrap_or_else(|| sc_nbt::compound::CompoundNbt::new(None));
            let is_component_based = !data.is_empty();

            ItemComponentEntry {
                version: runtime_manager.get_item_version(&name).unwrap_or(0),
                name,
                runtime_id: runtime_id as u16,
                is_component_based,
                data,
            }
        })
        .collect();

    let mut packet = ItemComponent::new();
    packet.set_entries(entries);
    packet
}

fn build_biome_definition_list(world: &World) -> BiomeDefinitionList {
    let Some(manager) =
        world.get_resource::<sc_packloader::definitions::biome::manager::MinecraftBiomeManager>()
    else {
        return BiomeDefinitionList::from_definitions(Vec::new());
    };
    let biome_nbt = manager.get_nbt().and_then(NbtValue::as_compound);

    use sc_packloader::definitions::biome::component::{Climate, Tags};

    let mut biomes = manager.iter().collect::<Vec<_>>();
    biomes.sort_by(|a, b| {
        let a_id = manager
            .biome_id(&a.description.identifier)
            .unwrap_or(i16::MAX);
        let b_id = manager
            .biome_id(&b.description.identifier)
            .unwrap_or(i16::MAX);
        a_id.cmp(&b_id)
            .then_with(|| a.description.identifier.cmp(&b.description.identifier))
    });

    let mut definitions = Vec::with_capacity(biomes.len());
    for biome in biomes {
        let mut temperature = 0.5f32;
        let mut downfall = 0.5f32;
        let mut foliage_snow = 0.0f32;
        let mut tags = Vec::new();

        if let Some(components) = biome.components.as_ref() {
            if let Some(climate) = components.get::<Climate>() {
                temperature = climate.temperature.unwrap_or(0.5);
                downfall = climate.downfall.unwrap_or(0.5);
                foliage_snow = climate
                    .snow_accumulation
                    .as_ref()
                    .and_then(|values| values.iter().copied().reduce(f32::max))
                    .unwrap_or(0.0);
            }
            if let Some(component_tags) = components.get::<Tags>() {
                tags.extend(component_tags.tags.iter().cloned());
            }
        }

        definitions.push(BiomeDefinition {
            name: biome.description.identifier.clone(),
            name_index: None,
            id: manager
                .biome_id(&biome.description.identifier)
                .unwrap_or(-1),
            temperature,
            downfall,
            foliage_snow,
            depth: 0.0,
            scale: 0.0,
            map_water_color: 0xff3f76e4u32 as i32,
            is_rain: downfall > 0.0,
            tag_indices: Vec::new(),
            tags,
            chunk_gen_data: biome_nbt
                .and_then(|all| all.get(&biome.description.identifier))
                .and_then(NbtValue::as_compound)
                .cloned(),
        });
    }

    BiomeDefinitionList::from_definitions(definitions)
}

fn create_player(world: World, mut event_reader: EventReader<CreatePlayer>) {
    for event in event_reader.read() {
        let entity = event.entity;
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            if let Some(connection) = world.get_component::<PlayerConnection>(&entity) {
                let Some(default_attributes) = world.get_resource::<DefaultEntityAttributes>()
                else {
                    log::error!(
                        "{}",
                        t_log!(
                            "console.player.create_fail",
                            entity = entity,
                            missing = "DefaultEntityAttributes is not registered"
                        )
                    );
                    return;
                };
                let attributes = default_attributes.default();
                let data = connection.get_data();
                let username = data.username.clone();
                let ip = connection.connection.address.ip().to_string();
                let locate = "Unknown";
                let os = data.device_os;
                let device = data.device_model.clone();
                let xuid = data.uuid;
                info!(
                    "{}",
                    t_log!(
                        "console.player.login",
                        player = username,
                        ip = ip,
                        ip_locate = locate,
                        os = os,
                        device = device,
                        xuid = xuid
                    )
                );
                //spawn client
                world.insert_entity_with(
                    &entity,
                    Material::PLAYER,
                    (
                        MinecraftClient::new(
                            entity,
                            world.clone(),
                            data.to_client_data(connection.get_protocol_version()),
                        ),
                        DisplayName(username),
                        attributes,
                        PlayerMovement::new(),
                        // Movement component (position is filled in once the first movement packet is adopted).
                        Transform::new(Position::new(0.0, 0.0, 0.0), Rotation::default()),
                        PhysicsBody::player(),
                        PlayerInventory::default(),
                        // Do not pre-insert `sc_game::ContainerOpen`: presence means
                        // "a window is open", and there is no window at login.
                        // Pre-inserting an empty value would permanently trip the
                        // "window already open" guard.
                    ),
                );
                world.send_sc_event(entity, PlayerSpawn);
            }
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}

fn player_spawn(world: World, mut event_reader: SCEventReader<PlayerSpawn>) {
    for event in event_reader.read() {
        let entity = event.client;
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            let Some(client) = world.get_component::<MinecraftClient>(&entity) else {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "spawn",
                        missing = "MinecraftClient is missing"
                    )
                );
                return;
            };
            let Some(server_properties) = world
                .get_resource::<ServerProperties>()
                .map(|properties| (*properties).clone())
            else {
                log::error!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "spawn",
                        missing = "ServerProperties is not registered"
                    )
                );
                let _ = client
                    .disconnect(&t!("console.login.invalid_data"), false)
                    .await;
                return;
            };

            let mut client_data = client.data.write();
            let client_xuid = client_data.uuid;

            let (is_online_player, is_first_play, nbt) = {
                let Some(server) = Server::global() else {
                    drop(client_data);
                    let _ = client
                        .disconnect(&t!("console.login.invalid_data"), false)
                        .await;
                    return;
                };
                let is_online_player = server.get_online_clients().iter().any(|(xuid, _)| {
                    world
                        .get_component::<DisplayName>(&entity)
                        .map(|client_name| {
                            client_name.0 == client_data.display_name && *xuid == client_xuid
                        })
                        .unwrap_or(false)
                });
                let is_first_play = server.get_client(client_xuid).is_none();
                let nbt = server.get_offline_player_nbt(client_xuid, server_properties.game_mode);
                (is_online_player, is_first_play, nbt)
            };

            if is_online_player {
                drop(client_data);
                let _ = client
                    .disconnect("disconnectionScreen.loggedinOtherLocation", false)
                    .await;
            } else {
                let default_gamemode = server_properties.game_mode;
                let force_gamemode = server_properties.force_gamemode;

                client_data.is_first_play = is_first_play;
                if let Some(mut nbt) = nbt {
                    // Update stored player data.
                    let last_played = nbt
                        .get("lastPlayed")
                        .and_then(NbtValue::as_i64)
                        .unwrap_or(0);
                    let first_played = nbt
                        .get("firstPlayed")
                        .and_then(NbtValue::as_i64)
                        .unwrap_or(0);
                    client_data.play_before_time = last_played - first_played;

                    nbt.insert(
                        "NameTag",
                        NbtValue::String(client_data.display_name.clone()),
                    );

                    let exp = nbt.get("EXP").and_then(NbtValue::as_i32).unwrap_or(0);
                    let exp_level = nbt.get("expLevel").and_then(NbtValue::as_i32).unwrap_or(0);
                    client.set_experience(exp, exp_level);

                    if force_gamemode {
                        client_data.gamemode = default_gamemode;
                        nbt.insert(
                            "playerGameType",
                            NbtValue::Int(client_data.gamemode.to_i32()),
                        );
                    }

                    // Resolve the spawn world.
                    let world_name = nbt
                        .get("Level")
                        .and_then(NbtValue::as_string)
                        .cloned()
                        .unwrap_or_default();
                    let world_info =
                        world
                            .get_resource::<MinecraftWorldManager>()
                            .and_then(|world_manager| {
                                let minecraft_world = world_manager
                                    .get_worlds_by_name(&world_name)
                                    .into_iter()
                                    .next()
                                    .or_else(|| {
                                        world_manager
                                            .get_worlds_by_type(&WorldType::Overworld)
                                            .into_iter()
                                            .next()
                                    });
                                minecraft_world.map(|minecraft_world| {
                                    (
                                        minecraft_world.world_id.clone(),
                                        minecraft_world.get_safe_spawn_position(),
                                    )
                                })
                            });

                    if let Some((world_id, spawn_position)) = world_info {
                        let pos;
                        match nbt.get("Pos").and_then(|v| v.as_list()) {
                            None => {
                                pos = spawn_position;
                                nbt.insert(
                                    "Pos",
                                    NbtValue::List(vec![
                                        NbtValue::Double(pos.x as f64),
                                        NbtValue::Double(pos.y as f64),
                                        NbtValue::Double(pos.z as f64),
                                    ]),
                                );
                            }
                            Some(list) => {
                                // Corrupt-save defense: treat non-Double elements / wrong
                                // length as invalid data and disconnect instead of panicking.
                                let p: Option<Vec<f32>> = list
                                    .iter()
                                    .map(|v| match v {
                                        NbtValue::Double(v) => Some(*v as f32),
                                        _ => None,
                                    })
                                    .collect();
                                let Some(p) = p.filter(|p| p.len() == 3) else {
                                    drop(client_data);
                                    let _ = client
                                        .disconnect(&t!("console.login.invalid_data"), false)
                                        .await;
                                    return;
                                };
                                pos = MinecraftPosition::new(p[0], p[1], p[2]);
                            }
                        }
                        client_data.position = pos;
                        if let Some(transform) = world.get_component::<Transform>(&entity) {
                            let mut transform = transform.write();
                            transform.position = Position::new(pos.x, pos.y, pos.z);
                            transform.broadcasted_position = transform.position;
                        }
                        world.add_component(&entity, world_id);
                    } else {
                        drop(client_data);
                        let _ = client
                            .disconnect(&t!("console.login.invalid_data"), false)
                            .await;
                        return;
                    }

                    // Missing/mistyped achievements and timestamps fall back to defaults.
                    if let Some(achievements) =
                        nbt.get("Achievements").and_then(|v| v.as_compound())
                    {
                        for (name, value) in achievements.iter() {
                            if let Some(value) = value.as_i16() {
                                if value > 0 {
                                    client_data.achievements.push(name.to_string())
                                }
                            }
                        }
                    }
                    client_data.first_played =
                        nbt.get("firstPlayed").and_then(|v| v.as_i64()).unwrap_or(0);
                    client_data.before_login_last_played =
                        nbt.get("lastPlayed").and_then(|v| v.as_i64()).unwrap_or(0);

                    let now_time = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|duration| duration.as_secs() as i64)
                        .unwrap_or(0);
                    nbt.insert("lastPlayed", NbtValue::Long(now_time));

                    let uuid = client_xuid.as_u64_pair();
                    nbt.insert("UUIDLeast", NbtValue::Long(uuid.1 as i64));
                    nbt.insert("UUIDMost", NbtValue::Long(uuid.0 as i64));

                    //auto save
                    debug!("Player({}) >> NBT Save", &client_data.display_name);

                    // AdventureSettings::new re-locks client.data, so release the
                    // write guard first to avoid same-task reentrant deadlock.
                    let display_name = client_data.display_name.clone();
                    drop(client_data);

                    // Create the AdventureSettings component (needed by the update below).
                    if let Some(adventure_settings) = AdventureSettings::new(entity, world.clone())
                    {
                        world.add_component(&entity, adventure_settings);
                        debug!(
                            "Player({}) >> AdventureSettings component created",
                            &display_name
                        );
                    }

                    world.send_sc_event(entity, PlayerLogin);
                } else {
                    drop(client_data);
                    let _ = client
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

fn player_login(world: World, mut receiver: SCEventReader<PlayerLogin>) {
    for event in receiver.read() {
        let entity = event.client;
        let world = world.clone();
        let manager_world = world.clone();
        let handle = SCECSAsync::runtime().spawn(async move {
            let Some(client) = world.get_component::<MinecraftClient>(&entity) else {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "login",
                        missing = "MinecraftClient is missing"
                    )
                );
                return;
            };
            let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "login",
                        missing = "PlayerConnection is missing"
                    )
                );
                return;
            };
            let Some(entity_id) = world
                .get_component::<MinecraftEntityId>(&entity)
                .map(|id| id.0)
            else {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "login",
                        missing = "MinecraftEntityId is missing"
                    )
                );
                let _ = client
                    .disconnect(&t!("console.login.invalid_data"), false)
                    .await;
                return;
            };
            let Some(world_id) = world
                .get_component::<MinecraftWorldId>(&entity)
                .map(|id| (*id).clone())
            else {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "login",
                        missing = "world id"
                    )
                );
                let _ = client
                    .disconnect(&t!("console.login.invalid_data"), false)
                    .await;
                return;
            };
            let Some(server_properties) = world
                .get_resource::<ServerProperties>()
                .map(|properties| (*properties).clone())
            else {
                log::error!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "login",
                        missing = "ServerProperties is missing"
                    )
                );
                let _ = client
                    .disconnect(&t!("console.login.invalid_data"), false)
                    .await;
                return;
            };
            let Some(runtime_manager) = world
                .get_resource::<MinecraftRuntimeManager>()
                .map(|manager| (*manager).clone())
            else {
                log::error!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "login",
                        missing = "MinecraftRuntimeManager is missing"
                    )
                );
                let _ = client
                    .disconnect(&t!("console.login.invalid_data"), false)
                    .await;
                return;
            };

            // Send the StartGame packet.
            let Some(world_data) =
                world
                    .get_resource::<MinecraftWorldManager>()
                    .and_then(|manager| {
                        manager
                            .get_world(&world_id)
                            .map(|minecraft_world| minecraft_world.world_data.clone())
                    })
            else {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.player.spawn_world_missing",
                        entity = entity,
                        world = world_id.world_id
                    )
                );
                let _ = client
                    .disconnect(&t!("console.login.invalid_data"), false)
                    .await;
                return;
            };
            let Some(item_palette) = runtime_manager.get_palette() else {
                log::error!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "login",
                        missing = "item palette"
                    )
                );
                let _ = client
                    .disconnect(&t!("console.login.invalid_data"), false)
                    .await;
                return;
            };
            let Some(entity_nbt) = runtime_manager.get_entity_network_nbt() else {
                log::error!(
                    "{}",
                    t_log!(
                        "console.player.skipped",
                        entity = entity,
                        phase = "login",
                        missing = "entity network NBT is missing"
                    )
                );
                let _ = client
                    .disconnect(&t!("console.login.invalid_data"), false)
                    .await;
                return;
            };

            let mut client_data = (*client.data.read()).clone();
            if !(-64.0..=319.0).contains(&client_data.position.y) {
                let (spawn_x, spawn_y, spawn_z) = world_data.sanitized_spawn();
                client_data.position =
                    MinecraftPosition::new(spawn_x as f32, spawn_y as f32, spawn_z as f32);
            }
            // Before-spawn send order.

            // 0. VoxelShapes (sent before StartGame; empty registry = 4 bytes).
            debug!("Player({}) >> Voxel Shapes", &client_data.display_name);
            login_seq_delay().await;
            let _ = connection.send_packet(VoxelShapes::empty(), true).await;

            // 1. StartGame

            let packet = StartGame {
                entity_id,
                client_data: client_data.clone(),
                world_data: world_data.clone(),
                server_properties: Some(server_properties.clone()),
                item_palette,
                gamerules: world_data.gamerules.clone(),
                world_editor: world_data.editor_world_type != 0,
                is_hardcore: world_data.is_hardcore,
                current_tick: world_data.current_tick,
                server_editor_connection_policy: world_data.server_editor_connection_policy,
                allow_anonymous_block_drops_in_editor_worlds: world_data
                    .allow_anonymous_block_drops_in_editor_worlds,
                // Chunk and block packets carry FNV1a block-state hashes rather
                // than version-pack runtime IDs.
                block_network_ids_hashed: true,
                enable_item_stack_net_manager: true, // ItemStackNetManager enabled
                is_sounds_server_authoritative: true, // Server-authoritative sounds
                ..Default::default()
            };
            debug!("Player({}) >> Start Game", &client_data.display_name);
            login_seq_delay().await;
            let _ = connection.send_packet(packet, true).await;

            // Once StartGame is sent, the client may send RequestChunkRadius
            // (and will not resend). Enter Initializing immediately, or early
            // radius requests are dropped and login stalls.
            connection.set_status(crate::player_connection::PlayerConnectionStatus::Initializing);

            // 2. ItemRegistry / ItemComponent (constant packet: shared bytes on cache hit).
            let item_component_bytes = world
                .get_resource_mut::<LoginPacketCache>()
                .map(|mut cache| {
                    cache.item_component(|| {
                        crate::packet::raw_batch::packet_to_bytes(build_item_component(
                            &runtime_manager,
                        ))
                        .unwrap_or_default()
                    })
                })
                .unwrap_or_default();
            debug!(
                "Player({}) >> Item Component bytes={}",
                &client_data.display_name,
                item_component_bytes.len()
            );
            login_seq_delay().await;
            let _ = connection.send_raw_packet(item_component_bytes, true).await;

            // 3. AvailableEntityIdentifiers (constant packet: shared bytes).
            let entity_identifiers_bytes = world
                .get_resource_mut::<LoginPacketCache>()
                .map(|mut cache| {
                    cache.entity_identifiers(|| {
                        crate::packet::raw_batch::packet_to_bytes(AvailableEntityIdentifiers {
                            nbt: entity_nbt.clone(),
                        })
                        .unwrap_or_default()
                    })
                })
                .unwrap_or_default();
            debug!(
                "Player({}) >> Available Entity Identifiers bytes={}",
                &client_data.display_name,
                entity_identifiers_bytes.len()
            );
            login_seq_delay().await;
            let _ = connection
                .send_raw_packet(entity_identifiers_bytes, true)
                .await;

            // SyncActorProperty is only sent when data-driven entity properties
            // exist. An empty compound crashes real 1.26.40 clients, and no
            // property registry exists here, so skip it.

            // 4. BiomeDefinitionList (constant packet: shared bytes). Built from
            // the version-pack biomes (climate temperature/downfall); an empty
            // table leaves clients unable to resolve chunk biome ids, so terrain
            // renders incorrectly.
            debug!(
                "Player({}) >> Biome Definition List",
                &client_data.display_name
            );
            let biome_definition_bytes = world
                .get_resource_mut::<LoginPacketCache>()
                .map(|mut cache| {
                    cache.biome_definition_list(|| {
                        crate::packet::raw_batch::packet_to_bytes(build_biome_definition_list(
                            &world,
                        ))
                        .unwrap_or_default()
                    })
                })
                .unwrap_or_default();
            login_seq_delay().await;
            let _ = connection
                .send_raw_packet(biome_definition_bytes, true)
                .await;

            // Sync attributes before AvailableCommands (before-spawn order).
            debug!("Player({}) >> Update Attributes", &client_data.display_name);
            login_seq_delay().await;
            if let Err(error) = client.sync_attributes().await {
                debug!(
                    "Player({}) >> UpdateAttributes aborted: {:?}",
                    &client_data.display_name, error
                );
                return;
            }

            // Send command metadata right after the biome registry, before
            // creative content and the first-spawn phase.
            debug!(
                "Player({}) >> Available Commands",
                &client_data.display_name
            );
            let commands = world
                .get_resource::<sc_command::registry::CommandRegistry>()
                .map(|registry| {
                    registry
                        .iter()
                        .map(|definition| crate::protocol::server::misc::CommandData {
                            name: definition.name.clone(),
                            description: definition.description.clone(),
                            flags: 0,
                            permission: definition.permission.network_id(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            login_seq_delay().await;
            let _ = connection
                .send_packet(AvailableCommands { commands }, true)
                .await;

            if send_legacy_login_state_packets() {
                // 5. SetSpawnPosition
                debug!(
                    "Player({}) >> Set Spawn Position",
                    &client_data.display_name
                );
                // Spawn at the player position (surface); the sanitized spawn
                // sentinel (SpawnY=32767) would clamp to the world top and spawn
                // the client in the air.
                let (spawn_x, spawn_y, spawn_z) = (
                    client_data.position.x.floor() as i32,
                    client_data.position.y.floor() as i32,
                    client_data.position.z.floor() as i32,
                );
                login_seq_delay().await;
                let _ = connection
                    .send_packet(
                        SetSpawnPosition {
                            spawn_type: SetSpawnPosition::TYPE_PLAYER_SPAWN,
                            x: spawn_x,
                            y: spawn_y,
                            z: spawn_z,
                            dimension: world_data.get_dimension(),
                            spawn_block_position: None,
                        },
                        true,
                    )
                    .await;

                // 6. SetTime
                debug!("Player({}) >> Set Time", &client_data.display_name);
                login_seq_delay().await;
                let _ = connection
                    .send_packet(
                        SetTime {
                            time: world_data.daylight_cycle,
                        },
                        true,
                    )
                    .await;

                // 7. SetDifficulty
                debug!("Player({}) >> Set Difficulty", &client_data.display_name);
                login_seq_delay().await;
                let _ = connection
                    .send_packet(
                        SetDifficulty {
                            difficulty: world_data.difficulty.to_i32() as u32,
                        },
                        true,
                    )
                    .await;

                // 8. SetCommandsEnabled
                debug!(
                    "Player({}) >> Set Commands Enabled",
                    &client_data.display_name
                );
                login_seq_delay().await;
                let _ = connection
                    .send_packet(
                        SetCommandsEnabled {
                            enabled: world_data.commands_enabled,
                        },
                        true,
                    )
                    .await;

                // 9. AdventureSettings (UpdateAbilities + UpdateAdventureSettings).
                debug!(
                    "Player({}) >> Adventure Settings (UpdateAbilities + UpdateAdventureSettings)",
                    &client_data.display_name
                );
                let Some(adventure_settings) = world.get_component::<AdventureSettings>(&entity)
                else {
                    log::warn!(
                        "{}",
                        t_log!(
                            "console.player.skipped",
                            entity = client_data.display_name,
                            phase = "abilities",
                            missing = "AdventureSettings"
                        )
                    );
                    return;
                };
                login_seq_delay().await;
                adventure_settings.update().await;

                // 10. GameRulesChanged
                debug!(
                    "Player({}) >> Game Rules Changed",
                    &client_data.display_name
                );
                login_seq_delay().await;
                let _ = connection
                    .send_packet(
                        GameRulesChanged {
                            gamerules: world_data.gamerules.clone(),
                        },
                        true,
                    )
                    .await;

                // 11. PlayerList (send the joining player as an ADD entry).
                debug!("Player({}) >> Player List", &client_data.display_name);
                let connection_data = connection.get_data();
                login_seq_delay().await;
                let _ = connection
                    .send_packet(
                        PlayerList {
                            list_type: PlayerList::TYPE_ADD,
                            entries: vec![crate::protocol::server::misc::PlayerListEntry {
                                uuid: connection_data.uuid,
                                entity_id: entity_id as i64,
                                name: client_data.display_name.clone(),
                                xuid: connection_data.xuid,
                                platform_chat_id: String::new(),
                                build_platform: connection_data.device_os.index(),
                                skin: connection_data.skin,
                                ..Default::default()
                            }],
                        },
                        true,
                    )
                    .await;
            }

            // 12. UpdateAttributes was moved before AvailableCommands (see above).

            // Sync inventory before CreativeContent (even when empty, matching
            // the second full sync in first spawn).
            debug!(
                "Player({}) >> Inventory sync (before creative)",
                &client_data.display_name
            );
            login_seq_delay().await;
            let _ = crate::handler::first_spawn::send_full_player_inventory(&world, entity).await;

            // 13. CreativeContent (constant packet: shared bytes) listing block
            // items from the item registry (empty when the registry is empty).
            debug!("Player({}) >> Creative Content", &client_data.display_name);
            let creative_bytes = world
                .get_resource_mut::<LoginPacketCache>()
                .map(|mut cache| {
                    cache.creative_content(|| {
                        crate::packet::raw_batch::packet_to_bytes(build_creative_content(
                            &world,
                            &runtime_manager,
                        ))
                        .unwrap_or_default()
                    })
                })
                .unwrap_or_default();
            let creative_bytes_len = creative_bytes.len();
            login_seq_delay().await;
            let _ = connection.send_raw_packet(creative_bytes, true).await;
            debug!(
                "Player({}) >> Creative Content sent ({creative_bytes_len} bytes, cached)",
                &client_data.display_name
            );

            // 16. SetEntityData (send entity metadata). Must carry default
            // metadata: DATA_AIR/MAX_AIR_SUPPLY=300 (full air),
            // DATA_FLAGS(0)=0; empty metadata leaves air_supply at 0 so the
            // client keeps showing the air bubbles.
            debug!("Player({}) >> Set Entity Data", &client_data.display_name);
            // Metadata built via EntityMetadataExt (set_flag/set_* helpers).
            use crate::protocol::server::entity_metadata::{
                EntityFlags as F, EntityKeys as K, EntityMetadataExt as _,
            };
            let mut metadata: Vec<EntityMetadataEntry> = Vec::new();
            metadata.set_flag(F::CAN_SHOW_NAMETAG, true);
            metadata.set_flag(F::CAN_CLIMB, true);
            metadata.set_flag(F::BREATHING, true); // Not in water: breathing on land
            metadata.set_flag(F::HAS_COLLISION, true);
            metadata.set_flag(F::GRAVITY, true);
            metadata.set_int(K::HEALTH, 20);
            metadata.set_byte(K::COLOR, 0);
            metadata.set_string(K::NAMETAG, client_data.display_name.clone());
            metadata.set_short(K::AIR, 300); // Current air (short)
            metadata.set_long(K::LEAD_HOLDER_EID, -1);
            metadata.set_float(K::SCALE, 1.0);
            metadata.set_short(K::MAX_AIR, 300); // Max air (short)
            metadata.set_float(K::BOUNDING_BOX_WIDTH, 0.6);
            metadata.set_float(K::BOUNDING_BOX_HEIGHT, 1.8);
            metadata.set_vector3i(K::PLAYER_BED_POSITION, 0, 0, 0);
            log::debug!(
                "player >> SetEntityData metadata={metadata:?} flags.breathing={}",
                metadata.get_flag(F::BREATHING)
            );
            login_seq_delay().await;
            let send_result = connection
                .send_packet(
                    SetEntityData {
                        entity_runtime_id: entity_id as u64,
                        metadata,
                        frame: 0,
                    },
                    true,
                )
                .await;
            if let Err(e) = send_result {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.player.set_entity_data_fail",
                        player = client_data.display_name,
                        error = e
                    )
                );
            }

            // Send modern client registries before the chunk request phase.
            // Empty registries use the same typed serializers and stay valid
            // for a vanilla world.
            debug!("Player({}) >> Trim Data", &client_data.display_name);
            login_seq_delay().await;
            let _ = connection.send_packet(TrimData::default(), true).await;

            debug!("Player({}) >> Player Fog", &client_data.display_name);
            login_seq_delay().await;
            let _ = connection.send_packet(PlayerFog::empty(), true).await;

            debug!("Player({}) >> Camera Presets", &client_data.display_name);
            login_seq_delay().await;
            let _ = connection.send_packet(CameraPresets::vanilla(), true).await;

            debug!(
                "Player({}) >> Camera Aim Assist Presets",
                &client_data.display_name
            );
            login_seq_delay().await;
            let _ = connection
                .send_packet(CameraAimAssistPresets::default(), true)
                .await;

            // Login tail: broadcast the joiner ADD entry to all connected
            // players, then send the full player list to the new player.
            // The player list is server-wide (not filtered by world; only
            // AddPlayer is same-world).
            login_seq_delay().await;
            broadcast_player_list_add(&world, entity, entity_id).await;

            debug!(
                "Player({}) >> Login sequence complete",
                &client_data.display_name
            );
        });
        if let Some(mut manager) = manager_world.get_resource_mut::<ConnectionThreadManager>() {
            manager.insert(entity, handle);
        }
    }
}

/// Player-list broadcast at the end of the login sequence (server-wide).
///
/// 1) Send the joiner ADD entry to all other past-login players.
/// 2) Send the full list (including the joiner) to the new player.
async fn broadcast_player_list_add(world: &World, entity: EntityId, _entity_id: u64) {
    fn entry_of(
        world: &World,
        other: EntityId,
        fallback_name: Option<&str>,
    ) -> Option<PlayerListEntry> {
        let connection = world.get_component::<PlayerConnection>(&other)?;
        let other_id = world.get_component::<MinecraftEntityId>(&other)?;
        let name = world
            .get_component::<DisplayName>(&other)
            .map(|name| name.0.clone())
            .or_else(|| fallback_name.map(str::to_string))?;
        let data = connection.get_data();
        Some(PlayerListEntry {
            uuid: data.uuid,
            entity_id: other_id.0 as i64,
            name,
            xuid: data.xuid,
            platform_chat_id: String::new(),
            build_platform: data.device_os.index(),
            skin: data.skin,
            ..Default::default()
        })
    }

    fn in_player_list(world: &World, other: EntityId) -> bool {
        world
            .get_component::<PlayerConnection>(&other)
            .map(|connection| {
                matches!(
                    connection.get_status(),
                    PlayerConnectionStatus::Initializing
                        | PlayerConnectionStatus::AwaitingClientInitialization
                        | PlayerConnectionStatus::InGame
                        | PlayerConnectionStatus::Spawned,
                )
            })
            .unwrap_or(false)
    }

    let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
        return;
    };
    let display_name = world
        .get_component::<MinecraftClient>(&entity)
        .map(|client| client.data.read().display_name.clone())
        .unwrap_or_default();
    let Some(joining) = entry_of(world, entity, Some(&display_name)) else {
        return;
    };

    // 1) Other players receive the joiner entry (server-wide).
    for other in world.entities_with_component::<PlayerConnection>() {
        if other == entity || !in_player_list(world, other) {
            continue;
        }
        let Some(other_connection) = world.get_component::<PlayerConnection>(&other) else {
            continue;
        };
        let _ = other_connection
            .send_packet(
                PlayerList {
                    list_type: PlayerList::TYPE_ADD,
                    entries: vec![joining.clone()],
                },
                true,
            )
            .await;
    }

    // 2) The new player receives the full list (including self).
    let mut full_list: Vec<PlayerListEntry> = Vec::new();
    for other in world.entities_with_component::<PlayerConnection>() {
        if !in_player_list(world, other) {
            continue;
        }
        if let Some(entry) = entry_of(world, other, None) {
            full_list.push(entry);
        }
    }
    if !full_list.iter().any(|entry| entry.uuid == joining.uuid) {
        full_list.push(joining);
    }
    let _ = connection
        .send_packet(
            PlayerList {
                list_type: PlayerList::TYPE_ADD,
                entries: full_list,
            },
            true,
        )
        .await;
}
