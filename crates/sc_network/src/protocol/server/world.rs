use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use sc_network_macros::MinecraftPacket;
use std::io::{Error, ErrorKind};

/// Bedrock BiomeDefinitionList packet.
///
/// Version packs provide biome game-data NBT/JSON, and this codec
/// serializes it.
#[derive(Debug, Clone, MinecraftPacket)]
pub struct BiomeDefinitionList {
    /// Biome string list. JSON fallback packets can leave this empty and
    /// the writer will build a minimal table from names and tags.
    pub string_list: Vec<String>,
    pub definitions: Vec<BiomeDefinition>,
}

#[derive(Debug, Clone)]
pub struct BiomeDefinition {
    pub name: String,
    pub name_index: Option<u16>,
    pub id: i16,
    pub temperature: f32,
    pub downfall: f32,
    pub foliage_snow: f32,
    pub depth: f32,
    pub scale: f32,
    pub map_water_color: i32,
    pub is_rain: bool,
    pub tag_indices: Vec<u16>,
    pub tags: Vec<String>,
    pub chunk_gen_data: Option<CompoundNbt>,
}

impl BiomeDefinitionList {
    pub fn from_definitions(definitions: Vec<BiomeDefinition>) -> Self {
        Self {
            string_list: Vec::new(),
            definitions,
        }
    }
}

impl Writer for BiomeDefinitionList {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        let mut strings = self.string_list.clone();
        buf.write_var_u32(self.definitions.len() as u32)?;

        for biome in &self.definitions {
            let name = if biome.name.starts_with("minecraft:") {
                biome.name.clone()
            } else {
                format!("minecraft:{}", biome.name)
            };
            let name_index = biome
                .name_index
                .unwrap_or_else(|| add_string(&name, &mut strings));
            buf.write_u16_le(name_index)?;

            buf.write_i16_le(biome.id)?;
            buf.write_f32_le(biome.temperature)?;
            buf.write_f32_le(biome.downfall)?;
            buf.write_f32_le(biome.foliage_snow)?;
            buf.write_f32_le(biome.depth)?;
            buf.write_f32_le(biome.scale)?;
            buf.write_i32_le(biome.map_water_color)?;
            buf.write_bool(biome.is_rain)?;

            let tag_indices = if !biome.tag_indices.is_empty() {
                biome.tag_indices.clone()
            } else {
                biome
                    .tags
                    .iter()
                    .map(|tag| add_string(tag, &mut strings))
                    .collect::<Vec<_>>()
            };
            write_optional(buf, !tag_indices.is_empty(), |buf| {
                buf.write_var_u32(tag_indices.len() as u32)?;
                for tag in tag_indices {
                    buf.write_u16_le(tag)?;
                }
                Ok(())
            })?;

            write_optional(buf, biome.chunk_gen_data.is_some(), |buf| {
                write_chunk_gen(buf, biome.chunk_gen_data.as_ref().unwrap())
            })?;
        }

        buf.write_var_u32(strings.len() as u32)?;
        for value in &strings {
            buf.write_string(value)?;
        }
        Ok(())
    }
}

impl Reader<BiomeDefinitionList> for BiomeDefinitionList {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "world packet decode is unsupported",
        ))
    }
}

fn invalid(message: &'static str) -> Error {
    Error::new(ErrorKind::InvalidData, message)
}

fn add_string(value: &str, strings: &mut Vec<String>) -> u16 {
    if let Some(index) = strings.iter().position(|existing| existing == value) {
        return index as u16;
    }
    let index = strings.len() as u16;
    strings.push(value.to_string());
    index
}

fn write_optional<F>(buf: &mut ByteWriter, present: bool, write: F) -> Result<(), Error>
where
    F: FnOnce(&mut ByteWriter) -> Result<(), Error>,
{
    buf.write_bool(present)?;
    if present {
        write(buf)?;
    }
    Ok(())
}

fn write_optional_compound<F>(
    buf: &mut ByteWriter,
    compound: Option<&CompoundNbt>,
    write: F,
) -> Result<(), Error>
where
    F: FnOnce(&mut ByteWriter, &CompoundNbt) -> Result<(), Error>,
{
    write_optional(buf, compound.is_some(), |buf| write(buf, compound.unwrap()))
}

fn write_optional_list<F>(
    buf: &mut ByteWriter,
    values: Option<&Vec<NbtValue>>,
    write: F,
) -> Result<(), Error>
where
    F: FnOnce(&mut ByteWriter, &Vec<NbtValue>) -> Result<(), Error>,
{
    write_optional(buf, values.is_some(), |buf| write(buf, values.unwrap()))
}

fn write_array<T, F>(buf: &mut ByteWriter, values: &[T], mut write: F) -> Result<(), Error>
where
    F: FnMut(&mut ByteWriter, &T) -> Result<(), Error>,
{
    buf.write_var_u32(values.len() as u32)?;
    for value in values {
        write(buf, value)?;
    }
    Ok(())
}

fn write_chunk_gen(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    // v1001/v2168 order.
    write_optional_compound(buf, compound(data, "climate"), write_climate)?;
    write_optional_list(
        buf,
        compound(data, "consolidatedFeatures").and_then(|data| list(data, "features")),
        write_consolidated_features,
    )?;
    write_optional_compound(buf, compound(data, "mountainParams"), write_mountain_params)?;
    write_optional_list(
        buf,
        compound(data, "surfaceMaterialAdjustments").and_then(|data| list(data, "adjustments")),
        write_surface_material_adjustments,
    )?;
    write_optional_compound(
        buf,
        compound(data, "overworldGenRules"),
        write_overworld_gen_rules,
    )?;
    write_optional_compound(
        buf,
        compound(data, "multinoiseGenRules"),
        write_multinoise_gen_rules,
    )?;
    write_optional_list(
        buf,
        compound(data, "legacyWorldGenRules").and_then(|data| list(data, "legacyPreHills")),
        write_conditional_transformations,
    )?;
    write_optional_list(
        buf,
        list(data, "replacementBiomes"),
        write_replacement_biomes,
    )?;
    write_optional(buf, data.contains_key("villageType"), |buf| {
        buf.write_u8(number_i32(data.get("villageType")).unwrap_or(0) as u8)
    })?;
    write_optional_compound(
        buf,
        compound(data, "surfaceBuilderData"),
        write_surface_builder,
    )?;
    write_optional_compound(
        buf,
        compound(data, "subSurfaceBuilderData"),
        write_surface_builder,
    )
}

fn write_climate(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    buf.write_f32_le(number_f32(data.get("temperature")).unwrap_or(0.5))?;
    buf.write_f32_le(number_f32(data.get("downfall")).unwrap_or(0.5))?;
    let snow = data
        .get("snowAccumulation")
        .or_else(|| data.get("snow_accumulation"));
    let (snow_min, snow_max) = snow
        .and_then(|value| value.as_list())
        .map(|values| {
            let min = values
                .iter()
                .filter_map(number_f32_value)
                .fold(f32::INFINITY, f32::min);
            let max = values
                .iter()
                .filter_map(number_f32_value)
                .fold(f32::NEG_INFINITY, f32::max);
            (
                if min.is_finite() { min } else { 0.0 },
                if max.is_finite() { max } else { 0.0 },
            )
        })
        .unwrap_or_else(|| {
            (
                number_f32(data.get("snowAccumulationMin")).unwrap_or(0.0),
                number_f32(data.get("snowAccumulationMax")).unwrap_or(0.0),
            )
        });
    buf.write_f32_le(snow_min)?;
    buf.write_f32_le(snow_max)
}

fn write_consolidated_features(buf: &mut ByteWriter, values: &Vec<NbtValue>) -> Result<(), Error> {
    write_array(buf, values, |buf, value| {
        let data = value
            .as_compound()
            .ok_or_else(|| invalid("consolidated feature is not compound"))?;
        let scatter =
            compound(data, "scatter").ok_or_else(|| invalid("feature scatter missing"))?;
        write_scatter(buf, scatter)?;
        buf.write_i16_le(number_i16(data.get("feature")).unwrap_or(0))?;
        buf.write_i16_le(number_i16(data.get("identifier")).unwrap_or(0))?;
        buf.write_i16_le(number_i16(data.get("pass")).unwrap_or(0))?;
        buf.write_bool(bool_value(data.get("canUseInternalFeature")).unwrap_or(false))
    })
}

fn write_scatter(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    let empty = Vec::new();
    write_array(
        buf,
        list(data, "coordinates").unwrap_or(&empty),
        write_coordinate,
    )?;
    buf.write_var_i32(number_i32(data.get("evalOrder")).unwrap_or(0))?;
    buf.write_var_i32(number_i32(data.get("chancePercentType")).unwrap_or(-1))?;
    buf.write_i16_le(number_i16(data.get("chancePercent")).unwrap_or(0))?;
    buf.write_i32_le(number_i32(data.get("chanceNumerator")).unwrap_or(0))?;
    buf.write_i32_le(number_i32(data.get("chanceDenominator")).unwrap_or(0))?;
    buf.write_var_i32(number_i32(data.get("iterationsType")).unwrap_or(-1))?;
    buf.write_i16_le(number_i16(data.get("iterations")).unwrap_or(0))
}

fn write_coordinate(buf: &mut ByteWriter, value: &NbtValue) -> Result<(), Error> {
    let data = value
        .as_compound()
        .ok_or_else(|| invalid("coordinate is not compound"))?;
    buf.write_var_i32(number_i32(data.get("minValueType")).unwrap_or(-1))?;
    buf.write_i16_le(number_i16(data.get("minValue")).unwrap_or(0))?;
    buf.write_var_i32(number_i32(data.get("maxValueType")).unwrap_or(-1))?;
    buf.write_i16_le(number_i16(data.get("maxValue")).unwrap_or(0))?;
    buf.write_i32_le(number_i64(data.get("gridOffset")).unwrap_or(0) as i32)?;
    buf.write_i32_le(number_i64(data.get("gridStepSize")).unwrap_or(0) as i32)?;
    buf.write_var_i32(number_i32(data.get("distribution")).unwrap_or(0))
}

fn write_mountain_params(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    write_block(buf, number_i32(data.get("steepBlock")))?;
    buf.write_bool(bool_value(data.get("northSlopes")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("southSlopes")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("westSlopes")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("eastSlopes")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("topSlideEnabled")).unwrap_or(false))
}

fn write_surface_material_adjustments(
    buf: &mut ByteWriter,
    values: &Vec<NbtValue>,
) -> Result<(), Error> {
    write_array(buf, values, |buf, value| {
        let data = value
            .as_compound()
            .ok_or_else(|| invalid("surface adjustment is not compound"))?;
        buf.write_f32_le(number_f32(data.get("noiseFrequencyScale")).unwrap_or(0.0))?;
        buf.write_f32_le(number_f32(data.get("noiseLowerBound")).unwrap_or(0.0))?;
        buf.write_f32_le(number_f32(data.get("noiseUpperBound")).unwrap_or(0.0))?;
        buf.write_var_i32(number_i32(data.get("heightMinType")).unwrap_or(-1))?;
        buf.write_i16_le(number_i16(data.get("heightMin")).unwrap_or(0))?;
        buf.write_var_i32(number_i32(data.get("heightMaxType")).unwrap_or(-1))?;
        buf.write_i16_le(number_i16(data.get("heightMax")).unwrap_or(0))?;
        write_surface_material(
            buf,
            compound(data, "adjustedMaterials")
                .ok_or_else(|| invalid("adjustedMaterials missing"))?,
        )
    })
}

fn write_surface_material(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    write_block(buf, number_i32(data.get("topBlock")))?;
    write_block(buf, number_i32(data.get("midBlock")))?;
    write_block(buf, number_i32(data.get("seaFloorBlock")))?;
    write_block(buf, number_i32(data.get("foundationBlock")))?;
    write_block(buf, number_i32(data.get("seaBlock")))?;
    buf.write_i32_le(number_i32(data.get("seaFloorDepth")).unwrap_or(0))
}

fn write_surface_builder(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    write_optional_compound(
        buf,
        compound(data, "surfaceMaterials"),
        write_surface_material,
    )?;
    buf.write_bool(bool_value(data.get("hasDefaultOverworldSurface")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("hasSwampSurface")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("hasFrozenOceanSurface")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("hasTheEndSurface")).unwrap_or(false))?;
    write_optional_compound(buf, compound(data, "mesaSurface"), write_mesa_surface)?;
    write_optional_compound(buf, compound(data, "cappedSurface"), write_capped_surface)?;
    write_optional_compound(
        buf,
        compound(data, "noiseGradientSurface"),
        write_noise_gradient_surface,
    )
}

fn write_mesa_surface(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    write_block(buf, number_i32(data.get("clayMaterial")))?;
    write_block(buf, number_i32(data.get("hardClayMaterial")))?;
    buf.write_bool(bool_value(data.get("brycePillars")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("hasForest")).unwrap_or(false))
}

fn write_capped_surface(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    let empty = Vec::new();
    write_array(
        buf,
        list(data, "floorBlocks").unwrap_or(&empty),
        |buf, value| write_block(buf, number_i32_value(value)),
    )?;
    write_array(
        buf,
        list(data, "ceilingBlocks").unwrap_or(&empty),
        |buf, value| write_block(buf, number_i32_value(value)),
    )?;
    write_optional(buf, data.contains_key("seaBlock"), |buf| {
        write_block(buf, number_i32(data.get("seaBlock")))
    })?;
    write_optional(buf, data.contains_key("foundationBlock"), |buf| {
        write_block(buf, number_i32(data.get("foundationBlock")))
    })?;
    write_optional(buf, data.contains_key("beachBlock"), |buf| {
        write_block(buf, number_i32(data.get("beachBlock")))
    })
}

fn write_noise_gradient_surface(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    let empty = Vec::new();
    write_array(
        buf,
        list(data, "nonReplaceableBlocks").unwrap_or(&empty),
        |buf, value| write_block(buf, number_i32_value(value)),
    )?;
    write_array(
        buf,
        list(data, "gradientBlocks").unwrap_or(&empty),
        |buf, value| {
            if let Some(data) = value.as_compound() {
                buf.write_string(string_value(data.get("noise")).unwrap_or(""))?;
                buf.write_f32_le(number_f32(data.get("threshold")).unwrap_or(0.0))?;
                if let Some(range) = compound(data, "range") {
                    buf.write_f32_le(number_f32(range.get("min")).unwrap_or(0.0))?;
                    buf.write_f32_le(number_f32(range.get("max")).unwrap_or(0.0))?;
                } else {
                    buf.write_f32_le(0.0)?;
                    buf.write_f32_le(0.0)?;
                }
                write_block(buf, number_i32(data.get("block")))
            } else {
                buf.write_string("")?;
                buf.write_f32_le(0.0)?;
                buf.write_f32_le(0.0)?;
                buf.write_f32_le(0.0)?;
                write_block(buf, number_i32_value(value))
            }
        },
    )?;
    buf.write_string(string_value(data.get("name")).unwrap_or(""))?;
    buf.write_i32_le(number_i32(data.get("firstOctave")).unwrap_or(0))?;
    write_array(
        buf,
        list(data, "amplitudes").unwrap_or(&empty),
        |buf, value| buf.write_f32_le(number_f32_value(value).unwrap_or(0.0)),
    )
}

fn write_overworld_gen_rules(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    let empty = Vec::new();
    write_array(
        buf,
        list(data, "hillsTransformations").unwrap_or(&empty),
        write_weight,
    )?;
    write_array(
        buf,
        list(data, "mutateTransformations").unwrap_or(&empty),
        write_weight,
    )?;
    write_array(
        buf,
        list(data, "riverTransformations").unwrap_or(&empty),
        write_weight,
    )?;
    write_array(
        buf,
        list(data, "shoreTransformations").unwrap_or(&empty),
        write_weight,
    )?;
    write_array(
        buf,
        list(data, "preHillsEdge").unwrap_or(&empty),
        write_conditional_transformation,
    )?;
    write_array(
        buf,
        list(data, "postShoreEdge").unwrap_or(&empty),
        write_conditional_transformation,
    )?;
    write_array(
        buf,
        list(data, "climate").unwrap_or(&empty),
        |buf, value| {
            let data = value
                .as_compound()
                .ok_or_else(|| invalid("weighted temperature is not compound"))?;
            buf.write_var_i32(number_i32(data.get("temperature")).unwrap_or(0))?;
            buf.write_i32_le(number_i64(data.get("weight")).unwrap_or(0) as i32)
        },
    )
}

fn write_weight(buf: &mut ByteWriter, value: &NbtValue) -> Result<(), Error> {
    let data = value
        .as_compound()
        .ok_or_else(|| invalid("weighted biome is not compound"))?;
    buf.write_i16_le(number_i16(data.get("biomeIdentifier")).unwrap_or(0))?;
    buf.write_i32_le(number_i32(data.get("weight")).unwrap_or(0))
}

fn write_conditional_transformations(
    buf: &mut ByteWriter,
    values: &Vec<NbtValue>,
) -> Result<(), Error> {
    write_array(buf, values, write_conditional_transformation)
}

fn write_conditional_transformation(buf: &mut ByteWriter, value: &NbtValue) -> Result<(), Error> {
    let data = value
        .as_compound()
        .ok_or_else(|| invalid("conditional transformation is not compound"))?;
    let empty = Vec::new();
    write_array(
        buf,
        list(data, "transformsInto").unwrap_or(&empty),
        write_weight,
    )?;
    buf.write_i16_le(number_i16(data.get("conditionJson")).unwrap_or(0))?;
    buf.write_i32_le(number_i64(data.get("minPassingNeighbors")).unwrap_or(0) as i32)
}

fn write_multinoise_gen_rules(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    buf.write_f32_le(number_f32(data.get("temperature")).unwrap_or(0.0))?;
    buf.write_f32_le(number_f32(data.get("humidity")).unwrap_or(0.0))?;
    buf.write_f32_le(number_f32(data.get("altitude")).unwrap_or(0.0))?;
    buf.write_f32_le(number_f32(data.get("weirdness")).unwrap_or(0.0))?;
    buf.write_f32_le(number_f32(data.get("weight")).unwrap_or(0.0))
}

fn write_replacement_biomes(buf: &mut ByteWriter, values: &Vec<NbtValue>) -> Result<(), Error> {
    write_array(buf, values, |buf, value| {
        let data = value
            .as_compound()
            .ok_or_else(|| invalid("replacement biome is not compound"))?;
        buf.write_i16_le(
            number_i16(data.get("replacementBiome"))
                .or_else(|| number_i16(data.get("biome")))
                .unwrap_or(0),
        )?;
        buf.write_i16_le(number_i16(data.get("dimension")).unwrap_or(0))?;
        let empty = Vec::new();
        write_array(
            buf,
            list(data, "targetBiomes").unwrap_or(&empty),
            |buf, value| buf.write_i16_le(number_i16_value(value).unwrap_or(0)),
        )?;
        buf.write_f32_le(number_f32(data.get("amount")).unwrap_or(0.0))?;
        buf.write_f32_le(number_f32(data.get("noiseFrequencyScale")).unwrap_or(0.0))?;
        buf.write_i32_le(number_i32(data.get("replacementIndex")).unwrap_or(0))
    })
}

fn write_block(buf: &mut ByteWriter, runtime_id: Option<i32>) -> Result<(), Error> {
    buf.write_i32_le(runtime_id.unwrap_or(-1))
}

fn compound<'a>(compound: &'a CompoundNbt, key: &str) -> Option<&'a CompoundNbt> {
    compound
        .get(key)
        .or_else(|| namespaced_component(compound, key))?
        .as_compound()
}

fn list<'a>(compound: &'a CompoundNbt, key: &str) -> Option<&'a Vec<NbtValue>> {
    compound
        .get(key)
        .or_else(|| namespaced_component(compound, key))?
        .as_list()
}

fn namespaced_component<'a>(compound: &'a CompoundNbt, key: &str) -> Option<&'a NbtValue> {
    let alias = match key {
        "climate" => "minecraft:climate",
        "surfaceMaterialAdjustments" => "minecraft:surface_material_adjustments",
        "overworldGenRules" => "minecraft:overworld_generation_rules",
        "multinoiseGenRules" => "minecraft:multinoise_generation_rules",
        "surfaceBuilderData" => "minecraft:surface_builder",
        "subSurfaceBuilderData" => "minecraft:surface_builder",
        _ => return None,
    };
    compound.get(alias)
}

fn string_value(value: Option<&NbtValue>) -> Option<&str> {
    value?.as_string().map(String::as_str)
}

fn bool_value(value: Option<&NbtValue>) -> Option<bool> {
    match value? {
        NbtValue::Byte(value) => Some(*value != 0),
        NbtValue::Int(value) => Some(*value != 0),
        _ => None,
    }
}

fn number_i16(value: Option<&NbtValue>) -> Option<i16> {
    number_i16_value(value?)
}

fn number_i16_value(value: &NbtValue) -> Option<i16> {
    match value {
        NbtValue::Byte(value) => Some(*value as i16),
        NbtValue::Short(value) => Some(*value),
        NbtValue::Int(value) => i16::try_from(*value).ok(),
        _ => None,
    }
}

fn number_i32(value: Option<&NbtValue>) -> Option<i32> {
    number_i32_value(value?)
}

fn number_i32_value(value: &NbtValue) -> Option<i32> {
    match value {
        NbtValue::Byte(value) => Some(*value as i32),
        NbtValue::Short(value) => Some(*value as i32),
        NbtValue::Int(value) => Some(*value),
        NbtValue::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn number_i64(value: Option<&NbtValue>) -> Option<i64> {
    match value? {
        NbtValue::Byte(value) => Some(*value as i64),
        NbtValue::Short(value) => Some(*value as i64),
        NbtValue::Int(value) => Some(*value as i64),
        NbtValue::Long(value) => Some(*value),
        _ => None,
    }
}

fn number_f32(value: Option<&NbtValue>) -> Option<f32> {
    number_f32_value(value?)
}

fn number_f32_value(value: &NbtValue) -> Option<f32> {
    match value {
        NbtValue::Float(value) => Some(*value),
        NbtValue::Double(value) => Some(*value as f32),
        NbtValue::Byte(value) => Some(*value as f32),
        NbtValue::Short(value) => Some(*value as f32),
        NbtValue::Int(value) => Some(*value as f32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::BiomeDefinitionList;
    use sc_binary::interfaces::Writer;
    use sc_binary::ByteWriter;

    #[test]
    fn empty_definition_table_is_two_varints() {
        let mut writer = ByteWriter::new();
        BiomeDefinitionList::from_definitions(Vec::new())
            .write(&mut writer)
            .unwrap();

        assert_eq!(writer.as_slice(), &[0, 0]);
    }
}
