//! Modern client configuration packets used by the 2168 login sequence.
//!
//! The packet values remain semantic Rust data; no captured wire bytes are stored
//! here.

use std::io::{Error, ErrorKind};

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::{NbtValue, SCNBTByteWriter};
use sc_network_macros::MinecraftPacket;

/// SyncActorPropertyPacket (0xa5).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SyncActorProperty {
    pub property_data: CompoundNbt,
}

/// VoxelShapesPacket (0x151).
///
/// Base layout: shapes array (uvarint count; per entry cells{u8 xyz size +
/// storage array} + x/y/z coordinate arrays) + nameMap (uvarint count +
/// string/sLE pairs); newer versions append customShapeCount (sLE).
/// An empty registry encodes as 4 bytes.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct VoxelShapes {
    /// Named voxel shape table (name to registry handle). Always empty (no custom shapes).
    pub shapes: Vec<(String, u16)>,
    /// Newer versions: custom shape count.
    pub custom_shape_count: u16,
}

impl VoxelShapes {
    pub fn empty() -> Self {
        Self {
            shapes: Vec::new(),
            custom_shape_count: 0,
        }
    }
}

impl Default for VoxelShapes {
    fn default() -> Self {
        Self::empty()
    }
}

impl Writer for VoxelShapes {
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
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "VoxelShapes decode is unsupported",
        ))
    }
}

impl SyncActorProperty {
    pub fn empty() -> Self {
        Self {
            property_data: CompoundNbt::new(None),
        }
    }
}

impl Default for SyncActorProperty {
    fn default() -> Self {
        Self::empty()
    }
}

impl Writer for SyncActorProperty {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_nbt::<BedrockNetworkNbt>(&NbtValue::Compound(self.property_data.clone()))
    }
}

impl Reader<SyncActorProperty> for SyncActorProperty {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "SyncActorProperty decode is unsupported",
        ))
    }
}

/// PlayerFogPacket (0xa0).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct PlayerFog {
    pub fog_stack: Vec<String>,
}

impl PlayerFog {
    pub fn empty() -> Self {
        Self {
            fog_stack: Vec::new(),
        }
    }
}

impl Default for PlayerFog {
    fn default() -> Self {
        Self::empty()
    }
}

impl Writer for PlayerFog {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.fog_stack.len() as u32)?;
        for fog in &self.fog_stack {
            buf.write_string(fog)?;
        }
        Ok(())
    }
}

impl Reader<PlayerFog> for PlayerFog {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "PlayerFog decode is unsupported",
        ))
    }
}

/// TrimDataPacket (0x12e).
#[derive(Clone, Debug, Default, MinecraftPacket)]
pub struct TrimData {
    pub patterns: Vec<TrimPattern>,
    pub materials: Vec<TrimMaterial>,
}

#[derive(Clone, Debug)]
pub struct TrimPattern {
    pub item_name: String,
    pub pattern_id: String,
}

#[derive(Clone, Debug)]
pub struct TrimMaterial {
    pub material_id: String,
    pub color: String,
    pub item_name: String,
}

impl Writer for TrimData {
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
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "TrimData decode is unsupported",
        ))
    }
}

/// CameraPresetsPacket (0xc6), structured format.
///
/// Replaces the legacy NBT form: `uvarint count + per-preset structs`
/// (name/parent strings plus a run of option fields, each with a 1-byte
/// presence flag).
#[derive(Clone, Debug, Default, MinecraftPacket)]
pub struct CameraPresets {
    pub presets: Vec<CameraPreset>,
}

#[derive(Clone, Debug)]
pub struct CameraPreset {
    pub identifier: String,
    pub inherit_from: String,
    /// Position (three independent option lf32).
    pub position: Option<[f32; 3]>,
    /// rotation.x(rot_x = pitch).
    pub pitch: Option<f32>,
    /// rotation.y(rot_y = yaw).
    pub yaw: Option<f32>,
    /// view offset(option vec2f).
    pub offset: Option<[f32; 2]>,
    /// entity offset(option vec3f).
    pub entity_offset: Option<[f32; 3]>,
    /// tracking radius(option lf32).
    pub radius: Option<f32>,
}

impl CameraPreset {
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
    /// Six built-in camera presets.
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

/// Option lf32: presence flag (bool) + LE f32.
fn write_opt_f32(buf: &mut ByteWriter, value: Option<f32>) -> Result<(), Error> {
    match value {
        Some(v) => {
            buf.write_u8(1)?;
            buf.write_f32_le(v)
        }
        None => buf.write_u8(0),
    }
}

/// Option bool: presence flag + bool.
fn write_opt_bool(buf: &mut ByteWriter, value: Option<bool>) -> Result<(), Error> {
    match value {
        Some(v) => {
            buf.write_u8(1)?;
            buf.write_u8(v as u8)
        }
        None => buf.write_u8(0),
    }
}

/// Option vec2f: presence flag + 2xLE f32.
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

/// Option vec3f: presence flag + 3xLE f32.
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
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "CameraPresets decode is unsupported",
        ))
    }
}

/// CameraAimAssistPresetsPacket (0x140).
///
/// The 2168 serializer writes category definitions, preset definitions, then
/// one operation byte.  The semantic definition types are intentionally kept
/// small here; the packet can be populated from a registry when one is
/// available, while an empty SET packet remains valid for vanilla worlds.
#[derive(Clone, Debug, Default, MinecraftPacket)]
pub struct CameraAimAssistPresets {
    pub categories: Vec<CameraAimAssistCategory>,
    pub presets: Vec<CameraAimAssistPreset>,
    pub operation: CameraAimAssistOperation,
}

#[derive(Clone, Debug, Default)]
pub struct CameraAimAssistCategory {
    pub name: String,
    pub entities: Vec<CameraAimAssistPriority>,
    pub blocks: Vec<CameraAimAssistPriority>,
    pub entity_default: Option<i32>,
    pub block_default: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct CameraAimAssistPriority {
    pub identifier: String,
    pub priority: i32,
}

#[derive(Clone, Debug, Default)]
pub struct CameraAimAssistPreset {
    pub identifier: String,
    pub categories: String,
    pub exclusions: Vec<String>,
    pub liquid_targeting: Vec<String>,
    pub item_settings: Vec<CameraAimAssistItemSetting>,
    pub default_item_settings: Option<String>,
    pub hand_settings: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CameraAimAssistItemSetting {
    pub item_id: String,
    pub category: String,
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(u8)]
pub enum CameraAimAssistOperation {
    #[default]
    Set = 0,
    AddToExisting = 1,
}

impl Writer for CameraAimAssistPresets {
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
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "CameraAimAssistPresets decode is unsupported",
        ))
    }
}

fn write_priorities(buf: &mut ByteWriter, values: &[CameraAimAssistPriority]) -> Result<(), Error> {
    buf.write_var_u32(values.len() as u32)?;
    for value in values {
        buf.write_string(&value.identifier)?;
        buf.write_i32_le(value.priority)?;
    }
    Ok(())
}

fn write_strings(buf: &mut ByteWriter, values: &[String]) -> Result<(), Error> {
    buf.write_var_u32(values.len() as u32)?;
    for value in values {
        buf.write_string(value)?;
    }
    Ok(())
}

fn write_optional_i32(buf: &mut ByteWriter, value: Option<i32>) -> Result<(), Error> {
    buf.write_bool(value.is_some())?;
    if let Some(value) = value {
        buf.write_i32_le(value)?;
    }
    Ok(())
}

fn write_optional_string(buf: &mut ByteWriter, value: Option<&str>) -> Result<(), Error> {
    buf.write_bool(value.is_some())?;
    if let Some(value) = value {
        buf.write_string(value)?;
    }
    Ok(())
}

fn write_nbt_string(buf: &mut ByteWriter, value: &str) -> Result<(), Error> {
    buf.write_var_u32(value.len() as u32)?;
    buf.write(value.as_bytes())
}

/// Furnace type for SetPlayerFurnaceOptions (uint8).
pub mod FurnaceType {
    pub const NONE: u8 = 0;
    pub const FURNACE: u8 = 1;
    pub const BLAST_FURNACE: u8 = 2;
    pub const SMOKER: u8 = 3;
}

/// Left recipe tab index for furnace options (varint).
pub mod FurnaceLeftTab {
    pub const NONE: i32 = 0;
    pub const RECIPE_FOOD: i32 = 1;
    pub const RECIPE_ITEMS: i32 = 2;
    pub const RECIPE_BLOCKS: i32 = 3;
    pub const RECIPE_SEARCH: i32 = 4;
    pub const INVENTORY: i32 = 5;
}

/// Furnace layout for furnace options (varint).
pub mod FurnaceLayout {
    pub const NONE: i32 = 0;
    pub const INVENTORY_ONLY: i32 = 1;
    pub const DEFAULT: i32 = 2;
}

/// Furnace UI options block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FurnaceOptions {
    pub left_tab: i32,
    pub filtering: bool,
    pub layout: i32,
}

fn write_furnace_options(buf: &mut ByteWriter, options: &FurnaceOptions) -> Result<(), Error> {
    buf.write_var_i32(options.left_tab)?;
    buf.write_bool(options.filtering)?;
    buf.write_var_i32(options.layout)
}

fn read_furnace_options(buf: &mut ByteReader) -> Result<FurnaceOptions, Error> {
    Ok(FurnaceOptions {
        left_tab: buf.read_var_i32()?,
        filtering: buf.read_bool()?,
        layout: buf.read_var_i32()?,
    })
}

/// SetPlayerFurnaceOptions (0x15f).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetPlayerFurnaceOptions {
    pub furnace_type: u8,
    pub options: FurnaceOptions,
}

impl Reader<SetPlayerFurnaceOptions> for SetPlayerFurnaceOptions {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            furnace_type: buf.read_u8()?,
            options: read_furnace_options(buf)?,
        })
    }
}

impl Writer for SetPlayerFurnaceOptions {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.furnace_type)?;
        write_furnace_options(buf, &self.options)
    }
}

/// RecordStarted (0x160). Jukebox playback notification.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct RecordStarted {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub server_sound_handle: u64,
}

impl Reader<RecordStarted> for RecordStarted {
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
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.x)?;
        buf.write_var_i32(self.y)?;
        buf.write_var_i32(self.z)?;
        buf.write_var_u64(self.server_sound_handle)
    }
}

/// Stonecutter recipe index echo (0x163).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ClientboundStonecutterSetRecipe {
    pub player_id: i64,
    pub container_id: u8,
    pub recipe_index: i32,
}

impl Reader<ClientboundStonecutterSetRecipe> for ClientboundStonecutterSetRecipe {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            player_id: buf.read_var_i64()?,
            container_id: buf.read_u8()?,
            recipe_index: buf.read_var_i32()?,
        })
    }
}

impl Writer for ClientboundStonecutterSetRecipe {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i64(self.player_id)?;
        buf.write_u8(self.container_id)?;
        buf.write_var_i32(self.recipe_index)
    }
}

/// Matchmaking state values (uint8).
pub mod MatchmakingState {
    pub const IDLE: u8 = 0;
}

/// Matchmaking state notification (0x161).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ClientboundMatchmakingState {
    pub state: u8,
    pub destination_name: String,
    pub triggering_player_name: String,
    pub triggered_by_local_player: bool,
}

impl Reader<ClientboundMatchmakingState> for ClientboundMatchmakingState {
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
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.state)?;
        buf.write_string(&self.destination_name)?;
        buf.write_string(&self.triggering_player_name)?;
        buf.write_bool(self.triggered_by_local_player)
    }
}

/// Signed audio content reference (compact JWT string).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedAudioContent {
    pub compact_jwt: String,
}

fn write_signed_audio_content(
    buf: &mut ByteWriter,
    content: &SignedAudioContent,
) -> Result<(), Error> {
    buf.write_string(&content.compact_jwt)
}

fn read_signed_audio_content(buf: &mut ByteReader) -> Result<SignedAudioContent, Error> {
    Ok(SignedAudioContent {
        compact_jwt: buf.read_string()?,
    })
}

/// Server-driven audio playback (0x167).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ClientboundPlayAudioContent {
    pub shared_metadata: SignedAudioContent,
    pub playback_content: SignedAudioContent,
    pub playback_type: u8,
    pub sound_name: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub volume: f32,
    pub pitch: f32,
    pub loop_count: i32,
    pub bypass_listener_range_check: bool,
    pub server_sound_handle: u64,
    pub playback_position_seconds: f32,
}

impl Reader<ClientboundPlayAudioContent> for ClientboundPlayAudioContent {
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

/// Block passenger emote values (uint8).
pub mod PassengerEmote {
    pub const STANDING: u8 = 0;
    pub const RIDING: u8 = 1;
    pub const LAYING: u8 = 2;
}

/// Block passenger assignment (0x165).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetPassengerOfBlock {
    pub passenger_id: i64,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub offset_z: f32,
    pub rotation: f32,
    pub rotation_limit: f32,
    pub emote: u8,
}

impl Reader<SetPassengerOfBlock> for SetPassengerOfBlock {
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
