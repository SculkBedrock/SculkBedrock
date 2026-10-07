use crate::types::block_specifier::MinecraftBlockSpecifiers;
use crate::types::molang::MolangExpression;
use crate::types::noise_type::NoiseType;
use sc_ecs::component::Component;
use sc_nbt::writer::{NbtCustomWrite, NbtWrite, NbtWriteTrait, NbtWriter};
use sc_nbt::NbtValue;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

#[derive(Clone, Component, Deserialize, Debug)]
pub struct CappedSurface {
    pub beach_material: Option<MinecraftBlockSpecifiers>,
    pub ceiling_materials: Vec<MinecraftBlockSpecifiers>,
    pub floor_materials: Vec<MinecraftBlockSpecifiers>,
    pub foundation_material: MinecraftBlockSpecifiers,
    pub sea_material: MinecraftBlockSpecifiers,
}

#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct Climate {
    pub ash: Option<f32>,
    pub blue_spores: Option<f32>,
    pub downfall: Option<f32>,
    pub red_spores: Option<f32>,
    pub snow_accumulation: Option<Vec<f32>>,
    pub temperature: Option<f32>,
    pub white_ash: Option<f32>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct FrozenOceanSurface {
    pub foundation_material: MinecraftBlockSpecifiers,
    pub mid_material: MinecraftBlockSpecifiers,
    pub sea_floor_depth: i32,
    pub sea_floor_material: MinecraftBlockSpecifiers,
    pub sea_material: MinecraftBlockSpecifiers,
    pub top_material: MinecraftBlockSpecifiers,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct MesaSurface {
    pub bryce_pillars: Option<bool>,
    pub clay_material: MinecraftBlockSpecifiers,
    pub foundation_material: MinecraftBlockSpecifiers,
    pub hard_clay_material: MinecraftBlockSpecifiers,
    pub has_forest: Option<bool>,
    pub mid_material: MinecraftBlockSpecifiers,
    pub sea_floor_depth: i32,
    pub sea_floor_material: MinecraftBlockSpecifiers,
    pub sea_material: MinecraftBlockSpecifiers,
    pub top_material: MinecraftBlockSpecifiers,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct MountainParameters {
    pub peaks_factor: Option<f32>,
    pub steep_material_adjustment: Option<SteepMaterialAdjustment>,
    pub top_slide: Option<TopSlide>,
}

#[derive(Clone, Deserialize, Debug)]
pub struct SteepMaterialAdjustment {
    pub east_slopes: Option<bool>,
    pub material: MinecraftBlockSpecifiers,
    pub north_slopes: Option<bool>,
    pub south_slopes: Option<bool>,
    pub west_slopes: Option<bool>,
}

#[derive(Clone, Deserialize, NbtWrite, Debug)]
pub struct TopSlide {
    pub enabled: Option<bool>,
}

#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct MultiNoiseGenerationRules {
    pub target_altitude: Option<f32>,
    pub target_humidity: Option<f32>,
    pub target_temperature: Option<f32>,
    pub target_weirdness: Option<f32>,
    pub weight: Option<f32>,
}

#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct OverworldGenerationRules {
    pub generate_for_climates: Option<Vec<WeightedClimateCategoriesSettings>>,
    pub hills_transformation: Option<WeightedBiomeNamesSettings>,
    pub mutate_transformation: Option<WeightedBiomeNamesSettings>,
    pub river_transformation: Option<WeightedBiomeNamesSettings>,
    pub shore_transformation: Option<WeightedBiomeNamesSettings>,
}

#[derive(Clone, Deserialize, Debug)]
#[serde(untagged)]
pub enum WeightedBiomeNamesSettings {
    BiomeName(String),
    Vector(Vec<WeightedBiomeNamesSettings>),
    BiomeWeight(BiomeWeight),
}

impl NbtCustomWrite for WeightedBiomeNamesSettings {
    fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> std::io::Result<()> {
        writer.write::<T>(&self.to_nbt().unwrap())
    }

    fn to_nbt(&self) -> Option<NbtValue> {
        let vec = match self {
            Self::BiomeName(name) => vec![BiomeWeight {
                biome: name.clone(),
                weight: 1,
            }
            .to_nbt()?],
            Self::Vector(vec) => {
                let mut v = Vec::new();
                for i in vec {
                    if let Self::BiomeWeight(w) = i {
                        v.push(w.to_nbt()?);
                    } else if let Self::BiomeName(w) = i {
                        v.push(
                            BiomeWeight {
                                biome: w.clone(),
                                weight: 1,
                            }
                            .to_nbt()?,
                        );
                    } else {
                        return None;
                    }
                }
                v
            }
            Self::BiomeWeight(weight) => vec![weight.to_nbt()?],
        };
        Some(NbtValue::List(vec))
    }
}

#[derive(Clone, Debug, NbtWrite)]
pub struct BiomeWeight {
    pub biome: String,
    pub weight: i32,
}

impl<'de> Deserialize<'de> for BiomeWeight {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let x: Vec<Value> = Vec::deserialize(deserializer)?;
        if x.len() != 2 {
            return Err(Error::custom("Invalid BiomeWeight"));
        }
        let biome = x[0]
            .as_str()
            .ok_or(Error::custom("Invalid biome name"))?
            .to_string();
        let weight = x[1].as_i64().ok_or(Error::custom("Invalid weight value"))? as i32;
        Ok(Self { biome, weight })
    }
}

#[derive(Clone, NbtWrite, Debug)]
pub struct WeightedClimateCategoriesSettings {
    pub temperature: ClimatesTemperature,
    pub weight: i32,
}

#[repr(u8)]
#[derive(Clone, Debug)]
pub enum ClimatesTemperature {
    Medium = 0,
    Warm = 1,
    Lukewarm = 2,
    Cold = 3,
    Frozen = 4,
}

impl ClimatesTemperature {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Medium),
            1 => Some(Self::Warm),
            2 => Some(Self::Lukewarm),
            3 => Some(Self::Cold),
            4 => Some(Self::Frozen),
            _ => None,
        }
    }

    pub fn from_string(value: String) -> Option<Self> {
        match value.as_str() {
            "medium" => Some(Self::Medium),
            "warm" => Some(Self::Warm),
            "lukewarm" => Some(Self::Lukewarm),
            "cold" => Some(Self::Cold),
            "frozen" => Some(Self::Frozen),
            _ => None,
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            ClimatesTemperature::Medium => 0,
            ClimatesTemperature::Warm => 1,
            ClimatesTemperature::Lukewarm => 2,
            ClimatesTemperature::Cold => 3,
            ClimatesTemperature::Frozen => 4,
        }
    }
}

impl NbtCustomWrite for ClimatesTemperature {
    fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> std::io::Result<()> {
        writer.write::<T>(&self.to_nbt().unwrap())
    }

    fn to_nbt(&self) -> Option<NbtValue> {
        Some(NbtValue::Byte(self.to_u8() as i8))
    }
}

impl<'de> Deserialize<'de> for WeightedClimateCategoriesSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let x: Vec<Value> = Vec::deserialize(deserializer)?;
        if x.len() != 2 {
            return Err(Error::custom("Invalid ClimateCategoriesSettings"));
        }
        let temperature = ClimatesTemperature::from_string(
            x[0].as_str()
                .ok_or(Error::custom("Invalid temperature value type"))?
                .to_string(),
        )
        .ok_or(Error::custom("Invalid temperature value"))?;
        let weight = x[1].as_i64().ok_or(Error::custom("Invalid weight value"))? as i32;
        Ok(Self {
            temperature,
            weight,
        })
    }
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct OverworldHeight {
    pub noise_params: Option<Vec<f32>>,
    pub noise_type: Option<NoiseType>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct SurfaceMaterialAdjustments {
    pub adjustments: Option<Vec<SurfaceMaterialAdjustment>>,
}

#[derive(Clone, Deserialize, Debug)]
pub struct SurfaceAdjustmentMaterialsSettings {
    pub foundation_material: Option<MinecraftBlockSpecifiers>,
    pub mid_material: Option<MinecraftBlockSpecifiers>,
    pub sea_floor_material: Option<MinecraftBlockSpecifiers>,
    pub sea_material: Option<MinecraftBlockSpecifiers>,
    pub top_material: Option<MinecraftBlockSpecifiers>,
}

#[derive(Clone, Deserialize, Debug)]
pub struct SurfaceMaterialAdjustment {
    pub height_range: Option<(MolangExpression, MolangExpression)>,
    pub materials: SurfaceAdjustmentMaterialsSettings,
    pub noise_frequency_scale: Option<f32>,
    pub noise_range: Option<(f32, f32)>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct SurfaceParameters {
    pub foundation_material: MinecraftBlockSpecifiers,
    pub mid_material: MinecraftBlockSpecifiers,
    pub sea_floor_depth: i32,
    pub sea_floor_material: MinecraftBlockSpecifiers,
    pub sea_material: MinecraftBlockSpecifiers,
    pub top_material: MinecraftBlockSpecifiers,
}

/// `minecraft:surface_builder` (BDS 1.21.110+ format: builder nesting overworld materials).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SurfaceBuilder {
    pub builder: OverworldSurfaceBuilder,
}

#[derive(Clone, Deserialize, Debug)]
pub struct OverworldSurfaceBuilder {
    #[serde(rename = "type")]
    pub builder_type: String,
    pub sea_floor_depth: i32,
    pub sea_floor_material: MinecraftBlockSpecifiers,
    pub foundation_material: MinecraftBlockSpecifiers,
    pub mid_material: MinecraftBlockSpecifiers,
    pub top_material: MinecraftBlockSpecifiers,
    pub sea_material: MinecraftBlockSpecifiers,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct SwampSurface {
    pub foundation_material: MinecraftBlockSpecifiers,
    pub mid_material: MinecraftBlockSpecifiers,
    pub sea_floor_depth: i32,
    pub sea_floor_material: MinecraftBlockSpecifiers,
    pub sea_material: MinecraftBlockSpecifiers,
    pub top_material: MinecraftBlockSpecifiers,
}

#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct Tags {
    pub tags: Vec<String>,
}

#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct TheEndSurface {}
