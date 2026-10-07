//! AddPlayerPacket (0x0c), server -> client: spawn a player entity.
//!
//! Skin arrives via PlayerList; the client renders the entity after the list
//! entry. Metadata and ability layers use fixed defaults (empty metadata,
//! single BASE ability layer).

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;
use uuid::Uuid;

use crate::protocol::client::transaction::{read_item_data, write_item_data, ItemData};

#[derive(Clone, Debug, MinecraftPacket)]
pub struct AddPlayer {
    pub uuid: Uuid,
    pub username: String,
    pub entity_runtime_id: u64,
    pub platform_chat_id: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub speed_x: f32,
    pub speed_y: f32,
    pub speed_z: f32,
    pub pitch: f32,
    pub yaw: f32,
    pub head_yaw: f32,
    /// Game mode (varint; Survival = 0).
    pub game_type: i32,
    pub player_permission: u32,
    pub command_permission: u32,
    pub device_id: String,
    pub build_platform: i32,
}

impl Writer for AddPlayer {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_uuid(&self.uuid)?;
        buf.write_string(&self.username)?;
        buf.write_var_u64(self.entity_runtime_id)?;
        buf.write_string(&self.platform_chat_id)?;
        buf.write_f32_le(self.x)?;
        buf.write_f32_le(self.y)?;
        buf.write_f32_le(self.z)?;
        buf.write_f32_le(self.speed_x)?;
        buf.write_f32_le(self.speed_y)?;
        buf.write_f32_le(self.speed_z)?;
        buf.write_f32_le(self.pitch)?;
        buf.write_f32_le(self.yaw)?;
        buf.write_f32_le(self.head_yaw)?;
        write_item_data(buf, &ItemData::default())?;
        buf.write_var_i32(self.game_type)?;
        // Empty EntityMetadata.
        buf.write_var_u32(0)?;
        // Entity properties (int/float counts).
        buf.write_var_u32(0)?;
        buf.write_var_u32(0)?;
        // entityUniqueId (fixed 64-bit little-endian).
        buf.write_i64_le(self.entity_runtime_id as i64)?;
        // Permissions.
        buf.write_var_u32(self.player_permission)?;
        buf.write_var_u32(self.command_permission)?;
        // Ability layers: single BASE layer with default values.
        buf.write_var_u32(1)?;
        buf.write_i16_le(1)?; // layer_type = BASE
        buf.write_i32_le(262143)?; // abilities_set
        buf.write_i32_le(63)?; // ability_values
        buf.write_f32_le(0.1)?; // fly_speed
        buf.write_f32_le(1.0)?; // vertical_fly_speed
        buf.write_f32_le(0.05)?; // walk_speed
                                 // Entity links: none.
        buf.write_var_u32(0)?;
        if crate::protocol::version::protocol_at_least(
            crate::protocol::version::PROTOCOL_VERSION_1_26_60,
        ) {
            // Passenger block data (empty: no vehicle block passengers).
            buf.write_var_i32(0)?;
            buf.write_var_i32(0)?;
            buf.write_var_i32(0)?;
            buf.write_f32_le(0.0)?;
            buf.write_f32_le(0.0)?;
            buf.write_f32_le(0.0)?;
            buf.write_f32_le(0.0)?;
            buf.write_f32_le(0.0)?;
            buf.write_u8(0)?;
        }
        buf.write_string(&self.device_id)?;
        buf.write_i32_le(self.build_platform)
    }
}

impl Reader<AddPlayer> for AddPlayer {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let uuid = buf.read_uuid()?;
        let username = buf.read_string()?;
        let entity_runtime_id = buf.read_var_u64()?;
        let platform_chat_id = buf.read_string()?;
        let x = buf.read_f32_le()?;
        let y = buf.read_f32_le()?;
        let z = buf.read_f32_le()?;
        let speed_x = buf.read_f32_le()?;
        let speed_y = buf.read_f32_le()?;
        let speed_z = buf.read_f32_le()?;
        let pitch = buf.read_f32_le()?;
        let yaw = buf.read_f32_le()?;
        let head_yaw = buf.read_f32_le()?;
        let _item = read_item_data(buf)?;
        let game_type = buf.read_var_i32()?;
        let _metadata_count = buf.read_var_u32()?;
        let _properties_int = buf.read_var_u32()?;
        let _properties_float = buf.read_var_u32()?;
        let _entity_unique_id = buf.read_i64_le()?;
        let player_permission = buf.read_var_u32()?;
        let command_permission = buf.read_var_u32()?;
        let _ability_layers = buf.read_var_u32()?;
        let _layer_type = buf.read_i16_le()?;
        let _abilities_set = buf.read_i32_le()?;
        let _ability_values = buf.read_i32_le()?;
        let _fly_speed = buf.read_f32_le()?;
        let _vertical_fly_speed = buf.read_f32_le()?;
        let _walk_speed = buf.read_f32_le()?;
        let _links = buf.read_var_u32()?;
        if crate::protocol::version::protocol_at_least(
            crate::protocol::version::PROTOCOL_VERSION_1_26_60,
        ) {
            // Passenger block data (see Writer).
            let _ = buf.read_var_i32()?;
            let _ = buf.read_var_i32()?;
            let _ = buf.read_var_i32()?;
            let _ = buf.read_f32_le()?;
            let _ = buf.read_f32_le()?;
            let _ = buf.read_f32_le()?;
            let _ = buf.read_f32_le()?;
            let _ = buf.read_f32_le()?;
            let _ = buf.read_u8()?;
        }
        let device_id = buf.read_string()?;
        let build_platform = buf.read_i32_le()?;
        Ok(Self {
            uuid,
            username,
            entity_runtime_id,
            platform_chat_id,
            x,
            y,
            z,
            speed_x,
            speed_y,
            speed_z,
            pitch,
            yaw,
            head_yaw,
            game_type,
            player_permission,
            command_permission,
            device_id,
            build_platform,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::version::{
        with_protocol_version, PROTOCOL_VERSION_1_26_40, PROTOCOL_VERSION_1_26_60,
    };
    use sc_binary::interfaces::{Reader, Writer};

    fn minimal_add_player() -> AddPlayer {
        AddPlayer {
            uuid: Uuid::nil(),
            username: String::new(),
            entity_runtime_id: 1,
            platform_chat_id: String::new(),
            x: 0.0,
            y: 0.0,
            z: 0.0,
            speed_x: 0.0,
            speed_y: 0.0,
            speed_z: 0.0,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            game_type: 0,
            player_permission: 0,
            command_permission: 0,
            device_id: String::new(),
            build_platform: 0,
        }
    }

    /// Passenger block data is present only for protocol 2225 and later.
    /// Empty defaults occupy 24 bytes (3 varints, 5 floats, 1 byte).
    #[test]
    fn add_player_passenger_block_round_trips_only_on_2225() {
        let old_bytes = with_protocol_version(PROTOCOL_VERSION_1_26_40, || {
            let mut writer = ByteWriter::new();
            minimal_add_player().write(&mut writer).unwrap();
            writer.as_slice().to_vec()
        });
        let new_bytes = with_protocol_version(PROTOCOL_VERSION_1_26_60, || {
            let mut writer = ByteWriter::new();
            minimal_add_player().write(&mut writer).unwrap();
            writer.as_slice().to_vec()
        });
        assert_eq!(new_bytes.len(), old_bytes.len() + 24);
        let decoded = with_protocol_version(PROTOCOL_VERSION_1_26_60, || {
            let mut reader = ByteReader::from(new_bytes.as_slice());
            AddPlayer::read(&mut reader).unwrap()
        });
        assert_eq!(decoded.entity_runtime_id, 1);
        assert_eq!(decoded.build_platform, 0);
    }
}
