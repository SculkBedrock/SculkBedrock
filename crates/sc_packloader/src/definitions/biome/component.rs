//! 生物群系生成组件（`minecraft:*` 组件）的结构与 NBT 序列化。
//!
//! Biome generation components (`minecraft:*`): structures and NBT
//! serialization.

use crate::types::block_specifier::MinecraftBlockSpecifiers;
use crate::types::molang::MolangExpression;
use crate::types::noise_type::NoiseType;
use sc_ecs::component::Component;
use sc_nbt::writer::{NbtCustomWrite, NbtWrite, NbtWriteTrait, NbtWriter};
use sc_nbt::NbtValue;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// `minecraft:capped_surface`：封顶表面材质配置。
///
/// `minecraft:capped_surface`: capped surface material configuration.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CappedSurface {
    /// 海滩材质。
    ///
    /// Beach material.
    pub beach_material: Option<MinecraftBlockSpecifiers>,
    /// 天花板材质列表。
    ///
    /// Ceiling materials.
    pub ceiling_materials: Vec<MinecraftBlockSpecifiers>,
    /// 地板材质列表。
    ///
    /// Floor materials.
    pub floor_materials: Vec<MinecraftBlockSpecifiers>,
    /// 地基材质。
    ///
    /// Foundation material.
    pub foundation_material: MinecraftBlockSpecifiers,
    /// 海水材质。
    ///
    /// Sea material.
    pub sea_material: MinecraftBlockSpecifiers,
}

/// `minecraft:climate`：气候环境效果参数（均可缺省）。
///
/// `minecraft:climate`: climate ambience parameters (all optional).
#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct Climate {
    /// 火山灰浓度。
    ///
    /// Ash density.
    pub ash: Option<f32>,
    /// 蓝色孢子粒子浓度。
    ///
    /// Blue spore particle density.
    pub blue_spores: Option<f32>,
    /// 降水（雨雪）强度。
    ///
    /// Downfall (rain/snow) intensity.
    pub downfall: Option<f32>,
    /// 红色孢子粒子浓度。
    ///
    /// Red spore particle density.
    pub red_spores: Option<f32>,
    /// 积雪累积范围 [min, max]。
    ///
    /// Snow accumulation range [min, max].
    pub snow_accumulation: Option<Vec<f32>>,
    /// 温度。
    ///
    /// Temperature.
    pub temperature: Option<f32>,
    /// 白色灰烬浓度。
    ///
    /// White ash density.
    pub white_ash: Option<f32>,
}

/// `minecraft:frozen_ocean_surface`：冻洋表面材质配置。
///
/// `minecraft:frozen_ocean_surface`: frozen ocean surface materials.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct FrozenOceanSurface {
    /// 地基材质。
    ///
    /// Foundation material.
    pub foundation_material: MinecraftBlockSpecifiers,
    /// 中层材质。
    ///
    /// Mid material.
    pub mid_material: MinecraftBlockSpecifiers,
    /// 海床深度。
    ///
    /// Sea floor depth.
    pub sea_floor_depth: i32,
    /// 海床材质。
    ///
    /// Sea floor material.
    pub sea_floor_material: MinecraftBlockSpecifiers,
    /// 海水材质。
    ///
    /// Sea material.
    pub sea_material: MinecraftBlockSpecifiers,
    /// 表层材质。
    ///
    /// Top material.
    pub top_material: MinecraftBlockSpecifiers,
}

/// `minecraft:mesa_surface`：平顶山表面材质配置。
///
/// `minecraft:mesa_surface`: mesa surface material configuration.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MesaSurface {
    /// 是否生成布莱斯岩柱。
    ///
    /// Whether bryce pillars generate.
    pub bryce_pillars: Option<bool>,
    /// 黏土材质。
    ///
    /// Clay material.
    pub clay_material: MinecraftBlockSpecifiers,
    /// 地基材质。
    ///
    /// Foundation material.
    pub foundation_material: MinecraftBlockSpecifiers,
    /// 硬化黏土材质。
    ///
    /// Hard clay material.
    pub hard_clay_material: MinecraftBlockSpecifiers,
    /// 是否附带森林。
    ///
    /// Whether a forest is attached.
    pub has_forest: Option<bool>,
    /// 中层材质。
    ///
    /// Mid material.
    pub mid_material: MinecraftBlockSpecifiers,
    /// 海床深度。
    ///
    /// Sea floor depth.
    pub sea_floor_depth: i32,
    /// 海床材质。
    ///
    /// Sea floor material.
    pub sea_floor_material: MinecraftBlockSpecifiers,
    /// 海水材质。
    ///
    /// Sea material.
    pub sea_material: MinecraftBlockSpecifiers,
    /// 表层材质。
    ///
    /// Top material.
    pub top_material: MinecraftBlockSpecifiers,
}

/// `minecraft:mountain_parameters`：山地地形参数。
///
/// `minecraft:mountain_parameters`: mountain terrain parameters.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MountainParameters {
    /// 山峰因子。
    ///
    /// Peaks factor.
    pub peaks_factor: Option<f32>,
    /// 陡坡材质调整。
    ///
    /// Steep slope material adjustment.
    pub steep_material_adjustment: Option<SteepMaterialAdjustment>,
    /// 顶部滑动参数。
    ///
    /// Top slide parameters.
    pub top_slide: Option<TopSlide>,
}

/// 陡坡材质调整（按方位开关 + 材质）。
///
/// Steep material adjustment (per-direction toggles plus material).
#[derive(Clone, Deserialize, Debug)]
pub struct SteepMaterialAdjustment {
    /// 东坡是否调整。
    ///
    /// Whether east slopes are adjusted.
    pub east_slopes: Option<bool>,
    /// 使用的材质。
    ///
    /// Material used.
    pub material: MinecraftBlockSpecifiers,
    /// 北坡是否调整。
    ///
    /// Whether north slopes are adjusted.
    pub north_slopes: Option<bool>,
    /// 南坡是否调整。
    ///
    /// Whether south slopes are adjusted.
    pub south_slopes: Option<bool>,
    /// 西坡是否调整。
    ///
    /// Whether west slopes are adjusted.
    pub west_slopes: Option<bool>,
}

/// 顶部滑动开关。
///
/// Top slide toggle.
#[derive(Clone, Deserialize, NbtWrite, Debug)]
pub struct TopSlide {
    /// 是否启用。
    ///
    /// Whether enabled.
    pub enabled: Option<bool>,
}

/// `minecraft:multinoise_generation_rules`：多噪声生成目标参数。
///
/// `minecraft:multinoise_generation_rules`: multi-noise generation targets.
#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct MultiNoiseGenerationRules {
    /// 目标海拔。
    ///
    /// Target altitude.
    pub target_altitude: Option<f32>,
    /// 目标湿度。
    ///
    /// Target humidity.
    pub target_humidity: Option<f32>,
    /// 目标温度。
    ///
    /// Target temperature.
    pub target_temperature: Option<f32>,
    /// 目标奇异度。
    ///
    /// Target weirdness.
    pub target_weirdness: Option<f32>,
    /// 权重。
    ///
    /// Weight.
    pub weight: Option<f32>,
}

/// `minecraft:overworld_generation_rules`：主世界生成转换规则。
///
/// `minecraft:overworld_generation_rules`: overworld generation transform rules.
#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct OverworldGenerationRules {
    /// 按气候类别加权的生成规则。
    ///
    /// Generation rules weighted by climate category.
    pub generate_for_climates: Option<Vec<WeightedClimateCategoriesSettings>>,
    /// 丘陵地形转换。
    ///
    /// Hills transformation.
    pub hills_transformation: Option<WeightedBiomeNamesSettings>,
    /// 变异地形转换。
    ///
    /// Mutate transformation.
    pub mutate_transformation: Option<WeightedBiomeNamesSettings>,
    /// 河流地形转换。
    ///
    /// River transformation.
    pub river_transformation: Option<WeightedBiomeNamesSettings>,
    /// 海岸地形转换。
    ///
    /// Shore transformation.
    pub shore_transformation: Option<WeightedBiomeNamesSettings>,
}

/// 加权生物群系列表：单个名称、嵌套列表或 (名称, 权重) 二元组。
///
/// Weighted biome list: a single name, a nested list, or a (name, weight) pair.
#[derive(Clone, Deserialize, Debug)]
#[serde(untagged)]
pub enum WeightedBiomeNamesSettings {
    /// 单个生物群系名称。
    ///
    /// A single biome name.
    BiomeName(String),
    /// 嵌套的加权列表。
    ///
    /// A nested weighted list.
    Vector(Vec<WeightedBiomeNamesSettings>),
    /// 名称与权重组成的二元组。
    ///
    /// A (name, weight) pair.
    BiomeWeight(BiomeWeight),
}

/// 加权生物群系名称的 NBT 写出：统一展开为 `BiomeWeight` 列表。
///
/// NBT writing for weighted biome names: always expanded into a `BiomeWeight` list.
impl NbtCustomWrite for WeightedBiomeNamesSettings {
    /// 通过 NBT 写入器写出当前值。
    ///
    /// Writes the value through an NBT writer.
    ///
    /// - `writer`: 目标 NBT 写入器
    fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> std::io::Result<()> {
        writer.write::<T>(&self.to_nbt().unwrap())
    }

    /// 转换为 NBT 值；嵌套列表内含非法成员时返回 `None`。
    ///
    /// Converts into an NBT value; `None` when a nested list contains an
    /// invalid member.
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

/// 生物群系权重对（名称 + 权重）。
///
/// Biome weight pair (name plus weight).
#[derive(Clone, Debug, NbtWrite)]
pub struct BiomeWeight {
    /// 生物群系标识名。
    ///
    /// Biome identifier.
    pub biome: String,
    /// 权重。
    ///
    /// Weight.
    pub weight: i32,
}

impl<'de> Deserialize<'de> for BiomeWeight {
    /// 从 `[名称, 权重]` 二元组反序列化；长度或类型非法时报错。
    ///
    /// Deserializes from a `[name, weight]` pair; errors on an invalid
    /// length or type.
    ///
    /// - `deserializer`: serde 反序列化器
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

/// 按气候类别加权的条目（温度类别 + 权重）。
///
/// Climate-category weighted entry (temperature category plus weight).
#[derive(Clone, NbtWrite, Debug)]
pub struct WeightedClimateCategoriesSettings {
    /// 温度类别。
    ///
    /// Temperature category.
    pub temperature: ClimatesTemperature,
    /// 权重。
    ///
    /// Weight.
    pub weight: i32,
}

/// 气候温度类别（判别值即网络序数值）。
///
/// Climate temperature category (discriminant is the network value).
#[repr(u8)]
#[derive(Clone, Debug)]
pub enum ClimatesTemperature {
    /// 温和（0）。
    ///
    /// Medium (0).
    Medium = 0,
    /// 温暖（1）。
    ///
    /// Warm (1).
    Warm = 1,
    /// 微温（2）。
    ///
    /// Lukewarm (2).
    Lukewarm = 2,
    /// 寒冷（3）。
    ///
    /// Cold (3).
    Cold = 3,
    /// 冰冻（4）。
    ///
    /// Frozen (4).
    Frozen = 4,
}

impl ClimatesTemperature {
    /// 从数值构造；越界返回 `None`。
    ///
    /// Builds from a numeric value; `None` when out of range.
    ///
    /// - `value`: 网络序数值
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

    /// 从字符串构造；未知名称返回 `None`。
    ///
    /// Builds from a string; `None` for an unknown name.
    ///
    /// - `value`: 温度类别名称
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

    /// 转换为数值。
    ///
    /// Converts into the numeric value.
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
    /// 通过 NBT 写入器写出单字节值。
    ///
    /// Writes the single-byte value through an NBT writer.
    ///
    /// - `writer`: 目标 NBT 写入器
    fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> std::io::Result<()> {
        writer.write::<T>(&self.to_nbt().unwrap())
    }

    /// 转换为单字节 NBT 值。
    ///
    /// Converts into a single-byte NBT value.
    fn to_nbt(&self) -> Option<NbtValue> {
        Some(NbtValue::Byte(self.to_u8() as i8))
    }
}

impl<'de> Deserialize<'de> for WeightedClimateCategoriesSettings {
    /// 从 `[温度名, 权重]` 二元组反序列化；长度或类型非法时报错。
    ///
    /// Deserializes from a `[temperature name, weight]` pair; errors on an
    /// invalid length or type.
    ///
    /// - `deserializer`: serde 反序列化器
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

/// `minecraft:overworld_height`：主世界高度噪声参数。
///
/// `minecraft:overworld_height`: overworld height noise parameters.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct OverworldHeight {
    /// 噪声参数表。
    ///
    /// Noise parameter table.
    pub noise_params: Option<Vec<f32>>,
    /// 噪声地形类型。
    ///
    /// Noise terrain type.
    pub noise_type: Option<NoiseType>,
}

/// `minecraft:surface_material_adjustments`：表面材质调整列表。
///
/// `minecraft:surface_material_adjustments`: surface material adjustment list.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SurfaceMaterialAdjustments {
    /// 调整条目列表。
    ///
    /// Adjustment entries.
    pub adjustments: Option<Vec<SurfaceMaterialAdjustment>>,
}

/// 表面材质调整的材质集合（各层材质均可缺省）。
///
/// Material set of a surface adjustment (each layer optional).
#[derive(Clone, Deserialize, Debug)]
pub struct SurfaceAdjustmentMaterialsSettings {
    /// 地基材质。
    ///
    /// Foundation material.
    pub foundation_material: Option<MinecraftBlockSpecifiers>,
    /// 中层材质。
    ///
    /// Mid material.
    pub mid_material: Option<MinecraftBlockSpecifiers>,
    /// 海床材质。
    ///
    /// Sea floor material.
    pub sea_floor_material: Option<MinecraftBlockSpecifiers>,
    /// 海水材质。
    ///
    /// Sea material.
    pub sea_material: Option<MinecraftBlockSpecifiers>,
    /// 表层材质。
    ///
    /// Top material.
    pub top_material: Option<MinecraftBlockSpecifiers>,
}

/// 单条表面材质调整：高度范围 + 材质 + 噪声参数。
///
/// A single surface adjustment: height range plus materials plus noise parameters.
#[derive(Clone, Deserialize, Debug)]
pub struct SurfaceMaterialAdjustment {
    /// Molang 表达式给出的高度范围 [min, max]。
    ///
    /// Height range [min, max] given by Molang expressions.
    pub height_range: Option<(MolangExpression, MolangExpression)>,
    /// 调整后的材质集合。
    ///
    /// Adjusted material set.
    pub materials: SurfaceAdjustmentMaterialsSettings,
    /// 噪声频率缩放。
    ///
    /// Noise frequency scale.
    pub noise_frequency_scale: Option<f32>,
    /// 噪声范围 [min, max]。
    ///
    /// Noise range [min, max].
    pub noise_range: Option<(f32, f32)>,
}

/// `minecraft:surface_parameters`：表面各层材质参数。
///
/// `minecraft:surface_parameters`: per-layer surface material parameters.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SurfaceParameters {
    /// 地基材质。
    ///
    /// Foundation material.
    pub foundation_material: MinecraftBlockSpecifiers,
    /// 中层材质。
    ///
    /// Mid material.
    pub mid_material: MinecraftBlockSpecifiers,
    /// 海床深度。
    ///
    /// Sea floor depth.
    pub sea_floor_depth: i32,
    /// 海床材质。
    ///
    /// Sea floor material.
    pub sea_floor_material: MinecraftBlockSpecifiers,
    /// 海水材质。
    ///
    /// Sea material.
    pub sea_material: MinecraftBlockSpecifiers,
    /// 表层材质。
    ///
    /// Top material.
    pub top_material: MinecraftBlockSpecifiers,
}

/// `minecraft:surface_builder`（BDS 1.21.110+ 格式：builder 嵌套主世界材质）。
///
/// `minecraft:surface_builder` (BDS 1.21.110+ format: builder nesting overworld materials).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SurfaceBuilder {
    /// 表面构建器。
    ///
    /// Surface builder.
    pub builder: OverworldSurfaceBuilder,
}

/// 主世界表面构建器：类型 + 各层材质。
///
/// Overworld surface builder: type plus per-layer materials.
#[derive(Clone, Deserialize, Debug)]
pub struct OverworldSurfaceBuilder {
    /// 构建器类型（JSON 键 `type`）。
    ///
    /// Builder type (JSON key `type`).
    #[serde(rename = "type")]
    pub builder_type: String,
    /// 海床深度。
    ///
    /// Sea floor depth.
    pub sea_floor_depth: i32,
    /// 海床材质。
    ///
    /// Sea floor material.
    pub sea_floor_material: MinecraftBlockSpecifiers,
    /// 地基材质。
    ///
    /// Foundation material.
    pub foundation_material: MinecraftBlockSpecifiers,
    /// 中层材质。
    ///
    /// Mid material.
    pub mid_material: MinecraftBlockSpecifiers,
    /// 表层材质。
    ///
    /// Top material.
    pub top_material: MinecraftBlockSpecifiers,
    /// 海水材质。
    ///
    /// Sea material.
    pub sea_material: MinecraftBlockSpecifiers,
}

/// `minecraft:swamp_surface`：沼泽表面材质配置。
///
/// `minecraft:swamp_surface`: swamp surface materials.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SwampSurface {
    /// 地基材质。
    ///
    /// Foundation material.
    pub foundation_material: MinecraftBlockSpecifiers,
    /// 中层材质。
    ///
    /// Mid material.
    pub mid_material: MinecraftBlockSpecifiers,
    /// 海床深度。
    ///
    /// Sea floor depth.
    pub sea_floor_depth: i32,
    /// 海床材质。
    ///
    /// Sea floor material.
    pub sea_floor_material: MinecraftBlockSpecifiers,
    /// 海水材质。
    ///
    /// Sea material.
    pub sea_material: MinecraftBlockSpecifiers,
    /// 表层材质。
    ///
    /// Top material.
    pub top_material: MinecraftBlockSpecifiers,
}

/// `minecraft:tags`：生物群系标签列表。
///
/// `minecraft:tags`: biome tag list.
#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct Tags {
    /// 标签字符串列表。
    ///
    /// Tag strings.
    pub tags: Vec<String>,
}

/// `minecraft:the_end_surface`：末地表面标记（无字段）。
///
/// `minecraft:the_end_surface`: end surface marker (no fields).
#[derive(Clone, Component, NbtWrite, Deserialize, Debug)]
pub struct TheEndSurface {}
