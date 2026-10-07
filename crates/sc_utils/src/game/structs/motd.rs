use crate::game::gamemode::Gamemode;

#[derive(Debug, Clone)]
pub struct Motd {
    /// The name of the server
    pub motd: String,
    /// The protocol version
    pub protocol: u16,
    /// The version of the server
    pub version: String,
    /// The number of players online
    pub player_online: u32,
    /// The maximum number of players
    pub player_max: u32,
    /// The server's GUID
    pub server_guid: u64,
    /// The level's name
    pub level_name: String,
    /// The gamemode of the server
    pub gamemode: Gamemode,
}

impl Motd {
    pub fn new(server_guid: u64) -> Self {
        Self {
            motd: "UndefinedRedstore Server".to_string(),
            protocol: 121,
            version: "1.0".into(),
            player_online: 0,
            player_max: 100,
            server_guid,
            level_name: "UndefinedRedstore Server".to_string(),
            gamemode: Gamemode::Survival,
        }
    }

    /// Takes the Motd and parses it into a valid MCPE
    /// MOTD buffer.
    pub fn write(&self) -> String {
        let props: Vec<String> = vec![
            "MCPE".into(),
            self.motd.clone(),
            self.protocol.to_string(),
            self.version.clone(),
            self.player_online.to_string(),
            self.player_max.to_string(),
            self.server_guid.to_string(),
            self.level_name.to_string(),
            self.gamemode.as_str().to_string(),
            "1".to_string(),
        ];
        props.join(";").into()
    }
}
