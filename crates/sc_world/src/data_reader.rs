use log::debug;
use sc_binary::ByteReader;
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

#[derive(Debug)]
pub enum WorldDataReaderError {
    InvalidWorldData,
    IOError(std::io::Error),
    SerdeJsonError(serde_json::Error),
}

impl Display for WorldDataReaderError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(fmt, "{:?}", self)
    }
}

impl Error for WorldDataReaderError {}

pub struct WorldDataReader;

impl WorldDataReader {
    pub fn new(file_path: PathBuf) -> Result<MinecraftWorldData, WorldDataReaderError> {
        let file_bytes = fs::read(file_path).map_err(|e| WorldDataReaderError::IOError(e))?;
        let mut byte_reader = ByteReader::from(file_bytes);

        // Validates that the magic number is the level.dat magic number.
        let _ = byte_reader
            .read_bytes(8)
            .map_err(|e| WorldDataReaderError::IOError(e))?;

        // Reads level.dat.
        let mut world_data = NbtReader::from_reader(&mut byte_reader);
        Self::from_nbt(
            world_data
                .read::<BedrockLocalNbt>()
                .map_err(|e| WorldDataReaderError::IOError(e))?,
        )
    }

    pub fn from_nbt(nbt: NbtValue) -> Result<MinecraftWorldData, WorldDataReaderError> {
        let compound = nbt
            .as_compound()
            .ok_or(WorldDataReaderError::InvalidWorldData)?;
        debug!("World Nbt: {:?}", compound);
        // Converts through JSON as an interim bridge.
        let json = serde_json::to_string(&compound)
            .map_err(|e| WorldDataReaderError::SerdeJsonError(e))?;
        let mut world_data: MinecraftWorldData = serde_json::from_str(json.as_str())
            .map_err(|e| WorldDataReaderError::SerdeJsonError(e))?;

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
