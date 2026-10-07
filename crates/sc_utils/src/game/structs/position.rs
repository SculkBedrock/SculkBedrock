use serde::de::{SeqAccess, Visitor};
use serde::Deserialize;
use std::fmt;

#[derive(Debug, Copy, Clone)]
pub struct MinecraftPosition {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl MinecraftPosition {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn new_i32(x: i32, y: i32, z: i32) -> Self {
        Self {
            x: x as f32,
            y: y as f32,
            z: z as f32,
        }
    }

    pub fn to_string(&self) -> String {
        format!("{} {} {}", self.x, self.y, self.z)
    }

    pub fn to_vec(&self) -> Vec<f32> {
        vec![self.x, self.y, self.z]
    }

    pub fn to_vec_i32(&self) -> Vec<i32> {
        vec![self.x as i32, self.y as i32, self.z as i32]
    }
}

impl<'de> Deserialize<'de> for MinecraftPosition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct MinecraftPositionVisitor;
        impl<'de> Visitor<'de> for MinecraftPositionVisitor {
            type Value = MinecraftPosition;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("[f32, f32, f32]")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let x = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &"[f32, f32, f32]"))?;
                let y = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &"[f32, f32, f32]"))?;
                let z = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(2, &"[f32, f32, f32]"))?;

                Ok(MinecraftPosition::new(x, y, z))
            }
        }
        deserializer.deserialize_any(MinecraftPositionVisitor {})
    }
}

#[cfg(test)]
mod tests {
    use super::MinecraftPosition;

    #[test]
    fn rejects_short_position_arrays_without_panicking() {
        let result = serde_json::from_str::<MinecraftPosition>("[1.0, 2.0]");
        assert!(result.is_err());
    }
}
