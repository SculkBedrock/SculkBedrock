use crate::game::gamemode::Gamemode;
use crate::game::structs::position::MinecraftPosition;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct MinecraftClientData {
    pub display_name: String,
    pub uuid: Uuid,
    pub play_before_time: i64,
    pub gamemode: Gamemode,
    pub achievements: Vec<String>,
    pub is_first_play: bool,
    pub first_played: i64,
    pub before_login_last_played: i64,
    pub protocol_version: u32,
    pub position: MinecraftPosition,
    pub is_op: bool,
}

impl Default for MinecraftClientData {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            uuid: Uuid::nil(),
            play_before_time: 0,
            gamemode: Gamemode::Survival,
            achievements: vec![],
            is_first_play: false,
            first_played: 0,
            before_login_last_played: 0,
            protocol_version: 0,
            position: MinecraftPosition::new(0.0, 0.0, 0.0),
            is_op: false,
        }
    }
}
