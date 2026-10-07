use serde::de::Unexpected;
use serde::Deserialize;
use std::u8;

#[derive(Debug, Clone)]
pub enum Difficulty {
    Peaceful,
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    pub fn from_i32(value: i32) -> Option<Difficulty> {
        match value {
            0 => Some(Difficulty::Peaceful),
            1 => Some(Difficulty::Easy),
            2 => Some(Difficulty::Normal),
            3 => Some(Difficulty::Hard),
            _ => None,
        }
    }

    pub fn to_i32(&self) -> i32 {
        match self {
            Difficulty::Peaceful => 0,
            Difficulty::Easy => 1,
            Difficulty::Normal => 2,
            Difficulty::Hard => 3,
        }
    }
}

impl<'de> Deserialize<'de> for Difficulty {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        match value {
            0 => Ok(Difficulty::Peaceful),
            1 => Ok(Difficulty::Easy),
            2 => Ok(Difficulty::Normal),
            3 => Ok(Difficulty::Hard),
            _ => Err(serde::de::Error::invalid_value(
                Unexpected::Unsigned(value as u64),
                &"Peaceful, Easy, Normal, Hard",
            )),
        }
    }
}
