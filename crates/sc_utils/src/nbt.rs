use serde::Deserialize;

pub fn deserialize_nbt_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let b = i8::deserialize(deserializer)?;
    Ok(b != 0)
}
