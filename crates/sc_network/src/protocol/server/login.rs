use crate::protocol::ProtocolInfo;
use log::debug;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{BinaryIo, ByteReader, ByteWriter};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::NbtValue;
use sc_nbt::SCNBTByteWriter;
use sc_network_macros::MinecraftPacket;
use sc_utils::game::experiment::ExperimentData;
use sc_utils::game::gamerules::GameRules;
use sc_utils::game::structs::server_properties::ServerProperties;
use sc_utils::world::client_data::MinecraftClientData;
use sc_utils::world::data::MinecraftWorldData;
use std::io::Error;
use std::sync::Arc;
use uuid::Uuid;

/// Player eye height. Bedrock player positions are eye level (feet + 1.62);
// the StartGame player position adds this offset on write.
const PLAYER_EYE_HEIGHT: f32 = 1.62;
const SERVER_AUTHORITATIVE_V3_MOVEMENT: i32 = 3;

#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct PlayStatus {
    pub status: u32,
}

impl PlayStatus {
    pub const LOGIN_SUCCESS: u32 = 0;
    pub const LOGIN_FAILED_CLIENT: u32 = 1;
    pub const LOGIN_FAILED_SERVER: u32 = 2;
    pub const PLAYER_SPAWN: u32 = 3;
    pub const LOGIN_FAILED_INVALID_TENANT: u32 = 4;
    pub const LOGIN_FAILED_VANILLA_EDU: u32 = 5;
    pub const LOGIN_FAILED_EDU_VANILLA: u32 = 6;
    pub const LOGIN_FAILED_SERVER_FULL: u32 = 7;
    pub const LOGIN_FAILED_EDITOR_TO_VANILLA_MISMATCH: u32 = 8;
    pub const LOGIN_FAILED_VANILLA_TO_EDITOR_MISMATCH: u32 = 9;

    pub fn get_string(status: u32) -> &'static str {
        match status {
            Self::LOGIN_SUCCESS => "LOGIN_SUCCESS",
            Self::LOGIN_FAILED_CLIENT => "LOGIN_FAILED_CLIENT",
            Self::LOGIN_FAILED_SERVER => "LOGIN_FAILED_SERVER",
            Self::PLAYER_SPAWN => "PLAYER_SPAWN",
            Self::LOGIN_FAILED_INVALID_TENANT => "LOGIN_FAILED_INVALID_TENANT",
            Self::LOGIN_FAILED_VANILLA_EDU => "LOGIN_FAILED_VANILLA_EDU",
            Self::LOGIN_FAILED_EDU_VANILLA => "LOGIN_FAILED_EDU_VANILLA",
            Self::LOGIN_FAILED_SERVER_FULL => "LOGIN_FAILED_SERVER_FULL",
            Self::LOGIN_FAILED_EDITOR_TO_VANILLA_MISMATCH => {
                "LOGIN_FAILED_EDITOR_TO_VANILLA_MISMATCH"
            }
            Self::LOGIN_FAILED_VANILLA_TO_EDITOR_MISMATCH => {
                "LOGIN_FAILED_VANILLA_TO_EDITOR_MISMATCH"
            }
            _ => "UNKNOWN",
        }
    }
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct StartGame {
    pub entity_id: u64,
    pub yaw: f32,
    pub pitch: f32,
    pub world_editor: bool,
    pub gamerules: GameRules,
    pub server_properties: Option<ServerProperties>,
    pub client_data: MinecraftClientData,
    pub world_data: MinecraftWorldData,
    pub item_palette: Arc<Vec<u8>>,
    pub disabling_personas: bool,
    pub disabling_custom_skins: bool,
    pub emote_chat_muted: bool,
    pub is_hardcore: bool,
    pub is_trial: bool,
    pub chat_restriction_level: i8,
    pub disable_player_interactions: bool,
    pub level_id: String,
    pub premium_world_template_id: String,
    pub server_authoritative_movement: Option<i32>,
    pub is_movement_server_authoritative: bool,
    pub current_tick: i64,
    pub enchantment_seed: i32,
    pub multiplayer_correlation_id: String,
    pub enable_item_stack_net_manager: bool,
    pub player_property_data: CompoundNbt,
    pub client_side_generation_enabled: bool,
    pub block_network_ids_hashed: bool,
    pub is_sounds_server_authoritative: bool,
    pub server_editor_connection_policy: i32,
    pub allow_anonymous_block_drops_in_editor_worlds: bool,
    pub logging_chat: bool,
}

impl StartGame {
    pub fn write_custom_blocks(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(0)
        //TODO: custom blocks (needs version-pack vs resource-pack split)
    }

    pub fn write_experiments(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        let mut experiments = vec![];
        let Some(server_properties) = self.server_properties.as_ref() else {
            return Err(Error::new(
                std::io::ErrorKind::InvalidData,
                "StartGame is missing server properties",
            ));
        };

        if server_properties.experimental_data_driven_items {
            experiments.push(ExperimentData::DATA_DRIVEN_ITEMS.set_enabled(true));
        }
        if server_properties.experimental_data_driven_biomes {
            experiments.push(ExperimentData::DATA_DRIVEN_BIOMES.set_enabled(true));
        }
        if server_properties.experimental_upcoming_creator_features {
            experiments.push(ExperimentData::UPCOMING_CREATOR_FEATURES.set_enabled(true));
        }
        if server_properties.experimental_gametest {
            experiments.push(ExperimentData::GAMETEST.set_enabled(true));
        }
        if server_properties.experimental_molang_features {
            experiments.push(ExperimentData::EXPERIMENTAL_MOLOANG_FEATURE.set_enabled(true));
        }
        if server_properties.experimental_cameras {
            experiments.push(ExperimentData::CAMERAS.set_enabled(true));
        }
        // Experiments array length is LInt. A VarInt here shifts every
        // following StartGame field and disconnects the client.
        buf.write_i32_le(experiments.len() as i32)?;
        for experiment in &experiments {
            experiment.write(buf)?;
        }
        buf.write_bool(experiments.len() != 0) // Were experiments previously toggled
    }
}

impl Writer for StartGame {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        let protocol_info = ProtocolInfo::global().ok_or_else(|| {
            Error::new(
                std::io::ErrorKind::InvalidData,
                "protocol info is not initialized",
            )
        })?;
        let server_properties = self.server_properties.as_ref().ok_or_else(|| {
            Error::new(
                std::io::ErrorKind::InvalidData,
                "StartGame is missing server properties",
            )
        })?;
        // vanillaVersion comes from the manifest network version declaration.
        let network_version = &protocol_info.minecraft_network_version;

        debug!(
            "StartGame >> BEGIN write | entity_id={} runtime_id={} gamemode={} network_version={}",
            self.entity_id,
            self.entity_id,
            self.client_data.gamemode.to_i32(),
            network_version
        );
        debug!(
            "StartGame >> pos=({},{},{}) yaw={} pitch={} seed_hidden={}",
            self.client_data.position.x,
            self.client_data.position.y,
            self.client_data.position.z,
            self.yaw,
            self.pitch,
            server_properties.hide_seed
        );

        // === Entity IDs ===
        buf.write_var_i64(self.entity_id as i64)?; // entityUniqueId (zigzag varlong)
        buf.write_var_u64(self.entity_id)?; // entityRuntimeId (unsigned varlong)
        buf.write_var_i32(self.client_data.gamemode.to_i32())?; // playerGamemode (zigzag varint)
        buf.write_f32_le(self.client_data.position.x)?; // pos.x (LFloat)
                                                        // Player positions use eye height (feet + 1.62):
                                                        // client_data.position stores feet, so add
                                                        // the eye offset on write.
        buf.write_f32_le(self.client_data.position.y + PLAYER_EYE_HEIGHT)?; // pos.y (LFloat)
        buf.write_f32_le(self.client_data.position.z)?; // pos.z (LFloat)
        buf.write_f32_le(self.yaw)?; // yaw (LFloat)
        buf.write_f32_le(self.pitch)?; // pitch (LFloat)

        // === Level settings start ===
        let seed = if server_properties.hide_seed {
            0
        } else {
            self.world_data.world_seed
        };
        // World spawn uses the player surface position, not sanitized_spawn:
        // a SpawnY=32767 sentinel clamps to the world top and spawns the
        // client in mid-air.
        let spawn = self.client_data.position;
        let spawn_x = spawn.x.floor() as i32;
        let spawn_y = spawn.y.floor() as i32;
        let spawn_z = spawn.z.floor() as i32;
        debug!("StartGame >> seed={} dim={} gen={} wgamemode={} hardcore={} difficulty={} spawn=({},{},{})",
            seed, self.world_data.get_dimension() & 0xff, self.world_data.generator,
            self.world_data.gamemode.to_i32(), self.is_hardcore, self.world_data.difficulty.to_i32(),
            spawn_x, spawn_y, spawn_z);

        buf.write_i64_le(seed)?; // seed (LLong)
        buf.write_i16_le(0)?; // SpawnBiomeType - Default (LShort)
        buf.write_string("plains")?; // UserDefinedBiomeName
        buf.write_var_i32(self.world_data.get_dimension() & 0xff)?; // dimension
        buf.write_var_i32(self.world_data.generator)?; // generator
        buf.write_var_i32(self.world_data.gamemode.to_i32())?; // worldGamemode
        buf.write_bool(self.is_hardcore)?; // hardcore
        buf.write_var_i32(self.world_data.difficulty.to_i32())?; // difficulty
                                                                 // spawn_position is BlockCoordinates (all zigzag varint32).
        buf.write_var_i32(spawn_x)?;
        buf.write_var_i32(spawn_y)?;
        buf.write_var_i32(spawn_z)?;
        buf.write_bool(server_properties.achievements_disable)?; // hasAchievementsDisabled
        buf.write_var_i32(if self.world_editor { 1 } else { 0 })?; // editorWorldType (always varint)
        buf.write_bool(self.world_data.is_created_in_editor)?;
        buf.write_bool(self.world_data.is_exported_from_editor)?;
        buf.write_var_i32(self.world_data.daylight_cycle)?; // dayCycleStopTime (-1 = not stopped)
        buf.write_var_u32(self.world_data.edu_offer as u32)?; // eduEditionOffer (putUnsignedVarInt)
        buf.write_bool(self.world_data.edu_features_enabled)?; // hasEduFeaturesEnabled
        buf.write_string("")?; // Education Edition Product ID
        buf.write_f32_le(self.world_data.rain_level)?; // rainLevel (LFloat)
        buf.write_f32_le(self.world_data.lightning_level)?; // lightningLevel (LFloat)
        buf.write_bool(self.world_data.confirmed_platform_locked_content)?; // hasConfirmedPlatformLockedContent
        buf.write_bool(self.world_data.multiplayer_game)?;
        buf.write_bool(self.world_data.lan_broadcast)?;
        buf.write_var_i32(self.world_data.xbl_broadcast_intent)?;
        buf.write_var_i32(self.world_data.platform_broadcast_intent)?;
        buf.write_bool(self.world_data.commands_enabled)?; // commandsEnabled
        buf.write_bool(self.world_data.texture_packs_required)?; // isTexturePacksRequired

        debug!("StartGame >> game_rules_count={}", self.gamerules.len());
        buf.write_game_rules(&self.gamerules)?; // gameRules (startGame=true, INTEGER uses zigzag varint)

        self.write_experiments(buf)?; // experiments

        buf.write_bool(self.world_data.bonus_chest_enabled)?; // bonusChest
        buf.write_bool(self.world_data.start_with_map_enabled)?; // hasStartWithMapEnabled
        buf.write_var_i32(1)?; // playerPermission (zigzag varint, Member)
        buf.write_i32_le(self.world_data.server_chunk_tick_range)?;
        buf.write_bool(self.world_data.has_locked_behavior_pack)?;
        buf.write_bool(self.world_data.has_locked_resource_pack)?;
        buf.write_bool(self.world_data.is_from_locked_template)?;
        buf.write_bool(self.world_data.use_msa_gamer_tags_only)?;
        buf.write_bool(self.world_data.is_from_world_template)?;
        buf.write_bool(self.world_data.is_world_template_option_locked)?;
        buf.write_bool(self.world_data.spawn_v1_villagers)?;
        buf.write_bool(self.disabling_personas)?; // isDisablingPersonas
        buf.write_bool(self.disabling_custom_skins)?; // isDisablingCustomSkins
        buf.write_bool(self.emote_chat_muted)?; // emoteChatMuted
                                                // LevelSettings.baseGameVersion sends "*".
        buf.write_string("*")?;
        buf.write_i32_le(16)?; // Limited world width (LInt)
        buf.write_i32_le(16)?; // Limited world height (LInt)
        buf.write_bool(false)?; // Nether type
                                // EduSharedUriResource
        buf.write_string("")?; // buttonName
        buf.write_string("")?; // linkUri
        buf.write_bool(false)?; // Experimental Gameplay
        buf.write_i8(self.chat_restriction_level)?; // chatRestrictionLevel (putByte)
        buf.write_bool(self.disable_player_interactions)?;
        buf.write_var_i32(self.server_editor_connection_policy)?; // ServerEditorConnectionPolicy
        buf.write_bool(self.allow_anonymous_block_drops_in_editor_worlds)?; // AllowAnonymousBlockDropsInEditorWorlds
                                                                            // === Level settings end ===

        debug!(
            "StartGame >> levelId={} worldName={} isTrial={}",
            self.level_id, self.world_data.world_name, self.is_trial
        );
        buf.write_string(self.level_id.as_str())?;
        buf.write_string(self.world_data.world_name.as_str())?;
        buf.write_string(self.premium_world_template_id.as_str())?;
        buf.write_bool(self.is_trial)?;
        // Movement settings: rewindHistorySize (varint) +
        // serverAuthoritativeBlockBreaking (bool). The movement mode field
        // is gone from the wire format (clients assume server-authoritative).
        buf.write_var_i32(0)?; // RewindHistorySize
        buf.write_bool(true)?; // isServerAuthoritativeBlockBreaking
        buf.write_i64_le(self.current_tick)?; // currentTick (LLong)
        buf.write_var_i32(self.enchantment_seed)?; // enchantmentSeed
        buf.write_var_u32(0)?; // No custom blocks (putUnsignedVarInt)
        buf.write_string(self.multiplayer_correlation_id.as_str())?;
        buf.write_bool(self.enable_item_stack_net_manager)?;
        buf.write_string("")?; // serverEngine (EMPTY STRING - NOT minecraft version!)

        debug!("StartGame >> writing playerPropertyData NBT (empty compound)");
        buf.write_nbt::<BedrockNetworkNbt>(&NbtValue::Compound(self.player_property_data.clone()))?; // playerPropertyData (raw NBT, no length prefix)
        buf.write_i64_le(0)?; // blockRegistryChecksum (LLong)
        buf.write_uuid(&Uuid::nil())?; // worldTemplateId (raw UUID, 16 bytes LE)
        buf.write_bool(self.client_side_generation_enabled)?;
        buf.write_bool(self.block_network_ids_hashed)?; // blockIdsAreHashed
        buf.write_bool(self.is_sounds_server_authoritative)?; // isServerAuthSounds (NetworkPermissions)
                                                              // LoggingChat exists only before 2168.
        let protocol_version = crate::protocol::version::current_protocol_version();
        if protocol_version < 2168 {
            buf.write_bool(self.logging_chat)?; // LoggingChat
        }
        buf.write_bool(false)?; // serverConfigurationJoinInfo (false = no join info)
                                // ServerTelemetryData (4 strings)
        buf.write_string("")?; // serverId
        buf.write_string("")?; // scenarioId
        buf.write_string("")?; // worldId
        buf.write_string("")?; // ownerId

        let total = buf.as_slice().len();
        debug!("StartGame >> END write, total bytes={}", total);
        Ok(())
    }
}

impl Reader<StartGame> for StartGame {
    fn read(_buf: &mut ByteReader) -> Result<StartGame, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "StartGame decode is unsupported",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::StartGame;
    use sc_binary::ByteWriter;
    use sc_utils::game::experiment::ExperimentData;
    use sc_utils::game::structs::server_properties::ServerProperties;

    #[test]
    fn experiments_use_little_endian_count() {
        let properties = ServerProperties {
            guid: 0,
            enable_snappy: false,
            xbox_auth: false,
            validate_encryption: false,
            force_gamemode: false,
            force_resource_packs: false,
            enable_version_pack_plugins: false,
            language: "zh-CN".to_string(),
            log_file_name_format: String::new(),
            log_flush_interval_ms: 200,
            server_name: String::new(),
            server_motd: String::new(),
            max_player: 0,
            game_mode: sc_utils::game::gamemode::Gamemode::Survival,
            ipv4_port: 0,
            ipv6_port: 0,
            demo_systems: false,
            experimental_data_driven_items: true,
            experimental_data_driven_biomes: false,
            experimental_upcoming_creator_features: false,
            experimental_gametest: false,
            experimental_molang_features: false,
            experimental_cameras: false,
            overworld_name: String::new(),
            the_nether_name: String::new(),
            the_end_name: String::new(),
            seed: String::new(),
            generator: String::new(),
            achievements_disable: false,
            hide_seed: false,
            view_distance: 10,
            chunks_per_tick: 4,
            spawn_threshold: 56,
            debug_blocked_packets: Vec::new(),
        };
        let packet = StartGame {
            server_properties: Some(properties.clone()),
            ..Default::default()
        };
        let mut writer = ByteWriter::new();
        packet.write_experiments(&mut writer).unwrap();

        assert_eq!(
            writer.as_slice(),
            &[
                1,
                0,
                0,
                0,
                ExperimentData::DATA_DRIVEN_ITEMS.name.len() as u8,
                b'd',
                b'a',
                b't',
                b'a',
                b'_',
                b'd',
                b'r',
                b'i',
                b'v',
                b'e',
                b'n',
                b'_',
                b'i',
                b't',
                b'e',
                b'm',
                b's',
                1,
                1,
            ]
        );
    }
}

impl Default for StartGame {
    fn default() -> Self {
        Self {
            entity_id: 0,
            yaw: 0.0,
            pitch: 0.0,
            world_editor: false,
            gamerules: Default::default(),
            server_properties: None,
            client_data: Default::default(),
            world_data: Default::default(),
            item_palette: Arc::new(Vec::new()),
            disabling_personas: false,
            disabling_custom_skins: false,
            emote_chat_muted: false,
            is_hardcore: false,
            is_trial: false,
            chat_restriction_level: 0,
            disable_player_interactions: false,
            level_id: "".to_string(),
            premium_world_template_id: "".to_string(),
            server_authoritative_movement: None,
            is_movement_server_authoritative: false,
            current_tick: 0,
            enchantment_seed: 0,
            multiplayer_correlation_id: "".to_string(),
            enable_item_stack_net_manager: true,
            player_property_data: CompoundNbt::new(None),
            client_side_generation_enabled: false,
            // v2168 advertises and serializes hashed
            // block-state IDs (FNV1a hashes), not version-pack runtime IDs.
            block_network_ids_hashed: true,
            is_sounds_server_authoritative: false,
            server_editor_connection_policy: 0,
            allow_anonymous_block_drops_in_editor_worlds: false,
            logging_chat: false,
        }
    }
}
