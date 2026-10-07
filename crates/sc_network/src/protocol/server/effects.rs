//! Bedrock world effect packets.

use std::io::Error;

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

/// LevelEvent (0x19).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct LevelEvent {
    pub event_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub data: i32,
}

impl Writer for LevelEvent {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        // Event id is zigzag varint32.
        buf.write_var_i32(self.event_id as i32)?;
        buf.write_f32_le(self.x)?;
        buf.write_f32_le(self.y)?;
        buf.write_f32_le(self.z)?;
        buf.write_var_i32(self.data)
    }
}

impl Reader<LevelEvent> for LevelEvent {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "LevelEvent decode is unsupported",
        ))
    }
}

/// LevelSoundEvent (0x7b).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct LevelSoundEvent {
    /// v2168 uses the v1001 serializer: a sound serialize name, not a numeric id.
    pub sound: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub extra_data: i32,
    pub entity_type: String,
    pub is_baby_mob: bool,
    pub is_global: bool,
    /// Entity id associated with the sound, or -1 when it is not entity-bound.
    pub entity_unique_id: i64,
    /// v975+ optional fire-at position. None is encoded as a single false byte.
    pub fire_at_position: Option<(f32, f32, f32)>,
}

impl Writer for LevelSoundEvent {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_string(&self.sound)?;
        buf.write_f32_le(self.x)?;
        buf.write_f32_le(self.y)?;
        buf.write_f32_le(self.z)?;
        buf.write_var_i32(self.extra_data)?;
        buf.write_string(&self.entity_type)?;
        buf.write_bool(self.is_baby_mob)?;
        buf.write_bool(self.is_global)?;
        buf.write_i64_le(self.entity_unique_id)?;
        buf.write_bool(self.fire_at_position.is_some())?;
        if let Some((x, y, z)) = self.fire_at_position {
            buf.write_f32_le(x)?;
            buf.write_f32_le(y)?;
            buf.write_f32_le(z)?;
        }
        Ok(())
    }
}

impl Reader<LevelSoundEvent> for LevelSoundEvent {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "LevelSoundEvent decode is unsupported",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::Writer;

    #[test]
    fn level_event_writer_has_stable_prefix() {
        let packet = LevelEvent {
            event_id: 2_001,
            x: 1.0,
            y: 2.0,
            z: 3.0,
            data: 42,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        // event 2001 zigzag → varint(4002) = [0xa2, 0x1f].
        assert_eq!(&writer.as_slice()[..2], &[0xa2, 0x1f]);
    }

    #[test]
    fn level_sound_event_writer_has_v1001_tail() {
        let packet = LevelSoundEvent {
            sound: "break".to_owned(),
            x: 1.0,
            y: 2.0,
            z: 3.0,
            extra_data: 0,
            entity_type: String::new(),
            is_baby_mob: false,
            is_global: false,
            entity_unique_id: -1,
            fire_at_position: None,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();

        let bytes = writer.as_slice();
        assert_eq!(&bytes[..6], b"\x05break");
        assert_eq!(
            &bytes[bytes.len() - 9..bytes.len() - 1],
            &(-1i64).to_le_bytes()
        );
        assert_eq!(bytes[bytes.len() - 1], 0);
    }

    #[test]
    fn level_sound_event_packet_id_is_0x7b() {
        use crate::protocol::MinecraftPackets;

        let packet = LevelSoundEvent {
            sound: "break".to_owned(),
            x: 1.0,
            y: 2.0,
            z: 3.0,
            extra_data: 0,
            entity_type: ":".to_owned(),
            is_baby_mob: false,
            is_global: false,
            entity_unique_id: -1,
            fire_at_position: None,
        };
        let bytes = MinecraftPackets::LevelSoundEvent(packet)
            .write_to_bytes()
            .unwrap();
        // BinaryIo enum discriminants are u16 BE; string sound packets use 0x7b.
        assert_eq!(&bytes.as_slice()[..2], &[0x00, 0x7b]);
    }
}
