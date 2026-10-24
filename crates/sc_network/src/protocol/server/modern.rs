//! 2168 登录序列使用的现代客户端配置包。
//!
//! Modern client configuration packets used by the 2168 login sequence.
//!
//! 包值保持为语义化的 Rust 数据；这里不存储捕获的原始字节。

use std::io::{Error, ErrorKind};

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::{NbtValue, SCNBTByteWriter};
use sc_network_macros::MinecraftPacket;

/// SyncActorPropertyPacket (0xa5)：同步实体属性 NBT。
///
/// SyncActorPropertyPacket (0xa5): synchronizes entity property NBT.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SyncActorProperty {
    /// 实体属性 NBT 数据。
    ///
    /// Entity property NBT data.
    pub property_data: CompoundNbt,
}

/// VoxelShapesPacket (0x151)：体素形状注册表。
///
/// VoxelShapesPacket (0x151): voxel shape registry.
///
/// 基础布局：shapes 数组（uvarint 计数；每项 cells{u8 xyz 尺寸 +
/// 存储数组} + x/y/z 坐标数组）+ nameMap（uvarint 计数 + string/sLE 对）；
/// 新版本追加 customShapeCount（sLE）。空注册表编码为 4 字节。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct VoxelShapes {
    /// 命名的体素形状表（名称 → 注册表句柄）。恒为空（无自定义形状）。
    ///
    /// Named voxel shape table (name to registry handle). Always empty (no
    /// custom shapes).
    pub shapes: Vec<(String, u16)>,
    /// 新版本：自定义形状计数。
    ///
    /// Newer versions: custom shape count.
    pub custom_shape_count: u16,
}

impl VoxelShapes {
    /// 构造空注册表。
    ///
    /// Builds an empty registry.
    pub fn empty() -> Self {
        Self {
            shapes: Vec::new(),
            custom_shape_count: 0,
        }
    }
}

impl Default for VoxelShapes {
    /// 默认实现等价于 [`VoxelShapes::empty`]。
    ///
    /// The default implementation is equivalent to [`VoxelShapes::empty`].
    fn default() -> Self {
        Self::empty()
    }
}

impl Writer for VoxelShapes {
    /// 写入 VoxelShapes 包体。
    ///
    /// Writes the VoxelShapes packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(0)?; // shapes (built-in list, empty)
        buf.write_var_u32(self.shapes.len() as u32)?; // nameMap count
        for (name, handle) in &self.shapes {
            buf.write_string(name)?;
            buf.write_u16_le(*handle)?;
        }
        buf.write_u16_le(self.custom_shape_count) // customShapeCount (sLE)
    }
}

impl Reader<VoxelShapes> for VoxelShapes {
    /// 不支持解码：服务端只发送体素形状注册表。
    ///
    /// Decoding is unsupported: the server only sends the voxel shape
    /// registry.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "VoxelShapes decode is unsupported",
        ))
    }
}

impl SyncActorProperty {
    /// 构造空属性包。
    ///
    /// Builds an empty property packet.
    pub fn empty() -> Self {
        Self {
            property_data: CompoundNbt::new(None),
        }
    }
}

impl Default for SyncActorProperty {
    /// 默认实现等价于 [`SyncActorProperty::empty`]。
    ///
    /// The default implementation is equivalent to
    /// [`SyncActorProperty::empty`].
    fn default() -> Self {
        Self::empty()
    }
}

impl Writer for SyncActorProperty {
    /// 以 Bedrock 网络 NBT 格式写入属性数据。
    ///
    /// Writes the property data using the Bedrock network NBT format.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// NBT 序列化失败时返回错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_nbt::<BedrockNetworkNbt>(&NbtValue::Compound(self.property_data.clone()))
    }
}

impl Reader<SyncActorProperty> for SyncActorProperty {
    /// 不支持解码：服务端只发送实体属性。
    ///
    /// Decoding is unsupported: the server only sends entity properties.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "SyncActorProperty decode is unsupported",
        ))
    }
}

/// PlayerFogPacket (0xa0)：玩家雾效栈。
///
/// PlayerFogPacket (0xa0): player fog stack.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct PlayerFog {
    /// 雾效标识字符串栈。
    ///
    /// Fog identifier string stack.
    pub fog_stack: Vec<String>,
}

impl PlayerFog {
    /// 构造空雾效栈。
    ///
    /// Builds an empty fog stack.
    pub fn empty() -> Self {
        Self {
            fog_stack: Vec::new(),
        }
    }
}

impl Default for PlayerFog {
    /// 默认实现等价于 [`PlayerFog::empty`]。
    ///
    /// The default implementation is equivalent to [`PlayerFog::empty`].
    fn default() -> Self {
        Self::empty()
    }
}

impl Writer for PlayerFog {
    /// 写入 PlayerFog 包体。
    ///
    /// Writes the PlayerFog packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.fog_stack.len() as u32)?;
        for fog in &self.fog_stack {
            buf.write_string(fog)?;
        }
        Ok(())
    }
}

impl Reader<PlayerFog> for PlayerFog {
    /// 不支持解码：服务端只发送雾效栈。
    ///
    /// Decoding is unsupported: the server only sends the fog stack.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "PlayerFog decode is unsupported",
        ))
    }
}

/// TrimDataPacket (0x12e)：锻造纹样与材质数据。
///
/// TrimDataPacket (0x12e): smithing trim pattern and material data.
#[derive(Clone, Debug, Default, MinecraftPacket)]
pub struct TrimData {
    /// 纹样列表。
    ///
    /// Trim patterns.
    pub patterns: Vec<TrimPattern>,
    /// 材质列表。
    ///
    /// Trim materials.
    pub materials: Vec<TrimMaterial>,
}

/// TrimPattern：锻造纹样条目。
///
/// TrimPattern: a smithing trim pattern entry.
#[derive(Clone, Debug)]
pub struct TrimPattern {
    /// 纹样物品名。
    ///
    /// Pattern item name.
    pub item_name: String,
    /// 纹样 id。
    ///
    /// Pattern id.
    pub pattern_id: String,
}

/// TrimMaterial：锻造材质条目。
///
/// TrimMaterial: a smithing trim material entry.
#[derive(Clone, Debug)]
pub struct TrimMaterial {
    /// 材质 id。
    ///
    /// Material id.
    pub material_id: String,
    /// 调色板颜色。
    ///
    /// Palette color.
    pub color: String,
    /// 材质物品名。
    ///
    /// Material item name.
    pub item_name: String,
}

impl Writer for TrimData {
    /// 写入 TrimData 包体。
    ///
    /// Writes the TrimData packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.patterns.len() as u32)?;
        for pattern in &self.patterns {
            buf.write_string(&pattern.item_name)?;
            buf.write_string(&pattern.pattern_id)?;
        }

        buf.write_var_u32(self.materials.len() as u32)?;
        for material in &self.materials {
            buf.write_string(&material.material_id)?;
            buf.write_string(&material.color)?;
            buf.write_string(&material.item_name)?;
        }
        Ok(())
    }
}

impl Reader<TrimData> for TrimData {
    /// 不支持解码：服务端只发送锻造数据。
    ///
    /// Decoding is unsupported: the server only sends trim data.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "TrimData decode is unsupported",
        ))
    }
}

/// CameraPresetsPacket (0xc6)，结构化格式。
///
/// CameraPresetsPacket (0xc6), structured format.
///
/// 取代旧 NBT 形式：`uvarint 计数 + 每个预设的结构体`
/// （名称/父级字符串加一串选项字段，每项带 1 字节存在性标志）。
#[derive(Clone, Debug, Default, MinecraftPacket)]
pub struct CameraPresets {
    /// 摄像机预设列表。
    ///
    /// Camera presets.
    pub presets: Vec<CameraPreset>,
}

/// CameraPreset：单个摄像机预设定义。
///
/// CameraPreset: a single camera preset definition.
#[derive(Clone, Debug)]
pub struct CameraPreset {
    /// 预设标识符。
    ///
    /// Preset identifier.
    pub identifier: String,
    /// 继承的父预设。
    ///
    /// Parent preset to inherit from.
    pub inherit_from: String,
    /// 位置（三个独立 option lf32）。
    ///
    /// Position (three independent option lf32).
    pub position: Option<[f32; 3]>,
    /// 俯仰角（rotation.x，rot_x = pitch）。
    ///
    /// Pitch (rotation.x, rot_x = pitch).
    pub pitch: Option<f32>,
    /// 偏航角（rotation.y，rot_y = yaw）。
    ///
    /// Yaw (rotation.y, rot_y = yaw).
    pub yaw: Option<f32>,
    /// 视角偏移（option vec2f）。
    ///
    /// View offset (option vec2f).
    pub offset: Option<[f32; 2]>,
    /// 实体偏移（option vec3f）。
    ///
    /// Entity offset (option vec3f).
    pub entity_offset: Option<[f32; 3]>,
    /// 跟踪半径（option lf32）。
    ///
    /// Tracking radius (option lf32).
    pub radius: Option<f32>,
}

impl CameraPreset {
    /// 用标识符构造预设（其余字段留空）。
    ///
    /// Builds a preset from an identifier (all other fields empty).
    ///
    /// - `identifier`: 预设标识符
    pub fn new(identifier: impl Into<String>) -> Self {
        Self {
            identifier: identifier.into(),
            inherit_from: String::new(),
            position: None,
            pitch: None,
            yaw: None,
            offset: None,
            entity_offset: None,
            radius: None,
        }
    }
}

impl CameraPresets {
    /// 六个原版内置摄像机预设。
    ///
    /// Six built-in vanilla camera presets.
    pub fn vanilla() -> Self {
        let first_person = CameraPreset::new("minecraft:first_person");
        let mut fixed_boom = CameraPreset::new("minecraft:fixed_boom");
        fixed_boom.offset = Some([0.0, 0.0]);
        fixed_boom.entity_offset = Some([0.0, 0.0, 0.0]);
        let mut follow_orbit = CameraPreset::new("minecraft:follow_orbit");
        follow_orbit.offset = Some([0.0, 0.0]);
        follow_orbit.entity_offset = Some([0.0, 0.0, 0.0]);
        follow_orbit.radius = Some(10.0);
        let mut free = CameraPreset::new("minecraft:free");
        free.position = Some([0.0, 0.0, 0.0]);
        free.pitch = Some(0.0);
        free.yaw = Some(0.0);
        let third_person = CameraPreset::new("minecraft:third_person");
        let third_person_front = CameraPreset::new("minecraft:third_person_front");
        Self {
            presets: vec![
                first_person,
                fixed_boom,
                follow_orbit,
                free,
                third_person,
                third_person_front,
            ],
        }
    }
}

/// Option lf32：存在性标志（bool）+ 小端 f32。
///
/// Option lf32: presence flag (bool) + LE f32.
///
/// - `buf`: 输出字节写入器
/// - `value`: 可选值
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_opt_f32(buf: &mut ByteWriter, value: Option<f32>) -> Result<(), Error> {
    match value {
        Some(v) => {
            buf.write_u8(1)?;
            buf.write_f32_le(v)
        }
        None => buf.write_u8(0),
    }
}

/// Option bool：存在性标志 + bool。
///
/// Option bool: presence flag + bool.
///
/// - `buf`: 输出字节写入器
/// - `value`: 可选值
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_opt_bool(buf: &mut ByteWriter, value: Option<bool>) -> Result<(), Error> {
    match value {
        Some(v) => {
            buf.write_u8(1)?;
            buf.write_u8(v as u8)
        }
        None => buf.write_u8(0),
    }
}

/// Option vec2f：存在性标志 + 2 个小端 f32。
///
/// Option vec2f: presence flag + 2 LE f32.
///
/// - `buf`: 输出字节写入器
/// - `value`: 可选值
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_opt_vec2f(buf: &mut ByteWriter, value: Option<[f32; 2]>) -> Result<(), Error> {
    match value {
        Some([x, y]) => {
            buf.write_u8(1)?;
            buf.write_f32_le(x)?;
            buf.write_f32_le(y)
        }
        None => buf.write_u8(0),
    }
}

/// Option vec3f：存在性标志 + 3 个小端 f32。
///
/// Option vec3f: presence flag + 3 LE f32.
///
/// - `buf`: 输出字节写入器
/// - `value`: 可选值
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_opt_vec3f(buf: &mut ByteWriter, value: Option<[f32; 3]>) -> Result<(), Error> {
    match value {
        Some([x, y, z]) => {
            buf.write_u8(1)?;
            buf.write_f32_le(x)?;
            buf.write_f32_le(y)?;
            buf.write_f32_le(z)
        }
        None => buf.write_u8(0),
    }
}

impl Writer for CameraPresets {
    /// 写入 CameraPresets 包体（逐预设的结构化字段）。
    ///
    /// Writes the CameraPresets body (structured fields per preset).
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.presets.len() as u32)?;
        for preset in &self.presets {
            buf.write_string(&preset.identifier)?; // name
            buf.write_string(&preset.inherit_from)?; // parent
                                                     // Position (x/y/z, each an option lf32).
            write_opt_f32(buf, preset.position.map(|p| p[0]))?;
            write_opt_f32(buf, preset.position.map(|p| p[1]))?;
            write_opt_f32(buf, preset.position.map(|p| p[2]))?;
            // rotation:Vec2fopts(x=pitch, y=yaw).
            write_opt_f32(buf, preset.pitch)?;
            write_opt_f32(buf, preset.yaw)?;
            write_opt_f32(buf, None)?; // rotation_speed
            write_opt_bool(buf, None)?; // snap_to_target
            write_opt_vec2f(buf, None)?; // horizontal_rotation_limit
            write_opt_vec2f(buf, None)?; // vertical_rotation_limit
            write_opt_bool(buf, None)?; // continue_targeting
            write_opt_f32(buf, None)?; // tracking_radius
            write_opt_vec2f(buf, preset.offset)?; // offset (view offset)
            write_opt_vec3f(buf, preset.entity_offset)?; // entity_offset
            write_opt_f32(buf, preset.radius)?; // radius
            write_opt_f32(buf, None)?; // yaw_limit_min
            write_opt_f32(buf, None)?; // yaw_limit_max
                                       // audio_listener(option u8).
            buf.write_u8(0)?;
            // player_effects(option bool).
            buf.write_u8(0)?;
            // aim_assist (option container): absent.
            buf.write_u8(0)?;
            // control_scheme (option u8 mapper): absent.
            buf.write_u8(0)?;
            if crate::protocol::version::protocol_at_least(
                crate::protocol::version::PROTOCOL_VERSION_1_26_60,
            ) {
                // apply_inherited_starting_rotation + starting rotation Vec2.
                buf.write_bool(false)?;
                buf.write_f32_le(0.0)?;
                buf.write_f32_le(0.0)?;
            }
        }
        Ok(())
    }
}

impl Reader<CameraPresets> for CameraPresets {
    /// 不支持解码：服务端只发送摄像机预设。
    ///
    /// Decoding is unsupported: the server only sends camera presets.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "CameraPresets decode is unsupported",
        ))
    }
}

/// CameraAimAssistPresetsPacket (0x140)：摄像机瞄准辅助预设。
///
/// CameraAimAssistPresetsPacket (0x140): camera aim assist presets.
///
/// 2168 序列化器依次写类别定义、预设定义，最后是 1 字节操作码。
/// 语义定义类型在此刻意保持精简；有注册表时可从中填充，而空的 SET
/// 包对原版世界仍然有效。
#[derive(Clone, Debug, Default, MinecraftPacket)]
pub struct CameraAimAssistPresets {
    /// 瞄准辅助类别定义。
    ///
    /// Aim assist category definitions.
    pub categories: Vec<CameraAimAssistCategory>,
    /// 瞄准辅助预设定义。
    ///
    /// Aim assist preset definitions.
    pub presets: Vec<CameraAimAssistPreset>,
    /// 操作类型。
    ///
    /// Operation type.
    pub operation: CameraAimAssistOperation,
}

/// CameraAimAssistCategory：瞄准辅助类别（实体/方块优先级表）。
///
/// CameraAimAssistCategory: aim assist category (entity/block priority
/// tables).
#[derive(Clone, Debug, Default)]
pub struct CameraAimAssistCategory {
    /// 类别名称。
    ///
    /// Category name.
    pub name: String,
    /// 实体优先级列表。
    ///
    /// Entity priorities.
    pub entities: Vec<CameraAimAssistPriority>,
    /// 方块优先级列表。
    ///
    /// Block priorities.
    pub blocks: Vec<CameraAimAssistPriority>,
    /// 实体默认优先级。
    ///
    /// Default entity priority.
    pub entity_default: Option<i32>,
    /// 方块默认优先级。
    ///
    /// Default block priority.
    pub block_default: Option<i32>,
}

/// CameraAimAssistPriority：标识符 + 优先级。
///
/// CameraAimAssistPriority: identifier + priority.
#[derive(Clone, Debug)]
pub struct CameraAimAssistPriority {
    /// 实体/方块标识符。
    ///
    /// Entity/block identifier.
    pub identifier: String,
    /// 优先级数值。
    ///
    /// Priority value.
    pub priority: i32,
}

/// CameraAimAssistPreset：瞄准辅助预设。
///
/// CameraAimAssistPreset: an aim assist preset.
#[derive(Clone, Debug, Default)]
pub struct CameraAimAssistPreset {
    /// 预设标识符。
    ///
    /// Preset identifier.
    pub identifier: String,
    /// 类别引用。
    ///
    /// Category reference.
    pub categories: String,
    /// 排除列表。
    ///
    /// Exclusion list.
    pub exclusions: Vec<String>,
    /// 液体目标列表。
    ///
    /// Liquid targeting list.
    pub liquid_targeting: Vec<String>,
    /// 物品设置列表。
    ///
    /// Item settings.
    pub item_settings: Vec<CameraAimAssistItemSetting>,
    /// 默认物品设置引用。
    ///
    /// Default item settings reference.
    pub default_item_settings: Option<String>,
    /// 手持设置引用。
    ///
    /// Hand settings reference.
    pub hand_settings: Option<String>,
}

/// CameraAimAssistItemSetting：物品 → 类别映射。
///
/// CameraAimAssistItemSetting: item -> category mapping.
#[derive(Clone, Debug)]
pub struct CameraAimAssistItemSetting {
    /// 物品 id。
    ///
    /// Item id.
    pub item_id: String,
    /// 类别名。
    ///
    /// Category name.
    pub category: String,
}

/// CameraAimAssistOperation：瞄准辅助预设操作类型。
///
/// CameraAimAssistOperation: aim assist preset operation type.
#[derive(Clone, Copy, Debug, Default)]
#[repr(u8)]
pub enum CameraAimAssistOperation {
    /// 覆盖设置。
    ///
    /// Replace the settings.
    #[default]
    Set = 0,
    /// 追加到现有设置。
    ///
    /// Add to the existing settings.
    AddToExisting = 1,
}

impl Writer for CameraAimAssistPresets {
    /// 写入 CameraAimAssistPresets 包体。
    ///
    /// Writes the CameraAimAssistPresets packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.categories.len() as u32)?;
        for category in &self.categories {
            buf.write_string(&category.name)?;
            write_priorities(buf, &category.entities)?;
            write_priorities(buf, &category.blocks)?;
            write_optional_i32(buf, category.entity_default)?;
            write_optional_i32(buf, category.block_default)?;
        }

        buf.write_var_u32(self.presets.len() as u32)?;
        for preset in &self.presets {
            buf.write_string(&preset.identifier)?;
            buf.write_string(&preset.categories)?;
            write_strings(buf, &preset.exclusions)?;
            write_strings(buf, &preset.liquid_targeting)?;
            buf.write_var_u32(preset.item_settings.len() as u32)?;
            for setting in &preset.item_settings {
                buf.write_string(&setting.item_id)?;
                buf.write_string(&setting.category)?;
            }
            write_optional_string(buf, preset.default_item_settings.as_deref())?;
            write_optional_string(buf, preset.hand_settings.as_deref())?;
        }

        buf.write_u8(self.operation as u8)?;
        Ok(())
    }
}

impl Reader<CameraAimAssistPresets> for CameraAimAssistPresets {
    /// 不支持解码：服务端只发送瞄准辅助预设。
    ///
    /// Decoding is unsupported: the server only sends aim assist presets.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "CameraAimAssistPresets decode is unsupported",
        ))
    }
}

/// 写入瞄准辅助优先级数组（标识符 + i32）。
///
/// Writes the aim assist priority array (identifier + i32).
///
/// - `buf`: 输出字节写入器
/// - `values`: 优先级列表
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_priorities(buf: &mut ByteWriter, values: &[CameraAimAssistPriority]) -> Result<(), Error> {
    buf.write_var_u32(values.len() as u32)?;
    for value in values {
        buf.write_string(&value.identifier)?;
        buf.write_i32_le(value.priority)?;
    }
    Ok(())
}

/// 写入字符串数组（var_u32 长度 + 逐个字符串）。
///
/// Writes a string array (var_u32 length followed by each string).
///
/// - `buf`: 输出字节写入器
/// - `values`: 字符串列表
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_strings(buf: &mut ByteWriter, values: &[String]) -> Result<(), Error> {
    buf.write_var_u32(values.len() as u32)?;
    for value in values {
        buf.write_string(value)?;
    }
    Ok(())
}

/// 写入可选 i32（bool 存在性标志 + 小端 i32）。
///
/// Writes an optional i32 (bool presence flag + LE i32).
///
/// - `buf`: 输出字节写入器
/// - `value`: 可选值
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_optional_i32(buf: &mut ByteWriter, value: Option<i32>) -> Result<(), Error> {
    buf.write_bool(value.is_some())?;
    if let Some(value) = value {
        buf.write_i32_le(value)?;
    }
    Ok(())
}

/// 写入可选字符串（bool 存在性标志 + 字符串）。
///
/// Writes an optional string (bool presence flag + string).
///
/// - `buf`: 输出字节写入器
/// - `value`: 可选值
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_optional_string(buf: &mut ByteWriter, value: Option<&str>) -> Result<(), Error> {
    buf.write_bool(value.is_some())?;
    if let Some(value) = value {
        buf.write_string(value)?;
    }
    Ok(())
}

// Retained NBT helper; reserved for future wire encoders.
#[allow(dead_code)]
/// 以长度前缀方式写入字符串字节（保留的 NBT 辅助函数）。
///
/// Writes string bytes with a length prefix (retained NBT helper).
///
/// - `buf`: 输出字节写入器
/// - `value`: 字符串
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_nbt_string(buf: &mut ByteWriter, value: &str) -> Result<(), Error> {
    buf.write_var_u32(value.len() as u32)?;
    buf.write(value.as_bytes())
}

/// SetPlayerFurnaceOptions 的熔炉类型（uint8）。
///
/// Furnace type for SetPlayerFurnaceOptions (uint8).
// Casing mirrors the wire-protocol names.
#[allow(non_snake_case)]
// Retained wire-protocol constants; not all variants are read yet.
#[allow(dead_code)]
pub mod FurnaceType {
    /// 无。
    ///
    /// None.
    pub const NONE: u8 = 0;
    /// 熔炉。
    ///
    /// Furnace.
    pub const FURNACE: u8 = 1;
    /// 高炉。
    ///
    /// Blast furnace.
    pub const BLAST_FURNACE: u8 = 2;
    /// 烟熏炉。
    ///
    /// Smoker.
    pub const SMOKER: u8 = 3;
}

/// 熔炉选项左侧标签页索引（varint）。
///
/// Left recipe tab index for furnace options (varint).
// Casing mirrors the wire-protocol names.
#[allow(non_snake_case)]
// Retained wire-protocol constants; not all variants are read yet.
#[allow(dead_code)]
pub mod FurnaceLeftTab {
    /// 无。
    ///
    /// None.
    pub const NONE: i32 = 0;
    /// 食物配方页。
    ///
    /// Food recipe tab.
    pub const RECIPE_FOOD: i32 = 1;
    /// 物品配方页。
    ///
    /// Item recipe tab.
    pub const RECIPE_ITEMS: i32 = 2;
    /// 方块配方页。
    ///
    /// Block recipe tab.
    pub const RECIPE_BLOCKS: i32 = 3;
    /// 搜索页。
    ///
    /// Search tab.
    pub const RECIPE_SEARCH: i32 = 4;
    /// 物品栏页。
    ///
    /// Inventory tab.
    pub const INVENTORY: i32 = 5;
}

/// 熔炉选项界面布局（varint）。
///
/// Furnace layout for furnace options (varint).
// Casing mirrors the wire-protocol names.
#[allow(non_snake_case)]
// Retained wire-protocol constants; not all variants are read yet.
#[allow(dead_code)]
pub mod FurnaceLayout {
    /// 无。
    ///
    /// None.
    pub const NONE: i32 = 0;
    /// 仅物品栏。
    ///
    /// Inventory only.
    pub const INVENTORY_ONLY: i32 = 1;
    /// 默认布局。
    ///
    /// Default layout.
    pub const DEFAULT: i32 = 2;
}

/// FurnaceOptions：熔炉 UI 选项块。
///
/// FurnaceOptions: furnace UI options block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FurnaceOptions {
    /// 左侧标签页索引，见 [`FurnaceLeftTab`]。
    ///
    /// Left tab index, see [`FurnaceLeftTab`].
    pub left_tab: i32,
    /// 是否启用过滤。
    ///
    /// Whether filtering is enabled.
    pub filtering: bool,
    /// 界面布局，见 [`FurnaceLayout`]。
    ///
    /// Layout, see [`FurnaceLayout`].
    pub layout: i32,
}

/// 写入熔炉选项块。
///
/// Writes the furnace options block.
///
/// - `buf`: 输出字节写入器
/// - `options`: 熔炉选项
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_furnace_options(buf: &mut ByteWriter, options: &FurnaceOptions) -> Result<(), Error> {
    buf.write_var_i32(options.left_tab)?;
    buf.write_bool(options.filtering)?;
    buf.write_var_i32(options.layout)
}

/// 解码熔炉选项块。
///
/// Decodes the furnace options block.
///
/// - `buf`: 输入字节读取器
///
/// # Errors
/// 缓冲区不足时返回解码错误。
fn read_furnace_options(buf: &mut ByteReader) -> Result<FurnaceOptions, Error> {
    Ok(FurnaceOptions {
        left_tab: buf.read_var_i32()?,
        filtering: buf.read_bool()?,
        layout: buf.read_var_i32()?,
    })
}

/// SetPlayerFurnaceOptionsPacket (0x15f)：设置玩家熔炉界面选项。
///
/// SetPlayerFurnaceOptionsPacket (0x15f): sets the player furnace UI
/// options.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetPlayerFurnaceOptions {
    /// 熔炉类型，见 [`FurnaceType`]。
    ///
    /// Furnace type, see [`FurnaceType`].
    pub furnace_type: u8,
    /// 界面选项。
    ///
    /// UI options.
    pub options: FurnaceOptions,
}

impl Reader<SetPlayerFurnaceOptions> for SetPlayerFurnaceOptions {
    /// 解码 SetPlayerFurnaceOptions 包体。
    ///
    /// Decodes a SetPlayerFurnaceOptions body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            furnace_type: buf.read_u8()?,
            options: read_furnace_options(buf)?,
        })
    }
}

impl Writer for SetPlayerFurnaceOptions {
    /// 写入 SetPlayerFurnaceOptions 包体。
    ///
    /// Writes the SetPlayerFurnaceOptions body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.furnace_type)?;
        write_furnace_options(buf, &self.options)
    }
}

/// RecordStartedPacket (0x160)：唱片机播放通知。
///
/// RecordStartedPacket (0x160): jukebox playback notification.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct RecordStarted {
    /// 唱片机 X 坐标。
    ///
    /// Jukebox X coordinate.
    pub x: i32,
    /// 唱片机 Y 坐标。
    ///
    /// Jukebox Y coordinate.
    pub y: i32,
    /// 唱片机 Z 坐标。
    ///
    /// Jukebox Z coordinate.
    pub z: i32,
    /// 服务端声音句柄。
    ///
    /// Server sound handle.
    pub server_sound_handle: u64,
}

impl Reader<RecordStarted> for RecordStarted {
    /// 解码 RecordStarted 包体。
    ///
    /// Decodes a RecordStarted body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            x: buf.read_var_i32()?,
            y: buf.read_var_i32()?,
            z: buf.read_var_i32()?,
            server_sound_handle: buf.read_var_u64()?,
        })
    }
}

impl Writer for RecordStarted {
    /// 写入 RecordStarted 包体。
    ///
    /// Writes the RecordStarted body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.x)?;
        buf.write_var_i32(self.y)?;
        buf.write_var_i32(self.z)?;
        buf.write_var_u64(self.server_sound_handle)
    }
}

/// ClientboundStonecutterSetRecipePacket (0x163)：切石机配方索引回显。
///
/// ClientboundStonecutterSetRecipePacket (0x163): stonecutter recipe index
/// echo.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ClientboundStonecutterSetRecipe {
    /// 玩家 id。
    ///
    /// Player id.
    pub player_id: i64,
    /// 容器 id。
    ///
    /// Container id.
    pub container_id: u8,
    /// 选中的配方索引。
    ///
    /// Selected recipe index.
    pub recipe_index: i32,
}

impl Reader<ClientboundStonecutterSetRecipe> for ClientboundStonecutterSetRecipe {
    /// 解码 ClientboundStonecutterSetRecipe 包体。
    ///
    /// Decodes a ClientboundStonecutterSetRecipe body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            player_id: buf.read_var_i64()?,
            container_id: buf.read_u8()?,
            recipe_index: buf.read_var_i32()?,
        })
    }
}

impl Writer for ClientboundStonecutterSetRecipe {
    /// 写入 ClientboundStonecutterSetRecipe 包体。
    ///
    /// Writes the ClientboundStonecutterSetRecipe body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i64(self.player_id)?;
        buf.write_u8(self.container_id)?;
        buf.write_var_i32(self.recipe_index)
    }
}

/// 匹配状态取值（uint8）。
///
/// Matchmaking state values (uint8).
// Casing mirrors the wire-protocol names.
#[allow(non_snake_case)]
// Retained wire-protocol constants; not all variants are read yet.
#[allow(dead_code)]
pub mod MatchmakingState {
    /// 空闲。
    ///
    /// Idle.
    pub const IDLE: u8 = 0;
}

/// ClientboundMatchmakingStatePacket (0x161)：匹配状态通知。
///
/// ClientboundMatchmakingStatePacket (0x161): matchmaking state
/// notification.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ClientboundMatchmakingState {
    /// 匹配状态，见 [`MatchmakingState`]。
    ///
    /// Matchmaking state, see [`MatchmakingState`].
    pub state: u8,
    /// 目标名称。
    ///
    /// Destination name.
    pub destination_name: String,
    /// 触发匹配的玩家名。
    ///
    /// Name of the player who triggered the match.
    pub triggering_player_name: String,
    /// 是否由本地玩家触发。
    ///
    /// Whether the local player triggered it.
    pub triggered_by_local_player: bool,
}

impl Reader<ClientboundMatchmakingState> for ClientboundMatchmakingState {
    /// 解码 ClientboundMatchmakingState 包体。
    ///
    /// Decodes a ClientboundMatchmakingState body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            state: buf.read_u8()?,
            destination_name: buf.read_string()?,
            triggering_player_name: buf.read_string()?,
            triggered_by_local_player: buf.read_bool()?,
        })
    }
}

impl Writer for ClientboundMatchmakingState {
    /// 写入 ClientboundMatchmakingState 包体。
    ///
    /// Writes the ClientboundMatchmakingState body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.state)?;
        buf.write_string(&self.destination_name)?;
        buf.write_string(&self.triggering_player_name)?;
        buf.write_bool(self.triggered_by_local_player)
    }
}

/// SignedAudioContent：签名音频内容引用（紧凑 JWT 字符串）。
///
/// SignedAudioContent: signed audio content reference (compact JWT
/// string).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedAudioContent {
    /// 紧凑 JWT 字符串。
    ///
    /// Compact JWT string.
    pub compact_jwt: String,
}

/// 写入签名音频内容引用。
///
/// Writes a signed audio content reference.
///
/// - `buf`: 输出字节写入器
/// - `content`: 音频内容引用
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_signed_audio_content(
    buf: &mut ByteWriter,
    content: &SignedAudioContent,
) -> Result<(), Error> {
    buf.write_string(&content.compact_jwt)
}

/// 解码签名音频内容引用。
///
/// Decodes a signed audio content reference.
///
/// - `buf`: 输入字节读取器
///
/// # Errors
/// 缓冲区不足时返回解码错误。
fn read_signed_audio_content(buf: &mut ByteReader) -> Result<SignedAudioContent, Error> {
    Ok(SignedAudioContent {
        compact_jwt: buf.read_string()?,
    })
}

/// ClientboundPlayAudioContentPacket (0x167)：服务端驱动的音频播放。
///
/// ClientboundPlayAudioContentPacket (0x167): server-driven audio
/// playback.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ClientboundPlayAudioContent {
    /// 共享元数据（签名引用）。
    ///
    /// Shared metadata (signed reference).
    pub shared_metadata: SignedAudioContent,
    /// 播放内容（签名引用）。
    ///
    /// Playback content (signed reference).
    pub playback_content: SignedAudioContent,
    /// 播放类型。
    ///
    /// Playback type.
    pub playback_type: u8,
    /// 声音名称。
    ///
    /// Sound name.
    pub sound_name: String,
    /// 播放位置 X 坐标。
    ///
    /// Playback X coordinate.
    pub x: i32,
    /// 播放位置 Y 坐标。
    ///
    /// Playback Y coordinate.
    pub y: i32,
    /// 播放位置 Z 坐标。
    ///
    /// Playback Z coordinate.
    pub z: i32,
    /// 音量。
    ///
    /// Volume.
    pub volume: f32,
    /// 音调。
    ///
    /// Pitch.
    pub pitch: f32,
    /// 循环次数。
    ///
    /// Loop count.
    pub loop_count: i32,
    /// 是否跳过听者距离检查。
    ///
    /// Whether to bypass the listener range check.
    pub bypass_listener_range_check: bool,
    /// 服务端声音句柄。
    ///
    /// Server sound handle.
    pub server_sound_handle: u64,
    /// 播放起始位置（秒）。
    ///
    /// Playback start position in seconds.
    pub playback_position_seconds: f32,
}

impl Reader<ClientboundPlayAudioContent> for ClientboundPlayAudioContent {
    /// 解码 ClientboundPlayAudioContent 包体。
    ///
    /// Decodes a ClientboundPlayAudioContent body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            shared_metadata: read_signed_audio_content(buf)?,
            playback_content: read_signed_audio_content(buf)?,
            playback_type: buf.read_u8()?,
            sound_name: buf.read_string()?,
            x: buf.read_var_i32()?,
            y: buf.read_var_i32()?,
            z: buf.read_var_i32()?,
            volume: buf.read_f32_le()?,
            pitch: buf.read_f32_le()?,
            loop_count: buf.read_var_i32()?,
            bypass_listener_range_check: buf.read_bool()?,
            server_sound_handle: buf.read_var_u64()?,
            playback_position_seconds: buf.read_f32_le()?,
        })
    }
}

impl Writer for ClientboundPlayAudioContent {
    /// 写入 ClientboundPlayAudioContent 包体。
    ///
    /// Writes the ClientboundPlayAudioContent body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        write_signed_audio_content(buf, &self.shared_metadata)?;
        write_signed_audio_content(buf, &self.playback_content)?;
        buf.write_u8(self.playback_type)?;
        buf.write_string(&self.sound_name)?;
        buf.write_var_i32(self.x)?;
        buf.write_var_i32(self.y)?;
        buf.write_var_i32(self.z)?;
        buf.write_f32_le(self.volume)?;
        buf.write_f32_le(self.pitch)?;
        buf.write_var_i32(self.loop_count)?;
        buf.write_bool(self.bypass_listener_range_check)?;
        buf.write_var_u64(self.server_sound_handle)?;
        buf.write_f32_le(self.playback_position_seconds)
    }
}

/// 方块乘客表情取值（uint8）。
///
/// Block passenger emote values (uint8).
// Casing mirrors the wire-protocol names.
#[allow(non_snake_case)]
// Retained wire-protocol constants; not all variants are read yet.
#[allow(dead_code)]
pub mod PassengerEmote {
    /// 站立。
    ///
    /// Standing.
    pub const STANDING: u8 = 0;
    /// 骑乘中。
    ///
    /// Riding.
    pub const RIDING: u8 = 1;
    /// 躺卧。
    ///
    /// Laying.
    pub const LAYING: u8 = 2;
}

/// SetPassengerOfBlockPacket (0x165)：方块乘客分配。
///
/// SetPassengerOfBlockPacket (0x165): block passenger assignment.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetPassengerOfBlock {
    /// 乘客实体 id。
    ///
    /// Passenger entity id.
    pub passenger_id: i64,
    /// 方块 X 坐标。
    ///
    /// Block X coordinate.
    pub x: i32,
    /// 方块 Y 坐标。
    ///
    /// Block Y coordinate.
    pub y: i32,
    /// 方块 Z 坐标。
    ///
    /// Block Z coordinate.
    pub z: i32,
    /// 乘客偏移 X 分量。
    ///
    /// Passenger offset X component.
    pub offset_x: f32,
    /// 乘客偏移 Y 分量。
    ///
    /// Passenger offset Y component.
    pub offset_y: f32,
    /// 乘客偏移 Z 分量。
    ///
    /// Passenger offset Z component.
    pub offset_z: f32,
    /// 旋转角。
    ///
    /// Rotation.
    pub rotation: f32,
    /// 旋转角上限。
    ///
    /// Rotation limit.
    pub rotation_limit: f32,
    /// 表情类型，见 [`PassengerEmote`]。
    ///
    /// Emote type, see [`PassengerEmote`].
    pub emote: u8,
}

impl Reader<SetPassengerOfBlock> for SetPassengerOfBlock {
    /// 解码 SetPassengerOfBlock 包体。
    ///
    /// Decodes a SetPassengerOfBlock body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            passenger_id: buf.read_var_i64()?,
            x: buf.read_var_i32()?,
            y: buf.read_var_i32()?,
            z: buf.read_var_i32()?,
            offset_x: buf.read_f32_le()?,
            offset_y: buf.read_f32_le()?,
            offset_z: buf.read_f32_le()?,
            rotation: buf.read_f32_le()?,
            rotation_limit: buf.read_f32_le()?,
            emote: buf.read_u8()?,
        })
    }
}

impl Writer for SetPassengerOfBlock {
    /// 写入 SetPassengerOfBlock 包体。
    ///
    /// Writes the SetPassengerOfBlock body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i64(self.passenger_id)?;
        buf.write_var_i32(self.x)?;
        buf.write_var_i32(self.y)?;
        buf.write_var_i32(self.z)?;
        buf.write_f32_le(self.offset_x)?;
        buf.write_f32_le(self.offset_y)?;
        buf.write_f32_le(self.offset_z)?;
        buf.write_f32_le(self.rotation)?;
        buf.write_f32_le(self.rotation_limit)?;
        buf.write_u8(self.emote)
    }
}

#[cfg(test)]
mod protocol_2225_tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};

    #[test]
    fn furnace_options_round_trip() {
        let packet = SetPlayerFurnaceOptions {
            furnace_type: FurnaceType::FURNACE,
            options: FurnaceOptions {
                left_tab: FurnaceLeftTab::INVENTORY,
                filtering: true,
                layout: FurnaceLayout::DEFAULT,
            },
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), &[1, 10, 1, 4]);
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = SetPlayerFurnaceOptions::read(&mut reader).unwrap();
        assert_eq!(decoded.furnace_type, FurnaceType::FURNACE);
        assert_eq!(decoded.options.left_tab, FurnaceLeftTab::INVENTORY);
        assert!(decoded.options.filtering);
        assert_eq!(decoded.options.layout, FurnaceLayout::DEFAULT);
    }

    #[test]
    fn record_started_round_trip() {
        let packet = RecordStarted {
            x: 1,
            y: 64,
            z: -2,
            server_sound_handle: 7,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = RecordStarted::read(&mut reader).unwrap();
        assert_eq!((decoded.x, decoded.y, decoded.z), (1, 64, -2));
        assert_eq!(decoded.server_sound_handle, 7);
    }

    #[test]
    fn stonecutter_recipe_echo_round_trip() {
        let packet = ClientboundStonecutterSetRecipe {
            player_id: -1,
            container_id: 29,
            recipe_index: 3,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ClientboundStonecutterSetRecipe::read(&mut reader).unwrap();
        assert_eq!(decoded.player_id, -1);
        assert_eq!(decoded.container_id, 29);
        assert_eq!(decoded.recipe_index, 3);
    }

    #[test]
    fn passenger_of_block_round_trip() {
        let packet = SetPassengerOfBlock {
            passenger_id: 5,
            x: 1,
            y: 2,
            z: 3,
            offset_x: 0.0,
            offset_y: 0.5,
            offset_z: 0.0,
            rotation: 0.0,
            rotation_limit: 0.0,
            emote: PassengerEmote::RIDING,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = SetPassengerOfBlock::read(&mut reader).unwrap();
        assert_eq!(decoded.passenger_id, 5);
        assert_eq!((decoded.x, decoded.y, decoded.z), (1, 2, 3));
        assert_eq!(decoded.emote, PassengerEmote::RIDING);
    }
}
