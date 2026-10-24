//! 世界特效与音效包定义。
//!
//! World effect and sound packet definitions.

use std::io::Error;

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

/// LevelEventPacket (0x19)：世界级特效/事件（如粒子、破坏动画）。
///
/// LevelEventPacket (0x19): world-level effects/events (particles, break
/// animations, etc.).
///
/// event_id 为 zigzag varint32 编码，坐标为小端 f32。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct LevelEvent {
    /// 事件类型 id（zigzag varint32）。
    ///
    /// Event type id (zigzag varint32).
    pub event_id: u32,
    /// 事件发生处的 X 坐标。
    ///
    /// X coordinate of the event position.
    pub x: f32,
    /// 事件发生处的 Y 坐标。
    ///
    /// Y coordinate of the event position.
    pub y: f32,
    /// 事件发生处的 Z 坐标。
    ///
    /// Z coordinate of the event position.
    pub z: f32,
    /// 事件附加数据（zigzag varint32）。
    ///
    /// Extra event data (zigzag varint32).
    pub data: i32,
}

impl Writer for LevelEvent {
    /// 写入 LevelEvent 包体。
    ///
    /// Writes the LevelEvent packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
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
    /// 不支持解码：服务端只发送 LevelEvent。
    ///
    /// Decoding is unsupported: the server only sends LevelEvent.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "LevelEvent decode is unsupported",
        ))
    }
}

/// LevelSoundEventPacket (0x7b)：世界音效事件。
///
/// LevelSoundEventPacket (0x7b): world sound event.
///
/// v2168 起采用 v1001 序列化器：sound 为序列化名称字符串而非数字 id。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct LevelSoundEvent {
    /// 音效的序列化名称（v1001 序列化器）。
    ///
    /// Serialized sound name (v1001 serializer).
    pub sound: String,
    /// 音效发生处的 X 坐标。
    ///
    /// X coordinate of the sound position.
    pub x: f32,
    /// 音效发生处的 Y 坐标。
    ///
    /// Y coordinate of the sound position.
    pub y: f32,
    /// 音效发生处的 Z 坐标。
    ///
    /// Z coordinate of the sound position.
    pub z: f32,
    /// 音效附加数据。
    ///
    /// Extra sound data.
    pub extra_data: i32,
    /// 发声实体类型名。
    ///
    /// Entity type name of the sound source.
    pub entity_type: String,
    /// 是否为幼年实体音效。
    ///
    /// Whether the sound is a baby-mob variant.
    pub is_baby_mob: bool,
    /// 是否为全局音效（无位置衰减）。
    ///
    /// Whether the sound is global (no positional attenuation).
    pub is_global: bool,
    /// 与音效关联的实体唯一 id；非实体绑定时为 -1。
    ///
    /// Entity unique id associated with the sound, or -1 when not
    /// entity-bound.
    pub entity_unique_id: i64,
    /// v975+ 可选的开火位置；None 编码为单个 false 字节。
    ///
    /// Optional fire-at position (v975+); None is encoded as a single false
    /// byte.
    pub fire_at_position: Option<(f32, f32, f32)>,
}

impl Writer for LevelSoundEvent {
    /// 写入 LevelSoundEvent 包体（v1001 序列化器布局）。
    ///
    /// Writes the LevelSoundEvent packet body (v1001 serializer layout).
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
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
    /// 不支持解码：服务端只发送 LevelSoundEvent。
    ///
    /// Decoding is unsupported: the server only sends LevelSoundEvent.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
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
