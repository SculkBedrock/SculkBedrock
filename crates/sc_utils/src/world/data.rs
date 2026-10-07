use crate::game::difficulty::Difficulty;
use crate::game::experiment::ExperimentFlags;
use crate::game::gamemode::Gamemode;
use crate::game::gamerules::GameRules;
use crate::nbt::deserialize_nbt_bool;
use crate::world::client_abilities::ClientAbilities;
use crate::world::policies::WorldPolicies;
use crate::world::r#type::WorldType;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

#[serde_inline_default]
#[derive(Deserialize, Debug, Clone)]
pub struct MinecraftWorldData {
    #[serde(skip_deserializing)]
    #[serde(default)]
    pub world_type: Option<WorldType>,
    #[serde_inline_default(Difficulty::Easy)]
    #[serde(alias = "Difficulty")]
    pub difficulty: Difficulty,
    #[serde_inline_default(false)]
    #[serde(alias = "ForceGameType")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub force_gamemode: bool,
    #[serde_inline_default(Gamemode::Survival)]
    #[serde(alias = "GameType")]
    pub gamemode: Gamemode,
    #[serde_inline_default("SculkBedrock Level".to_string())]
    #[serde(alias = "LevelName")]
    pub world_name: String,
    #[serde(alias = "SpawnX")]
    pub spawn_x: i32,
    #[serde(alias = "SpawnY")]
    pub spawn_y: i32,
    #[serde(alias = "SpawnZ")]
    pub spawn_z: i32,
    #[serde(alias = "Time")]
    pub time: i64,
    #[serde(alias = "GameRules")]
    #[serde(skip_deserializing)] // NBT reading not implemented yet.
    pub gamerules: GameRules,
    #[serde(alias = "LimitedWorldOriginX")]
    pub limited_world_origin_x: i32,
    #[serde(alias = "LimitedWorldOriginY")]
    pub limited_world_origin_y: i32,
    #[serde(alias = "LimitedWorldOriginZ")]
    pub limited_world_origin_z: i32,
    #[serde(alias = "MinimumCompatibleClientVersion")]
    #[serde_inline_default(vec![])]
    pub minimum_compatible_minecraft_version: Vec<i32>,
    #[serde(alias = "LANBroadcast")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub lan_broadcast: bool,
    #[serde(alias = "LANBroadcastIntent")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub lan_broadcast_intent: bool,
    #[serde(alias = "LastPlayed")]
    pub last_played: i64,
    #[serde(alias = "BiomeOverride")]
    pub biome_override: String,
    #[serde(alias = "CenterMapsToOrigin")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub center_maps_to_origin: bool,
    #[serde(alias = "ConfirmedPlatformLockedContent")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub confirmed_platform_locked_content: bool,
    #[serde(alias = "FlatWorldLayers")]
    #[serde_inline_default(String::new())]
    pub flat_world_layers: String,
    #[serde(alias = "Generator")]
    pub generator: i32,
    #[serde(alias = "InventoryVersion")]
    pub inventory_version: String,
    #[serde(alias = "MultiplayerGame")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub multiplayer_game: bool,
    #[serde(alias = "MultiplayerGameIntent")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub multiplayer_game_intent: bool,
    #[serde(alias = "NetherScale")]
    #[serde_inline_default(8)]
    pub nether_scale: i32,
    #[serde(alias = "NetworkVersion")]
    pub protocol_version: i32,
    #[serde(alias = "Platform")]
    pub platform: i32,
    #[serde(alias = "PlatformBroadcastIntent")]
    pub platform_broadcast_intent: i32,
    #[serde(alias = "RandomSeed")]
    pub world_seed: i64,
    #[serde(alias = "SpawnV1Villagers")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub spawn_v1_villagers: bool,
    #[serde(alias = "StorageVersion")]
    pub storage_version: i32,
    #[serde(alias = "WorldVersion")]
    pub world_version: i32,
    #[serde(alias = "XBLBroadcastIntent")]
    pub xbl_broadcast_intent: i32,
    #[serde(alias = "abilities")]
    #[serde_inline_default(ClientAbilities::default())]
    pub client_abilities: ClientAbilities,
    #[serde(alias = "baseGameVersion")]
    #[serde_inline_default(String::new())]
    pub base_game_version: String,
    #[serde(alias = "bonusChestEnabled")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub bonus_chest_enabled: bool,
    #[serde(alias = "bonusChestSpawned")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub bonus_chest_spawned: bool,
    #[serde(alias = "cheatsEnabled")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub cheats_enabled: bool,
    #[serde(alias = "commandsEnabled")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub commands_enabled: bool,
    #[serde(alias = "currentTick")]
    pub current_tick: i64,
    #[serde(alias = "daylightCycle")]
    pub daylight_cycle: i32,
    #[serde(alias = "editorWorldType")]
    pub editor_world_type: i32,
    #[serde(alias = "eduOffer")]
    pub edu_offer: i32,
    #[serde(alias = "educationFeaturesEnabled")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub edu_features_enabled: bool,
    #[serde(alias = "experiments")]
    #[serde_inline_default(ExperimentFlags::default())]
    pub experiments: ExperimentFlags,
    #[serde(alias = "hasBeenLoadedInCreative")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub has_been_loaded_in_creative: bool,
    #[serde(alias = "hasLockedBehaviorPack")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub has_locked_behavior_pack: bool,
    #[serde(alias = "hasLockedResourcePack")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub has_locked_resource_pack: bool,
    #[serde(alias = "immutableWorld")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub immutable_world: bool,
    #[serde(alias = "isCreatedInEditor")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub is_created_in_editor: bool,
    #[serde(alias = "isExportedFromEditor")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub is_exported_from_editor: bool,
    #[serde(alias = "isFromLockedTemplate")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub is_from_locked_template: bool,
    #[serde(alias = "isFromWorldTemplate")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub is_from_world_template: bool,
    #[serde(alias = "isRandomSeedAllowed")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub is_random_seed_allowed: bool,
    #[serde(alias = "isSingleUseWorld")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub is_single_use_world: bool,
    #[serde(alias = "isWorldTemplateOptionLocked")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub is_world_template_option_locked: bool,
    #[serde(alias = "lastOpenedWithVersion")]
    #[serde_inline_default(vec![])]
    pub last_opened_with_version: Vec<i32>,
    #[serde(alias = "lightningLevel")]
    pub lightning_level: f32,
    #[serde(alias = "lightningTime")]
    pub lightning_time: i32,
    #[serde(alias = "limitedWorldDepth")]
    pub limited_world_depth: i32,
    #[serde(alias = "limitedWorldWidth")]
    pub limited_world_width: i32,
    #[serde(alias = "permissionsLevel")]
    pub permissions_level: i32,
    #[serde(alias = "playerPermissionsLevel")]
    pub player_permissions_level: i32,
    #[serde(alias = "playerssleepingpercentage")]
    #[serde_inline_default(100)]
    pub player_sleeping_percentage: i32,
    #[serde(alias = "prid")]
    pub prid: String,
    #[serde(alias = "rainLevel")]
    pub rain_level: f32,
    #[serde(alias = "rainTime")]
    pub rain_time: i32,
    #[serde(alias = "randomtickspeed")]
    #[serde_inline_default(1)]
    pub random_tick_speed: i32,
    #[serde(alias = "recipesunlock")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub recipes_unlock: bool,
    #[serde(alias = "requiresCopiedPackRemovalCheck")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub requires_copied_pack_removal_check: bool,
    #[serde(alias = "serverChunkTickRange")]
    pub server_chunk_tick_range: i32,
    #[serde(alias = "spawnMobs")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub spawn_mobs: bool,
    #[serde(alias = "startWithMapEnabled")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub start_with_map_enabled: bool,
    #[serde(alias = "texturePacksRequired")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub texture_packs_required: bool,
    #[serde(alias = "useMsaGamerTagsOnly")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    #[serde_inline_default(false)]
    pub use_msa_gamer_tags_only: bool,
    #[serde(alias = "worldStartCount")]
    pub world_start_count: i64,
    #[serde(alias = "HasUncompleteWorldFileOnDisk")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    #[serde_inline_default(false)]
    pub has_uncomplete_world_file_on_disk: bool,
    #[serde(alias = "IsHardcore")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    #[serde_inline_default(false)]
    pub is_hardcore: bool,
    #[serde(alias = "PlayerHasDied")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    #[serde_inline_default(false)]
    pub player_has_died: bool,
    #[serde(alias = "playerwaypoints")]
    #[serde_inline_default(1)]
    pub player_waypoints: i32,
    #[serde(alias = "serverEditorConnectionPolicy")]
    #[serde_inline_default(0)]
    pub server_editor_connection_policy: i32,
    #[serde(alias = "allowAnonymousBlockDropsInEditorWorlds")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    #[serde_inline_default(false)]
    pub allow_anonymous_block_drops_in_editor_worlds: bool,
    #[serde(alias = "showrecipemessages")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    #[serde_inline_default(true)]
    pub show_recipe_messages: bool,
    #[serde(alias = "projectilescanbreakblocks")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    #[serde_inline_default(true)]
    pub projectiles_can_break_blocks: bool,
    #[serde(alias = "tntexplosiondropdecay")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    #[serde_inline_default(false)]
    pub tnt_explosion_drop_decay: bool,
    #[serde_inline_default(WorldPolicies::new())]
    pub world_policies: WorldPolicies,
}

impl Default for MinecraftWorldData {
    fn default() -> Self {
        Self {
            world_type: None,
            difficulty: Difficulty::Peaceful,
            force_gamemode: false,
            gamemode: Gamemode::Survival,
            world_name: "".to_string(),
            spawn_x: 0,
            spawn_y: 0,
            spawn_z: 0,
            time: 0,
            gamerules: Default::default(),
            limited_world_origin_x: 0,
            limited_world_origin_y: 0,
            limited_world_origin_z: 0,
            minimum_compatible_minecraft_version: vec![],
            lan_broadcast: false,
            lan_broadcast_intent: false,
            last_played: 0,
            biome_override: "".to_string(),
            center_maps_to_origin: false,
            confirmed_platform_locked_content: false,
            flat_world_layers: String::new(),
            generator: 0,
            inventory_version: "".to_string(),
            multiplayer_game: false,
            multiplayer_game_intent: false,
            nether_scale: 8,
            protocol_version: 0,
            platform: 0,
            platform_broadcast_intent: 0,
            world_seed: 0,
            spawn_v1_villagers: false,
            storage_version: 0,
            world_version: 0,
            xbl_broadcast_intent: 0,
            client_abilities: Default::default(),
            base_game_version: String::new(),
            bonus_chest_enabled: false,
            bonus_chest_spawned: false,
            cheats_enabled: false,
            commands_enabled: false,
            current_tick: 0,
            daylight_cycle: 0,
            editor_world_type: 0,
            edu_offer: 0,
            edu_features_enabled: false,
            experiments: ExperimentFlags::default(),
            has_been_loaded_in_creative: false,
            has_locked_behavior_pack: false,
            has_locked_resource_pack: false,
            immutable_world: false,
            is_created_in_editor: false,
            is_exported_from_editor: false,
            is_from_locked_template: false,
            is_from_world_template: false,
            is_random_seed_allowed: false,
            is_single_use_world: false,
            is_world_template_option_locked: false,
            last_opened_with_version: vec![],
            lightning_level: 0.0,
            lightning_time: 0,
            limited_world_depth: 0,
            limited_world_width: 0,
            permissions_level: 0,
            player_permissions_level: 0,
            player_sleeping_percentage: 100,
            prid: "".to_string(),
            rain_level: 0.0,
            rain_time: 0,
            random_tick_speed: 1,
            recipes_unlock: false,
            requires_copied_pack_removal_check: false,
            server_chunk_tick_range: 0,
            spawn_mobs: false,
            start_with_map_enabled: false,
            texture_packs_required: false,
            use_msa_gamer_tags_only: false,
            world_start_count: 0,
            has_uncomplete_world_file_on_disk: false,
            is_hardcore: false,
            player_has_died: false,
            player_waypoints: 1,
            server_editor_connection_policy: 0,
            allow_anonymous_block_drops_in_editor_worlds: false,
            show_recipe_messages: true,
            projectiles_can_break_blocks: true,
            tnt_explosion_drop_decay: false,
            world_policies: WorldPolicies::new(),
        }
    }
}

impl MinecraftWorldData {
    pub const WORLD_DATA_MAGIC_NUMBER: [u8; 8] = [10, 0, 0, 0, 68, 11, 0, 0];

    pub fn sanitized_spawn(&self) -> (i32, i32, i32) {
        let y = self.spawn_y.clamp(-64, 319);
        (self.spawn_x, y, self.spawn_z)
    }

    pub fn get_dimension(&self) -> i32 {
        if let Some(ty) = &self.world_type {
            ty.get_dimension()
        } else {
            1
        }
    }
}
