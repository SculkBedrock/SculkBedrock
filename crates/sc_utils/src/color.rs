use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone)]
pub struct MinecraftColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl MinecraftColor {
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn new_without_alpha(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 0xFF }
    }

    pub fn from_rgba(rgba: u32) -> Self {
        Self {
            r: ((rgba >> 24) & 0xFF) as u8,
            g: ((rgba >> 16) & 0xFF) as u8,
            b: ((rgba >> 8) & 0xFF) as u8,
            a: (rgba & 0xFF) as u8,
        }
    }
}

impl<'de> Deserialize<'de> for MinecraftColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec = Vec::<u8>::deserialize(deserializer)?;
        if vec.len() < 3 {
            return Err(serde::de::Error::custom("Invalid color length"));
        }
        let a = vec.get(3).unwrap_or(&0xFF);
        Ok(Self {
            r: vec[0],
            g: vec[1],
            b: vec[2],
            a: *a,
        })
    }
}
