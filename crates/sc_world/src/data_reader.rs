use log::debug;
use sc_binary::ByteReader;
use sc_error::{ScError, ScSite, capture_site};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::local::BedrockLocalNbt;
use sc_nbt::reader::NbtReader;
use sc_nbt::NbtValue;
use sc_utils::game::gamerules::{GameRule, GameRuleType, GameRuleValue};
use sc_utils::world::data::MinecraftWorldData;
use std::error::Error;
use std::fmt::Display;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, ScError)]
pub enum WorldDataReaderError {
    /// Stored world data fails validation.
    #[sc_error(code = "world.reader.invalid_world_data")]
    InvalidWorldData {
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Low-level I/O failure.
    #[sc_error(code = "world.reader.io")]
    IOError {
        /// Underlying I/O error.
        source: std::io::Error,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// JSON bridge serialization failure.
    #[sc_error(code = "world.reader.serde_json")]
    SerdeJsonError {
        /// Underlying JSON error.
        source: serde_json::Error,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
}

impl WorldDataReaderError {
    /// Build an invalid-data error, capturing the call site.
    #[track_caller]
    pub fn invalid_world_data() -> Self {
        Self::InvalidWorldData {
            detail: "invalid world data".to_string(),
            site: capture_site(),
        }
    }

    /// Build an I/O error, capturing the call site.
    #[track_caller]
    pub fn io(source: std::io::Error) -> Self {
        let detail = source.to_string();
        Self::IOError {
            source,
            detail,
            site: capture_site(),
        }
    }

    /// Build a JSON bridge error, capturing the call site.
    #[track_caller]
    pub fn serde_json(source: serde_json::Error) -> Self {
        let detail = source.to_string();
        Self::SerdeJsonError {
            source,
            detail,
            site: capture_site(),
        }
    }
}

impl Display for WorldDataReaderError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        match self {
            Self::InvalidWorldData { detail, site } => {
                write!(
                    fmt,
                    "[world.reader.invalid_world_data] {detail} (at {site})"
                )
            }
            Self::IOError { detail, site, .. } => {
                write!(fmt, "[world.reader.io] {detail} (at {site})")
            }
            Self::SerdeJsonError { detail, site, .. } => {
                write!(fmt, "[world.reader.serde_json] {detail} (at {site})")
            }
        }
    }
}

impl Error for WorldDataReaderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IOError { source, .. } => Some(source),
            Self::SerdeJsonError { source, .. } => Some(source),
            _ => None,
        }
    }
}


pub struct WorldDataReader;

impl WorldDataReader {
    pub fn new(file_path: PathBuf) -> Result<MinecraftWorldData, WorldDataReaderError> {
        let file_bytes = fs::read(file_path).map_err(WorldDataReaderError::io)?;
        let mut byte_reader = ByteReader::from(file_bytes);

        // Validates that the magic number is the level.dat magic number.
        let _ = byte_reader
            .read_bytes(8)
            .map_err(WorldDataReaderError::io)?;

        // Reads level.dat.
        let mut world_data = NbtReader::from_reader(&mut byte_reader);
        Self::from_nbt(
            world_data
                .read::<BedrockLocalNbt>()
                .map_err(WorldDataReaderError::io)?,
        )
    }

    pub fn from_nbt(nbt: NbtValue) -> Result<MinecraftWorldData, WorldDataReaderError> {
        let compound = nbt
            .as_compound()
            .ok_or_else(WorldDataReaderError::invalid_world_data)?;
        debug!("World Nbt: {:?}", compound);
        // Converts through JSON as an interim bridge.
        let json =
            serde_json::to_string(&compound).map_err(WorldDataReaderError::serde_json)?;
        let mut world_data: MinecraftWorldData = serde_json::from_str(json.as_str())
            .map_err(WorldDataReaderError::serde_json)?;

        // Parses GameRules from the flat NBT fields (gamerule field names in world.dat are all lowercase).
        Self::parse_gamerules(&mut world_data, compound);

        Ok(world_data)
    }

    /// Parses GameRules from the flat NBT fields of world.dat.
    /// The Bedrock edition stores every gamerule as a top-level field (not a
    /// nested compound) with an all-lowercase name. Byte maps to bool rules,
    /// Int maps to int rules.
    fn parse_gamerules(world_data: &mut MinecraftWorldData, compound: &CompoundNbt) {
        for rule in GameRule::values() {
            let key = rule.name().to_ascii_lowercase();
            if let Some(nbt_value) = compound.get(key.as_str()) {
                let game_value = match nbt_value {
                    NbtValue::Byte(b) => GameRuleValue::new(GameRuleType::Bool(*b != 0)),
                    NbtValue::Int(i) => GameRuleValue::new(GameRuleType::Int(*i)),
                    _ => continue,
                };
                world_data.gamerules.change(rule.clone(), game_value);
            }
        }
        debug!(
            "parsed {} gamerules from world NBT",
            world_data.gamerules.len()
        );
    }
}
