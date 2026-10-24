//! 交互包（0x21），客户端发往服务端。
//!
//! Interact (0x21), client -> server.
//!
//! Bedrock sends this packet when the player presses E.  Opening the
//! player's own inventory is action 6; the optional coordinates are kept
//! because they are part of the wire format even though this action does not
//! use them.

use std::io::Error;

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

/// 交互动作类型常量。
///
/// Interact action type constants.
#[allow(non_snake_case)]
pub mod InteractAction {
    pub const VEHICLE_EXIT: u8 = 3;
    pub const MOUSEOVER: u8 = 4;
    pub const OPEN_NPC: u8 = 5;
    pub const OPEN_INVENTORY: u8 = 6;
}

/// 交互包，描述玩家对实体或容器的交互行为。
///
/// Interact packet describing how the player interacts with an entity or a
/// container.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct Interact {
    /// 交互动作类型，取值见 [`InteractAction`]。
    ///
    /// The interact action type; see [`InteractAction`] for the values.
    pub action: u8,
    /// 被交互实体的运行时 ID。
    ///
    /// The runtime entity id of the interacted entity.
    pub target: u64,
    /// 后续是否携带交互坐标。
    ///
    /// Whether interaction coordinates follow.
    pub has_position: bool,
    /// 交互点 X 坐标。
    ///
    /// The X coordinate of the interaction point.
    pub x: f32,
    /// 交互点 Y 坐标。
    ///
    /// The Y coordinate of the interaction point.
    pub y: f32,
    /// 交互点 Z 坐标。
    ///
    /// The Z coordinate of the interaction point.
    pub z: f32,
}

impl Reader<Interact> for Interact {
    /// 从字节流中解码一个交互包。
    ///
    /// Decodes an interact packet from a byte stream.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足时返回 IO 错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let action = buf.read_u8()?;
        let target = buf.read_var_u64()?;
        let has_position = buf.read_bool()?;
        let (x, y, z) = if has_position {
            (buf.read_f32_le()?, buf.read_f32_le()?, buf.read_f32_le()?)
        } else {
            (0.0, 0.0, 0.0)
        };
        Ok(Self {
            action,
            target,
            has_position,
            x,
            y,
            z,
        })
    }
}

impl Writer for Interact {
    /// 将交互包编码写入字节流。
    ///
    /// Encodes the interact packet into a byte stream.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.action)?;
        buf.write_var_u64(self.target)?;
        buf.write_bool(self.has_position)?;
        if self.has_position {
            buf.write_f32_le(self.x)?;
            buf.write_f32_le(self.y)?;
            buf.write_f32_le(self.z)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};

    #[test]
    fn open_inventory_round_trip_without_position() {
        let packet = Interact {
            action: InteractAction::OPEN_INVENTORY,
            target: 42,
            has_position: false,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), &[6, 42, 0]);

        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = Interact::read(&mut reader).unwrap();
        assert_eq!(decoded.action, InteractAction::OPEN_INVENTORY);
        assert_eq!(decoded.target, 42);
        assert!(!decoded.has_position);
    }
}
