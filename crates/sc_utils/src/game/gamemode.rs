use serde::de::{Error, Unexpected};
use serde::{Deserialize, Deserializer};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gamemode {
    Survival = 0,
    Creative,
    Adventure,
    Spectator,
}

impl Gamemode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Gamemode::Survival => "Survival",
            Gamemode::Creative => "Creative",
            Gamemode::Adventure => "Adventure",
            Gamemode::Spectator => "Spectator",
        }
    }

    pub fn from_str(s: &str) -> Option<Gamemode> {
        match s.to_ascii_lowercase().as_str() {
            "0" | "survival" | "s" => Some(Gamemode::Survival),
            "1" | "creative" | "c" => Some(Gamemode::Creative),
            "2" | "adventure" | "a" => Some(Gamemode::Adventure),
            "6" | "spectator" | "sp" => Some(Gamemode::Spectator),
            _ => None,
        }
    }

    pub fn from_u8(i: u8) -> Option<Gamemode> {
        match i {
            0 => Some(Gamemode::Survival),
            1 => Some(Gamemode::Creative),
            2 => Some(Gamemode::Adventure),
            6 => Some(Gamemode::Spectator),
            _ => None,
        }
    }

    pub fn to_i32(&self) -> i32 {
        match self {
            Gamemode::Survival => 0,
            Gamemode::Creative => 1,
            Gamemode::Adventure => 2,
            Gamemode::Spectator => 6,
        }
    }

    pub fn is_survival(&self) -> bool {
        match self {
            Gamemode::Survival => true,
            _ => false,
        }
    }

    pub fn is_creative(&self) -> bool {
        match self {
            Gamemode::Creative => true,
            _ => false,
        }
    }

    pub fn is_adventure(&self) -> bool {
        match self {
            Gamemode::Adventure => true,
            _ => false,
        }
    }

    pub fn is_spectator(&self) -> bool {
        match self {
            Gamemode::Spectator => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for Gamemode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = match *self {
            Gamemode::Survival => "0",
            Gamemode::Creative => "1",
            Gamemode::Adventure => "2",
            Gamemode::Spectator => "6",
        };
        write!(f, "{}", v)
    }
}

impl<'de> Deserialize<'de> for Gamemode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = u8::deserialize(deserializer)?;
        Self::from_u8(s).ok_or_else(|| {
            Error::invalid_value(Unexpected::Unsigned(s as u64), &"a valid gamemode")
        })
    }
}
