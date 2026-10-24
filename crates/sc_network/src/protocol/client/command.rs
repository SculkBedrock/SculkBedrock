//! 客户端命令请求包。
//!
//! Client command request packets.

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;
use uuid::Uuid;

/// 命令请求包（0x4d），客户端执行命令时发送。
///
/// Command request packet (0x4d), sent when a client runs a command.
///
/// Layout: `command(string) + origin{type(string) + uuid(16B) +
/// requestId(string) + playerId(long LE)} + internal(bool) + version(string)`.
/// Origin type is a string ("player" and similar); playerId is always 8 bytes.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct CommandRequest {
    /// 玩家输入的命令文本。
    ///
    /// The command text typed by the player.
    pub command: String,
    /// 命令来源类型字符串（如 "player"）。
    ///
    /// Origin type string ("player" and similar).
    pub origin_type: String,
    /// 命令来源实体的 UUID。
    ///
    /// The UUID of the entity that issued the command.
    pub uuid: Uuid,
    /// 请求 ID，用于将响应与本次请求关联。
    ///
    /// The request id, used to correlate the response with this request.
    pub request_id: String,
    /// 命令来源实体的数字 ID（小端 64 位）。
    ///
    /// The numeric id of the entity that issued the command (little-endian
    /// 64-bit).
    pub player_id: i64,
    /// 该命令是否由服务端内部发起。
    ///
    /// Whether the command was issued internally by the server.
    pub internal: bool,
    /// 命令协议版本字符串。
    ///
    /// The command protocol version string.
    pub version: String,
}

impl Reader<CommandRequest> for CommandRequest {
    /// 从字节流中解码一个命令请求包。
    ///
    /// Decodes a command request packet from a byte stream.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或字段格式非法时返回 IO 错误。
    fn read(buf: &mut ByteReader) -> Result<CommandRequest, Error> {
        let command = buf.read_string()?;
        let origin_type = buf.read_string()?;
        let most = buf.read_u64_le()?;
        let least = buf.read_u64_le()?;
        let uuid = Uuid::from_u64_pair(most, least);
        let request_id = buf.read_string()?;
        let player_id = buf.read_i64_le()?;
        let internal = buf.read_bool()?;
        let version = buf.read_string()?;
        Ok(Self {
            command,
            origin_type,
            uuid,
            request_id,
            player_id,
            internal,
            version,
        })
    }
}

impl Writer for CommandRequest {
    /// 将命令请求包编码写入字节流。
    ///
    /// Encodes the command request packet into a byte stream.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_string(&self.command)?;
        buf.write_string(&self.origin_type)?;
        buf.write_uuid(&self.uuid)?;
        buf.write_string(&self.request_id)?;
        buf.write_i64_le(self.player_id)?;
        buf.write_bool(self.internal)?;
        buf.write_string(&self.version)
    }
}
