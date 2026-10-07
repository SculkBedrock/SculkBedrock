use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone)]
pub enum NoiseType {
    Default,
    DefaultMutated,
    River,
    Ocean,
    DeepOcean,
    Lowlands,
    Taiga,
    Mountains,
    Highlands,
    Extreme,
    LessExtreme,
    Beach,
    StoneBeach,
    Mushroom,
    Swamp,
}

impl<'de> Deserialize<'de> for NoiseType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ty = String::deserialize(deserializer)?;
        Ok(match ty.as_ref() {
            "default" => NoiseType::Default,
            "default_mutated" => NoiseType::DefaultMutated,
            "river" => NoiseType::River,
            "ocean" => NoiseType::Ocean,
            "deep_ocean" => NoiseType::DeepOcean,
            "lowlands" => NoiseType::Lowlands,
            "taiga" => NoiseType::Taiga,
            "mountains" => NoiseType::Mountains,
            "highlands" => NoiseType::Highlands,
            "extreme" => NoiseType::Extreme,
            "less_extreme" => NoiseType::LessExtreme,
            "beach" => NoiseType::Beach,
            "stone_beach" => NoiseType::StoneBeach,
            "mushroom" => NoiseType::Mushroom,
            "swamp" => NoiseType::Swamp,
            _ => NoiseType::Default,
        })
    }
}
