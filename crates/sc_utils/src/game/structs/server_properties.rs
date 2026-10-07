use crate::game::gamemode::Gamemode;
use crate::game::structs::motd::Motd;
use rand::random;
use sc_ecs::resource::Resource;
use std::fs;
use std::path::Path;
use toml::{Table, Value};

#[derive(Clone, Debug, Resource)]
pub struct ServerProperties {
    pub guid: u64,

    // Server
    pub enable_snappy: bool,
    pub xbox_auth: bool,
    pub validate_encryption: bool,
    pub force_gamemode: bool,
    pub force_resource_packs: bool,
    pub enable_version_pack_plugins: bool,
    /// Console/log language id (`zh-CN` or `en-US`, `[server] language`).
    pub language: String,
    /// Daily-quote subtitle in the console (`[server] enable_hitokoto`, default off).
    pub enable_hitokoto: bool,

    // Log
    pub log_file_name_format: String,
    pub log_flush_interval_ms: u64,
    /// 日志级别（`[log] log_level`）：`info` 或 `debug`，改完重启生效。
    pub log_level: String,

    // Game
    pub server_name: String,
    pub server_motd: String,
    pub max_player: u32,
    pub game_mode: Gamemode,
    pub ipv4_port: u16,
    pub ipv6_port: u16,
    pub demo_systems: bool,

    // Experimental gameplay
    pub experimental_data_driven_items: bool,
    pub experimental_data_driven_biomes: bool,
    pub experimental_upcoming_creator_features: bool,
    pub experimental_gametest: bool,
    pub experimental_molang_features: bool,
    pub experimental_cameras: bool,

    // World
    pub overworld_name: String,
    pub the_nether_name: String,
    pub the_end_name: String,
    pub seed: String,
    pub generator: String,
    pub achievements_disable: bool,
    pub hide_seed: bool,

    // Chunk send pipeline
    pub view_distance: i32,
    pub chunks_per_tick: u32,
    pub spawn_threshold: u32,

    // Debug
    /// Outbound-blocked packet id list (decimal or TOML hex like 0x7a both accepted).
    /// send_batch filters by id before encoding, for bisecting real-client crashes without
    /// code changes. Empty by default.
    pub debug_blocked_packets: Vec<u16>,
}

fn required_section<'a>(toml: &'a Table, name: &str) -> Result<&'a Table, String> {
    toml.get(name)
        .and_then(Value::as_table)
        .ok_or_else(|| format!("missing or invalid section [{name}]"))
}

fn optional_section<'a>(toml: &'a Table, name: &str) -> Result<Option<&'a Table>, String> {
    match toml.get(name) {
        Some(value) => value
            .as_table()
            .map(Some)
            .ok_or_else(|| format!("invalid section [{name}]")),
        None => Ok(None),
    }
}

fn required_value<'a>(section: &'a Table, key: &str, path: &str) -> Result<&'a Value, String> {
    section
        .get(key)
        .ok_or_else(|| format!("missing required config key `{path}`"))
}

fn required_str(section: &Table, key: &str, path: &str) -> Result<String, String> {
    required_value(section, key, path)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("config key `{path}` must be a string"))
}

fn required_bool(section: &Table, key: &str, path: &str) -> Result<bool, String> {
    required_value(section, key, path)?
        .as_bool()
        .ok_or_else(|| format!("config key `{path}` must be a boolean"))
}

fn int_value(value: &Value, path: &str) -> Result<i64, String> {
    value
        .as_integer()
        .ok_or_else(|| format!("config key `{path}` must be an integer"))
}

fn range_error(path: &str, max: impl std::fmt::Display) -> String {
    format!("config key `{path}` must be in range 0..={max}")
}

fn required_u32(section: &Table, key: &str, path: &str) -> Result<u32, String> {
    let value = int_value(required_value(section, key, path)?, path)?;
    u32::try_from(value).map_err(|_| range_error(path, u32::MAX))
}

fn required_u16(section: &Table, key: &str, path: &str) -> Result<u16, String> {
    let value = int_value(required_value(section, key, path)?, path)?;
    u16::try_from(value).map_err(|_| range_error(path, u16::MAX))
}

fn required_port(section: &Table, key: &str, path: &str) -> Result<u16, String> {
    let value = required_u16(section, key, path)?;
    if value == 0 {
        return Err(format!(
            "config key `{path}` must be in range 1..={}",
            u16::MAX
        ));
    }
    Ok(value)
}

fn optional_bool(
    section: Option<&Table>,
    key: &str,
    path: &str,
    default: bool,
) -> Result<bool, String> {
    match section.and_then(|section| section.get(key)) {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("config key `{path}` must be a boolean")),
        None => Ok(default),
    }
}

fn optional_string(
    section: Option<&Table>,
    key: &str,
    path: &str,
    default: &str,
) -> Result<String, String> {
    match section.and_then(|section| section.get(key)) {
        Some(value) => value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("config key `{path}` must be a string")),
        None => Ok(default.to_string()),
    }
}

fn optional_u64(
    section: Option<&Table>,
    key: &str,
    path: &str,
    default: u64,
) -> Result<u64, String> {
    match section.and_then(|section| section.get(key)) {
        Some(value) => {
            let value = int_value(value, path)?;
            u64::try_from(value).map_err(|_| range_error(path, u64::MAX))
        }
        None => Ok(default),
    }
}

fn optional_u32(
    section: Option<&Table>,
    key: &str,
    path: &str,
    default: u32,
) -> Result<u32, String> {
    match section.and_then(|section| section.get(key)) {
        Some(value) => {
            let value = int_value(value, path)?;
            u32::try_from(value).map_err(|_| range_error(path, u32::MAX))
        }
        None => Ok(default),
    }
}

fn optional_i32_non_negative(
    section: Option<&Table>,
    key: &str,
    path: &str,
    default: i32,
) -> Result<i32, String> {
    match section.and_then(|section| section.get(key)) {
        Some(value) => {
            let value = int_value(value, path)?;
            if value < 0 || value > i64::from(i32::MAX) {
                Err(range_error(path, i32::MAX))
            } else {
                Ok(value as i32)
            }
        }
        None => Ok(default),
    }
}

impl ServerProperties {
    pub fn load<P: AsRef<Path>>(file_path: P) -> Result<Self, String> {
        let path = file_path.as_ref();
        let file_text = fs::read_to_string(path)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        let toml = file_text
            .parse::<Table>()
            .map_err(|err| format!("failed to parse {} as TOML: {err}", path.display()))?;

        Self::parse(toml)
    }

    fn parse(toml: Table) -> Result<Self, String> {
        let game = required_section(&toml, "game")?;
        let server_name = required_str(game, "name", "game.name")?;
        let server_motd = required_str(game, "motd", "game.motd")?;
        let max_player = required_u32(game, "max_player", "game.max_player")?;
        let game_mode_text = required_str(game, "gamemode", "game.gamemode")?;
        let game_mode = Gamemode::from_str(&game_mode_text).ok_or_else(|| {
            format!(
                "config key `game.gamemode` has invalid value `{game_mode_text}`; expected survival, creative, adventure, or spectator"
            )
        })?;
        let ipv4_port = required_port(game, "ipv4_port", "game.ipv4_port")?;
        let ipv6_port = required_port(game, "ipv6_port", "game.ipv6_port")?;
        let demo_systems = optional_bool(Some(game), "demo_systems", "game.demo_systems", false)?;

        let server = required_section(&toml, "server")?;
        let enable_snappy = required_bool(server, "enable_snappy", "server.enable_snappy")?;
        let xbox_auth = required_bool(server, "xbox_auth", "server.xbox_auth")?;
        let validate_encryption =
            required_bool(server, "validate_encryption", "server.validate_encryption")?;
        let force_gamemode = required_bool(server, "force_gamemode", "server.force_gamemode")?;
        let force_resource_packs = optional_bool(
            Some(server),
            "force_resource_packs",
            "server.force_resource_packs",
            false,
        )?;
        let enable_version_pack_plugins = optional_bool(
            Some(server),
            "enable_version_pack_plugins",
            "server.enable_version_pack_plugins",
            false,
        )?;
        let language = optional_string(Some(server), "language", "server.language", "zh-CN")?;
        if language != "zh-CN" && language != "en-US" {
            return Err("config key `server.language` must be \"zh-CN\" or \"en-US\"".to_string());
        };
        let enable_hitokoto = optional_bool(
            Some(server),
            "enable_hitokoto",
            "server.enable_hitokoto",
            false,
        )?;

        let log = optional_section(&toml, "log")?;
        let log_file_name_format = match log.and_then(|section| section.get("file_name_format")) {
            Some(value) => value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "config key `log.file_name_format` must be a string".to_string())?,
            None => "sc_log_%Y-%m-%d_%H-%M-%S.log".to_string(),
        };
        let log_flush_interval_ms =
            optional_u64(log, "flush_interval_ms", "log.flush_interval_ms", 200)?;
        // 可选（缺省 debug，保持此前硬编码行为）；只接受小写 info/debug，
        // 写错直接启动失败，比静默跑错级别好。
        let log_level = optional_string(log, "log_level", "log.log_level", "debug")?;
        if log_level != "info" && log_level != "debug" {
            return Err(
                "config key `log.log_level` must be \"info\" or \"debug\"".to_string(),
            );
        }

        let experimental_gameplay = required_section(&toml, "experimental_gameplay")?;
        let experimental_data_driven_items = required_bool(
            experimental_gameplay,
            "data_driven_items",
            "experimental_gameplay.data_driven_items",
        )?;
        let experimental_data_driven_biomes = required_bool(
            experimental_gameplay,
            "data_driven_biomes",
            "experimental_gameplay.data_driven_biomes",
        )?;
        let experimental_upcoming_creator_features = required_bool(
            experimental_gameplay,
            "upcoming_creator_features",
            "experimental_gameplay.upcoming_creator_features",
        )?;
        let experimental_gametest = required_bool(
            experimental_gameplay,
            "gametest",
            "experimental_gameplay.gametest",
        )?;
        let experimental_molang_features = required_bool(
            experimental_gameplay,
            "experimental_molang_features",
            "experimental_gameplay.experimental_molang_features",
        )?;
        let experimental_cameras = required_bool(
            experimental_gameplay,
            "cameras",
            "experimental_gameplay.cameras",
        )?;

        let world = required_section(&toml, "world")?;
        let overworld_name = required_str(world, "overworld_name", "world.overworld_name")?;
        let the_nether_name = required_str(world, "the_nether_name", "world.the_nether_name")?;
        let the_end_name = required_str(world, "the_end_name", "world.the_end_name")?;
        let seed = required_str(world, "seed", "world.seed")?;
        let generator = required_str(world, "generator", "world.generator")?;
        let achievements_disable =
            required_bool(world, "achievements_disable", "world.achievements_disable")?;
        let hide_seed = required_bool(world, "hide_seed", "world.hide_seed")?;

        let chunk = optional_section(&toml, "chunk")?;
        let view_distance =
            optional_i32_non_negative(chunk, "view_distance", "chunk.view_distance", 10)?;
        let chunks_per_tick = optional_u32(chunk, "chunks_per_tick", "chunk.chunks_per_tick", 4)?;
        let spawn_threshold = optional_u32(chunk, "spawn_threshold", "chunk.spawn_threshold", 56)?;

        let debug = optional_section(&toml, "debug")?;
        let debug_blocked_packets = match debug.and_then(|section| section.get("blocked_packets")) {
            Some(value) => value
                .as_array()
                .ok_or_else(|| {
                    "config key `debug.blocked_packets` must be an array of packet ids".to_string()
                })?
                .iter()
                .map(|value| {
                    let value = int_value(value, "debug.blocked_packets")?;
                    u16::try_from(value).map_err(|_| {
                        "config key `debug.blocked_packets` entries must be in range 0..=65535"
                            .to_string()
                    })
                })
                .collect::<Result<Vec<u16>, String>>()?,
            None => Vec::new(),
        };

        Ok(Self {
            enable_snappy,
            server_name,
            server_motd,
            max_player,
            game_mode,
            ipv4_port,
            ipv6_port,
            demo_systems,
            guid: random(),
            xbox_auth,
            validate_encryption,
            force_gamemode,
            force_resource_packs,
            enable_version_pack_plugins,
            language,
            enable_hitokoto,
            log_file_name_format,
            log_flush_interval_ms,
            log_level,
            experimental_data_driven_items,
            experimental_data_driven_biomes,
            experimental_upcoming_creator_features,
            experimental_gametest,
            experimental_molang_features,
            experimental_cameras,
            overworld_name,
            the_nether_name,
            the_end_name,
            seed,
            generator,
            achievements_disable,
            hide_seed,
            view_distance,
            chunks_per_tick,
            spawn_threshold,
            debug_blocked_packets,
        })
    }

    pub fn to_motd(&self, protocol: u32, version: String) -> Motd {
        Motd {
            motd: self.server_motd.clone(),
            protocol: u16::try_from(protocol).unwrap_or(u16::MAX),
            version,
            player_online: 0,
            player_max: self.max_player,
            gamemode: self.game_mode,
            server_guid: self.guid,
            level_name: self.overworld_name.clone(),
        }
    }
}
