use sc_error::{ScError, ScSite, capture_site};
use serde::Deserialize;
use std::fmt::{Debug, Display, Formatter};
use std::num::ParseIntError;
use std::str::FromStr;

#[derive(Debug, ScError)]
pub enum MinecraftVersionError {
    /// Unrecognized version string.
    #[sc_error(code = "utils.version.unknown_version_string")]
    UnknownVersionString {
        /// Rejected version string.
        value: String,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Unrecognized version byte vector.
    #[sc_error(code = "utils.version.unknown_version_vec")]
    UnknownVersionVec {
        /// Rejected version bytes.
        value: Vec<u8>,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Version component is not a number.
    #[sc_error(code = "utils.version.parse_int")]
    ParseIntError {
        /// Underlying parse error.
        source: ParseIntError,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
}

impl MinecraftVersionError {
    /// 构造未知版本字符串错误并捕获调用位置。
    ///
    /// Build an unknown-string error, capturing the call site.
    #[track_caller]
    pub fn unknown_version_string(value: String) -> Self {
        let detail = format!("unknown version string '{value}'");
        Self::UnknownVersionString {
            value,
            detail,
            site: capture_site(),
        }
    }

    /// 构造未知版本字节错误并捕获调用位置。
    ///
    /// Build an unknown-bytes error, capturing the call site.
    #[track_caller]
    pub fn unknown_version_vec(value: Vec<u8>) -> Self {
        let detail = format!(
            "unknown version with {} parts, expected 2 or 3",
            value.len()
        );
        Self::UnknownVersionVec {
            value,
            detail,
            site: capture_site(),
        }
    }

    /// 构造整数解析错误并捕获调用位置。
    ///
    /// Build a component-parse error, capturing the call site.
    #[track_caller]
    pub fn parse_int(source: ParseIntError) -> Self {
        let detail = source.to_string();
        Self::ParseIntError {
            source,
            detail,
            site: capture_site(),
        }
    }
}

impl Display for MinecraftVersionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownVersionString { detail, site, .. } => {
                write!(
                    f,
                    "[utils.version.unknown_version_string] {detail} (at {site})"
                )
            }
            Self::UnknownVersionVec { detail, site, .. } => {
                write!(
                    f,
                    "[utils.version.unknown_version_vec] {detail} (at {site})"
                )
            }
            Self::ParseIntError { detail, site, .. } => {
                write!(f, "[utils.version.parse_int] {detail} (at {site})")
            }
        }
    }
}

impl std::error::Error for MinecraftVersionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ParseIntError { source, .. } => Some(source),
            _ => None,
        }
    }
}


impl From<ParseIntError> for MinecraftVersionError {
    #[track_caller]
    fn from(error: ParseIntError) -> Self {
        Self::parse_int(error)
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MinecraftVersionEnum {
    Vec(Vec<u8>),
    String(String),
}

impl MinecraftVersionEnum {
    pub fn to_version(self) -> Result<MinecraftVersion, MinecraftVersionError> {
        match self {
            MinecraftVersionEnum::Vec(vec) => MinecraftVersion::from_vec(vec),
            MinecraftVersionEnum::String(str) => MinecraftVersion::from_str(str.as_str()),
        }
    }
}

#[derive(PartialEq, Eq, Ord, PartialOrd, Clone, Copy)]
pub struct MinecraftVersion {
    v0: u8,
    v1: u8,
    v2: u8,
}

impl MinecraftVersion {
    pub fn new(v0: u8, v1: u8, v2: u8) -> Self {
        Self { v0, v1, v2 }
    }

    pub fn from_str(version: &str) -> Result<MinecraftVersion, MinecraftVersionError> {
        let split: Vec<&str> = version.split(".").collect();
        if split.len() != 3 && split.len() != 2 {
            return Err(MinecraftVersionError::unknown_version_string(
                version.to_string(),
            ));
        }
        if split.len() == 2 {
            Ok(Self {
                v0: 1,
                v1: u8::from_str(split[0].trim())?,
                v2: u8::from_str(split[1].trim())?,
            })
        } else {
            Ok(Self {
                v0: u8::from_str(split[0].trim())?,
                v1: u8::from_str(split[1].trim())?,
                v2: u8::from_str(split[2].trim())?,
            })
        }
    }

    pub fn from_vec(vec: Vec<u8>) -> Result<MinecraftVersion, MinecraftVersionError> {
        if vec.len() != 3 && vec.len() != 2 {
            return Err(MinecraftVersionError::unknown_version_vec(vec));
        }
        if vec.len() == 2 {
            Ok(Self {
                v0: 1,
                v1: vec[0],
                v2: vec[1],
            })
        } else {
            Ok(Self {
                v0: vec[0],
                v1: vec[1],
                v2: vec[2],
            })
        }
    }

    pub fn to_string(&self) -> String {
        format!("{}.{}.{}", self.v0, self.v1, self.v2)
    }
}

impl Debug for MinecraftVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "MinecraftVersion({})", self.to_string())
    }
}

impl Display for MinecraftVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct MinecraftVersions {
    vec: Vec<MinecraftVersion>,
}

impl MinecraftVersions {
    pub fn new() -> Self {
        Self { vec: vec![] }
    }

    pub fn from_vec(vec: Vec<MinecraftVersion>) -> Self {
        let mut vec = vec;
        vec.sort();
        Self { vec }
    }

    pub fn single(version: MinecraftVersion) -> Self {
        Self::from_vec(vec![version])
    }

    pub fn from_str(version: &str) -> Result<Self, MinecraftVersionError> {
        Ok(Self::single(MinecraftVersion::from_str(version)?))
    }

    pub fn push(&mut self, version: MinecraftVersion) {
        self.vec.push(version);
        self.vec.sort();
    }

    pub fn remove(&mut self, version: MinecraftVersion) {
        if let Some(i) = self.vec.iter().position(|x| *x == version) {
            self.vec.remove(i);
        }
    }

    pub fn get_min_version(&self) -> Option<MinecraftVersion> {
        self.vec.first().cloned()
    }

    pub fn get_max_version(&self) -> Option<MinecraftVersion> {
        self.vec.last().cloned()
    }
}
