//! 双向容器窗口包。
//!
//! Bidirectional container window packets.

use std::io::Error;

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

/// 关闭容器包（0x2f），通知服务端玩家已关闭某个容器窗口。
///
/// Container close packet (0x2f), telling the server that the player closed
/// a container window.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ContainerClose {
    /// 被关闭的容器窗口 ID。
    ///
    /// The id of the closed container window.
    pub window_id: u8,
    /// 容器类型标识；`-1` 表示玩家自身背包。
    ///
    /// The container type identifier; `-1` means the player's own inventory.
    pub container_type: i8,
    /// 关闭操作是否由服务端发起。
    ///
    /// Whether the close was initiated by the server.
    pub was_server_initiated: bool,
}

impl Reader<ContainerClose> for ContainerClose {
    /// 从字节流中解码一个关闭容器包。
    ///
    /// Decodes a container close packet from a byte stream.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足时返回 IO 错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            window_id: buf.read_u8()?,
            container_type: buf.read_u8()? as i8,
            was_server_initiated: buf.read_bool()?,
        })
    }
}

impl Writer for ContainerClose {
    /// 将关闭容器包编码写入字节流。
    ///
    /// Encodes the container close packet into a byte stream.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.window_id)?;
        buf.write_u8(self.container_type as u8)?;
        buf.write_bool(self.was_server_initiated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};

    #[test]
    fn container_close_round_trip() {
        let packet = ContainerClose {
            window_id: 0,
            container_type: -1,
            was_server_initiated: false,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), &[0, 0xff, 0]);

        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ContainerClose::read(&mut reader).unwrap();
        assert_eq!(decoded.window_id, 0);
        assert_eq!(decoded.container_type, -1);
        assert!(!decoded.was_server_initiated);
    }
}
