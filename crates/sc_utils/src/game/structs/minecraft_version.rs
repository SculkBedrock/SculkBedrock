use serde::Deserialize;
use std::fmt::{Debug, Display, Formatter};
use std::num::ParseIntError;
use std::str::FromStr;

#[derive(Debug)]
pub enum MinecraftVersionError {
    UnknownVersionString(String),
    UnknownVersionVec(Vec<u8>),
    ParseIntError(ParseIntError),
}

impl Display for MinecraftVersionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for MinecraftVersionError {}

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
            return Err(MinecraftVersionError::UnknownVersionString(
                version.to_string(),
            ));
        }
        if split.len() == 2 {
            Ok(Self {
                v0: 1,
                v1: u8::from_str(split[0].trim())
                    .map_err(|e| MinecraftVersionError::ParseIntError(e))?,
                v2: u8::from_str(split[1].trim())
                    .map_err(|e| MinecraftVersionError::ParseIntError(e))?,
            })
        } else {
            Ok(Self {
                v0: u8::from_str(split[0].trim())
                    .map_err(|e| MinecraftVersionError::ParseIntError(e))?,
                v1: u8::from_str(split[1].trim())
                    .map_err(|e| MinecraftVersionError::ParseIntError(e))?,
                v2: u8::from_str(split[2].trim())
                    .map_err(|e| MinecraftVersionError::ParseIntError(e))?,
            })
        }
    }

    pub fn from_vec(vec: Vec<u8>) -> Result<MinecraftVersion, MinecraftVersionError> {
        if vec.len() != 3 && vec.len() != 2 {
            return Err(MinecraftVersionError::UnknownVersionVec(vec));
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
