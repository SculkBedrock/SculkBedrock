use std::num::ParseIntError;
use std::str::FromStr;

#[derive(PartialEq, Eq, Ord, PartialOrd, Clone, Debug)]
pub struct MinecraftProtocolVersions {
    vec: Vec<u32>,
}

impl MinecraftProtocolVersions {
    pub fn new() -> Self {
        Self { vec: vec![] }
    }

    pub fn from_vec(vec: Vec<u32>) -> Self {
        let mut vec = vec;
        vec.sort();
        Self { vec }
    }

    pub fn single(version: u32) -> Self {
        Self::from_vec(vec![version])
    }

    pub fn from_str(version: &str) -> Result<Self, ParseIntError> {
        Ok(Self::single(u32::from_str(version)?))
    }

    pub fn push(&mut self, version: u32) {
        self.vec.push(version);
        self.vec.sort();
    }

    pub fn remove(&mut self, version: u32) {
        if let Some(i) = self.vec.iter().position(|x| *x == version) {
            self.vec.remove(i);
        }
    }

    pub fn get_min_version(&self) -> Option<u32> {
        self.vec.first().cloned()
    }

    pub fn get_max_version(&self) -> Option<u32> {
        self.vec.last().cloned()
    }
}
