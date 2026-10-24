use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use sc_network_macros::MinecraftPacket;
use std::io::{Error, ErrorKind};

/// BiomeDefinitionListPacket：生物群系定义表。
///
/// BiomeDefinitionListPacket: biome definition table.
///
/// 版本包提供生物群系游戏数据 NBT/JSON，本编解码器负责序列化。
#[derive(Debug, Clone, MinecraftPacket)]
pub struct BiomeDefinitionList {
    /// 生物群系字符串表。JSON 回退包可留空，写入时会根据名称与标签
    /// 构建最小表。
    ///
    /// Biome string list. JSON fallback packets can leave this empty and
    /// the writer will build a minimal table from names and tags.
    pub string_list: Vec<String>,
    /// 生物群系定义列表。
    ///
    /// Biome definitions.
    pub definitions: Vec<BiomeDefinition>,
}

/// BiomeDefinition：单个生物群系定义条目。
///
/// BiomeDefinition: a single biome definition entry.
#[derive(Debug, Clone)]
pub struct BiomeDefinition {
    /// 生物群系名称（不含命名空间时自动加 `minecraft:` 前缀）。
    ///
    /// Biome name (`minecraft:` prefix is added when absent).
    pub name: String,
    /// 名称在字符串表中的索引；`None` 时由写入器分配。
    ///
    /// Index of the name in the string table; assigned by the writer when
    /// `None`.
    pub name_index: Option<u16>,
    /// 生物群系数字 id。
    ///
    /// Numeric biome id.
    pub id: i16,
    /// 温度。
    ///
    /// Temperature.
    pub temperature: f32,
    /// 降水（downfall）。
    ///
    /// Downfall.
    pub downfall: f32,
    /// 积雪（foliage snow）。
    ///
    /// Foliage snow.
    pub foliage_snow: f32,
    /// 深度。
    ///
    /// Depth.
    pub depth: f32,
    /// 缩放。
    ///
    /// Scale.
    pub scale: f32,
    /// 地图水色。
    ///
    /// Map water color.
    pub map_water_color: i32,
    /// 是否降雨。
    ///
    /// Whether it rains.
    pub is_rain: bool,
    /// 标签索引列表（非空时优先于 `tags`）。
    ///
    /// Tag index list (takes precedence over `tags` when non-empty).
    pub tag_indices: Vec<u16>,
    /// 标签字符串列表。
    ///
    /// Tag string list.
    pub tags: Vec<String>,
    /// 区块生成数据 NBT。
    ///
    /// Chunk generation data NBT.
    pub chunk_gen_data: Option<CompoundNbt>,
}

impl BiomeDefinitionList {
    /// 从定义列表构造包（字符串表留空）。
    ///
    /// Builds a packet from a definition list (string table left empty).
    ///
    /// - `definitions`: 生物群系定义列表
    pub fn from_definitions(definitions: Vec<BiomeDefinition>) -> Self {
        Self {
            string_list: Vec::new(),
            definitions,
        }
    }
}

impl Writer for BiomeDefinitionList {
    /// 写入 BiomeDefinitionList 包体。
    ///
    /// Writes the BiomeDefinitionList packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
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
    /// 不支持解码：服务端只发送生物群系定义表。
    ///
    /// Decoding is unsupported: the server only sends the biome table.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "world packet decode is unsupported",
        ))
    }
}

/// 构造一个 InvalidData 错误。
///
/// Builds an InvalidData error.
///
/// - `message`: 错误消息
fn invalid(message: &'static str) -> Error {
    Error::new(ErrorKind::InvalidData, message)
}

/// 把字符串加入字符串表并去重，返回其索引。
///
/// Appends a string to the string table (deduplicated) and returns its
/// index.
///
/// - `value`: 字符串
/// - `strings`: 字符串表
fn add_string(value: &str, strings: &mut Vec<String>) -> u16 {
    if let Some(index) = strings.iter().position(|existing| existing == value) {
        return index as u16;
    }
    let index = strings.len() as u16;
    strings.push(value.to_string());
    index
}

/// 写入可选字段：先写存在性 bool，存在时写内容。
///
/// Writes an optional field: a presence bool first, then the content when
/// present.
///
/// - `buf`: 输出字节写入器
/// - `present`: 是否存在
/// - `write`: 内容写入闭包
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入可选 NBT 复合体字段。
///
/// Writes an optional NBT compound field.
///
/// - `buf`: 输出字节写入器
/// - `compound`: 可选的复合体
/// - `write`: 内容写入闭包
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入可选 NBT 列表字段。
///
/// Writes an optional NBT list field.
///
/// - `buf`: 输出字节写入器
/// - `values`: 可选的列表
/// - `write`: 内容写入闭包
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入变长数组：var_u32 长度 + 逐项内容。
///
/// Writes a var-length array: a var_u32 length followed by each item.
///
/// - `buf`: 输出字节写入器
/// - `values`: 值切片
/// - `write`: 单项写入闭包
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入生物群系区块生成数据（v1001/v2168 字段顺序）。
///
/// Writes the biome chunk generation data (v1001/v2168 field order).
///
/// - `buf`: 输出字节写入器
/// - `data`: 生成数据 NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入气候参数（温度、降水、积雪范围）。
///
/// Writes climate parameters (temperature, downfall, snow accumulation
/// range).
///
/// - `buf`: 输出字节写入器
/// - `data`: 气候 NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入 consolidated features 数组。
///
/// Writes the consolidated features array.
///
/// - `buf`: 输出字节写入器
/// - `values`: 特性 NBT 列表
///
/// # Errors
/// 条目不是复合体或写入失败时返回错误。
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

/// 写入特性散布参数（坐标、顺序、概率、迭代）。
///
/// Writes feature scatter parameters (coordinates, order, chance,
/// iterations).
///
/// - `buf`: 输出字节写入器
/// - `data`: 散布 NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入单个散布坐标条目。
///
/// Writes one scatter coordinate entry.
///
/// - `buf`: 输出字节写入器
/// - `value`: 坐标 NBT
///
/// # Errors
/// 条目不是复合体或写入失败时返回错误。
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

/// 写入山地参数（陡坡方块与四向斜坡开关）。
///
/// Writes mountain parameters (steep block and per-side slope toggles).
///
/// - `buf`: 输出字节写入器
/// - `data`: 参数 NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_mountain_params(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    write_block(buf, number_i32(data.get("steepBlock")))?;
    buf.write_bool(bool_value(data.get("northSlopes")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("southSlopes")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("westSlopes")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("eastSlopes")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("topSlideEnabled")).unwrap_or(false))
}

/// 写入地表材质调整数组（噪声边界、高度范围与调整后材质）。
///
/// Writes the surface material adjustment array (noise bounds, height
/// range and adjusted materials).
///
/// - `buf`: 输出字节写入器
/// - `values`: 调整项 NBT 列表
///
/// # Errors
/// 条目不是复合体或写入失败时返回错误。
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

/// 写入地表材质（表层/中层/海床/基底/海水方块与海床深度）。
///
/// Writes a surface material (top/mid/sea-floor/foundation/sea blocks and
/// sea-floor depth).
///
/// - `buf`: 输出字节写入器
/// - `data`: 材质 NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_surface_material(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    write_block(buf, number_i32(data.get("topBlock")))?;
    write_block(buf, number_i32(data.get("midBlock")))?;
    write_block(buf, number_i32(data.get("seaFloorBlock")))?;
    write_block(buf, number_i32(data.get("foundationBlock")))?;
    write_block(buf, number_i32(data.get("seaBlock")))?;
    buf.write_i32_le(number_i32(data.get("seaFloorDepth")).unwrap_or(0))
}

/// 写入地表建造器数据（材质、预设开关与各子表面）。
///
/// Writes the surface builder data (materials, preset toggles and
/// sub-surfaces).
///
/// - `buf`: 输出字节写入器
/// - `data`: 建造器 NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入台地（mesa）地表参数。
///
/// Writes mesa surface parameters.
///
/// - `buf`: 输出字节写入器
/// - `data`: mesa NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_mesa_surface(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    write_block(buf, number_i32(data.get("clayMaterial")))?;
    write_block(buf, number_i32(data.get("hardClayMaterial")))?;
    buf.write_bool(bool_value(data.get("brycePillars")).unwrap_or(false))?;
    buf.write_bool(bool_value(data.get("hasForest")).unwrap_or(false))
}

/// 写入封顶（capped）地表参数（地板/天花板方块与可选方块）。
///
/// Writes capped surface parameters (floor/ceiling blocks and optional
/// blocks).
///
/// - `buf`: 输出字节写入器
/// - `data`: capped NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入噪声梯度地表参数（梯度块、噪声与振幅）。
///
/// Writes noise gradient surface parameters (gradient blocks, noise and
/// amplitudes).
///
/// - `buf`: 输出字节写入器
/// - `data`: 梯度地表 NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
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

/// 写入主世界生成规则（丘陵/变异/河流/岸边变换与气候权重）。
///
/// Writes overworld generation rules (hill/mutation/river/shore
/// transformations and climate weights).
///
/// - `buf`: 输出字节写入器
/// - `data`: 生成规则 NBT
///
/// # Errors
/// 条目不是复合体或写入失败时返回错误。
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

/// 写入带权生物群系条目（生物群系 id + 权重）。
///
/// Writes a weighted biome entry (biome id + weight).
///
/// - `buf`: 输出字节写入器
/// - `value`: 条目 NBT
///
/// # Errors
/// 条目不是复合体或写入失败时返回错误。
fn write_weight(buf: &mut ByteWriter, value: &NbtValue) -> Result<(), Error> {
    let data = value
        .as_compound()
        .ok_or_else(|| invalid("weighted biome is not compound"))?;
    buf.write_i16_le(number_i16(data.get("biomeIdentifier")).unwrap_or(0))?;
    buf.write_i32_le(number_i32(data.get("weight")).unwrap_or(0))
}

/// 写入条件变换数组。
///
/// Writes the conditional transformation array.
///
/// - `buf`: 输出字节写入器
/// - `values`: 变换项 NBT 列表
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_conditional_transformations(
    buf: &mut ByteWriter,
    values: &Vec<NbtValue>,
) -> Result<(), Error> {
    write_array(buf, values, write_conditional_transformation)
}

/// 写入单个条件变换（目标生物群系、条件与邻居阈值）。
///
/// Writes a single conditional transformation (target biomes, condition
/// and neighbor threshold).
///
/// - `buf`: 输出字节写入器
/// - `value`: 变换项 NBT
///
/// # Errors
/// 条目不是复合体或写入失败时返回错误。
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

/// 写入多噪声生成规则（温度/湿度/海拔/奇异度/权重）。
///
/// Writes multinoise generation rules (temperature/humidity/altitude/
/// weirdness/weight).
///
/// - `buf`: 输出字节写入器
/// - `data`: 规则 NBT
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_multinoise_gen_rules(buf: &mut ByteWriter, data: &CompoundNbt) -> Result<(), Error> {
    buf.write_f32_le(number_f32(data.get("temperature")).unwrap_or(0.0))?;
    buf.write_f32_le(number_f32(data.get("humidity")).unwrap_or(0.0))?;
    buf.write_f32_le(number_f32(data.get("altitude")).unwrap_or(0.0))?;
    buf.write_f32_le(number_f32(data.get("weirdness")).unwrap_or(0.0))?;
    buf.write_f32_le(number_f32(data.get("weight")).unwrap_or(0.0))
}

/// 写入生物群系替换表数组。
///
/// Writes the replacement biome array.
///
/// - `buf`: 输出字节写入器
/// - `values`: 替换项 NBT 列表
///
/// # Errors
/// 条目不是复合体或写入失败时返回错误。
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

/// 写入方块运行时 id（缺失时写 -1）。
///
/// Writes a block runtime id (-1 when absent).
///
/// - `buf`: 输出字节写入器
/// - `runtime_id`: 可选的方块运行时 id
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_block(buf: &mut ByteWriter, runtime_id: Option<i32>) -> Result<(), Error> {
    buf.write_i32_le(runtime_id.unwrap_or(-1))
}

/// 从 NBT 复合体中取子复合体（兼容 `minecraft:` 命名空间键）。
///
/// Gets a child compound from an NBT compound (accepts `minecraft:`
/// namespaced keys too).
///
/// - `compound`: 父复合体
/// - `key`: 键名
fn compound<'a>(compound: &'a CompoundNbt, key: &str) -> Option<&'a CompoundNbt> {
    compound
        .get(key)
        .or_else(|| namespaced_component(compound, key))?
        .as_compound()
}

/// 从 NBT 复合体中取子列表（兼容 `minecraft:` 命名空间键）。
///
/// Gets a child list from an NBT compound (accepts `minecraft:`
/// namespaced keys too).
///
/// - `compound`: 父复合体
/// - `key`: 键名
fn list<'a>(compound: &'a CompoundNbt, key: &str) -> Option<&'a Vec<NbtValue>> {
    compound
        .get(key)
        .or_else(|| namespaced_component(compound, key))?
        .as_list()
}

/// 把旧键名映射到 `minecraft:` 命名空间键并取值。
///
/// Maps a legacy key to its `minecraft:` namespaced alias and reads it.
///
/// - `compound`: 父复合体
/// - `key`: 旧键名
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

/// 取 NBT 字符串值。
///
/// Reads an NBT string value.
///
/// - `value`: 可选的 NBT 值
fn string_value(value: Option<&NbtValue>) -> Option<&str> {
    value?.as_string().map(String::as_str)
}

/// 取 NBT 布尔值（Byte/Int 非零即真）。
///
/// Reads an NBT boolean (Byte/Int non-zero is true).
///
/// - `value`: 可选的 NBT 值
fn bool_value(value: Option<&NbtValue>) -> Option<bool> {
    match value? {
        NbtValue::Byte(value) => Some(*value != 0),
        NbtValue::Int(value) => Some(*value != 0),
        _ => None,
    }
}

/// 取 NBT i16 值。
///
/// Reads an NBT i16 value.
///
/// - `value`: 可选的 NBT 值
fn number_i16(value: Option<&NbtValue>) -> Option<i16> {
    number_i16_value(value?)
}

/// 取 NBT i16 值（窄转换）。
///
/// Reads an NBT value as i16 (narrowing conversion).
///
/// - `value`: NBT 值
fn number_i16_value(value: &NbtValue) -> Option<i16> {
    match value {
        NbtValue::Byte(value) => Some(*value as i16),
        NbtValue::Short(value) => Some(*value),
        NbtValue::Int(value) => i16::try_from(*value).ok(),
        _ => None,
    }
}

/// 取 NBT i32 值。
///
/// Reads an NBT i32 value.
///
/// - `value`: 可选的 NBT 值
fn number_i32(value: Option<&NbtValue>) -> Option<i32> {
    number_i32_value(value?)
}

/// 取 NBT i32 值（窄转换）。
///
/// Reads an NBT value as i32 (narrowing conversion).
///
/// - `value`: NBT 值
fn number_i32_value(value: &NbtValue) -> Option<i32> {
    match value {
        NbtValue::Byte(value) => Some(*value as i32),
        NbtValue::Short(value) => Some(*value as i32),
        NbtValue::Int(value) => Some(*value),
        NbtValue::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

/// 取 NBT i64 值。
///
/// Reads an NBT i64 value.
///
/// - `value`: 可选的 NBT 值
fn number_i64(value: Option<&NbtValue>) -> Option<i64> {
    match value? {
        NbtValue::Byte(value) => Some(*value as i64),
        NbtValue::Short(value) => Some(*value as i64),
        NbtValue::Int(value) => Some(*value as i64),
        NbtValue::Long(value) => Some(*value),
        _ => None,
    }
}

/// 取 NBT f32 值。
///
/// Reads an NBT f32 value.
///
/// - `value`: 可选的 NBT 值
fn number_f32(value: Option<&NbtValue>) -> Option<f32> {
    number_f32_value(value?)
}

/// 取 NBT f32 值（含 Double 降级）。
///
/// Reads an NBT value as f32 (Double is demoted).
///
/// - `value`: NBT 值
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
