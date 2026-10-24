//! 世界数据读取：从 `level.dat` 解析 [`MinecraftWorldData`]。
//!
//! World data reading: parses [`MinecraftWorldData`] from `level.dat`.

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

/// 世界数据读取错误。
///
/// Errors produced while reading stored world data.
#[derive(Debug, ScError)]
pub enum WorldDataReaderError {
    /// 存储的世界数据未通过校验。
    ///
    /// Stored world data fails validation.
    #[sc_error(code = "world.reader.invalid_world_data")]
    InvalidWorldData {
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 底层 I/O 失败。
    ///
    /// Low-level I/O failure.
    #[sc_error(code = "world.reader.io")]
    IOError {
        /// 底层 I/O 错误。
        ///
        /// Underlying I/O error.
        source: std::io::Error,
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// JSON 桥接序列化失败。
    ///
    /// JSON bridge serialization failure.
    #[sc_error(code = "world.reader.serde_json")]
    SerdeJsonError {
        /// 底层 JSON 错误。
        ///
        /// Underlying JSON error.
        source: serde_json::Error,
        /// 说明原因的英文细节。
        ///
        /// English detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
}

impl WorldDataReaderError {
    /// 构造「世界数据无效」错误，并捕获调用位置。
    ///
    /// Build an invalid-data error, capturing the call site.
    #[track_caller]
    pub fn invalid_world_data() -> Self {
        Self::InvalidWorldData {
            detail: "invalid world data".to_string(),
            site: capture_site(),
        }
    }

    /// 构造 I/O 错误，并捕获调用位置。
    ///
    /// Build an I/O error, capturing the call site.
    ///
    /// - `source`: 底层 I/O 错误
    #[track_caller]
    pub fn io(source: std::io::Error) -> Self {
        let detail = source.to_string();
        Self::IOError {
            source,
            detail,
            site: capture_site(),
        }
    }

    /// 构造 JSON 桥接错误，并捕获调用位置。
    ///
    /// Build a JSON bridge error, capturing the call site.
    ///
    /// - `source`: 底层 JSON 错误
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
    /// 按错误码 + 细节 + 位置的格式格式化错误。
    ///
    /// Formats the error as code + detail + site.
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
    /// 返回底层错误源（I/O / JSON 变体），其余变体返回 `None`。
    ///
    /// Returns the underlying error source (I/O / JSON variants), or `None`
    /// for the rest.
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IOError { source, .. } => Some(source),
            Self::SerdeJsonError { source, .. } => Some(source),
            _ => None,
        }
    }
}


/// `level.dat` 读取器：文件字节 → NBT → [`MinecraftWorldData`]。
///
/// Reader for `level.dat`: file bytes to NBT to [`MinecraftWorldData`].
pub struct WorldDataReader;

impl WorldDataReader {
    /// 读取 `level.dat` 文件并解析为 [`MinecraftWorldData`]。
    ///
    /// Reads the `level.dat` file and parses it into [`MinecraftWorldData`].
    ///
    /// - `file_path`: `level.dat` 路径
    ///
    /// 错误：I/O 失败或 NBT 解析失败时返回 [`WorldDataReaderError`]。
    ///
    /// Errors: returns [`WorldDataReaderError`] on I/O or NBT parse failure.
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

    /// 从 `level.dat` 的 NBT 根值构造 [`MinecraftWorldData`]。
    ///
    /// Builds [`MinecraftWorldData`] from the NBT root value of `level.dat`.
    ///
    /// - `nbt`: `level.dat` 的根 NBT 值，必须是复合标签
    ///
    /// 错误：根值不是复合标签或 JSON 桥接失败时返回
    /// [`WorldDataReaderError`]。
    ///
    /// Errors: returns [`WorldDataReaderError`] when the root is not a
    /// compound or the JSON bridge fails.
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

    /// 从 world.dat 的扁平 NBT 字段解析游戏规则。
    /// 基岩版把每条游戏规则存为顶层字段（不是嵌套复合标签），
    /// 字段名全小写；Byte 映射 bool 规则，Int 映射 int 规则。
    ///
    /// Parses GameRules from the flat NBT fields of world.dat.
    /// The Bedrock edition stores every gamerule as a top-level field (not a
    /// nested compound) with an all-lowercase name. Byte maps to bool rules,
    /// Int maps to int rules.
    ///
    /// - `world_data`: 目标世界数据（原地写入 gamerules）
    /// - `compound`: `level.dat` 的根复合标签
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
