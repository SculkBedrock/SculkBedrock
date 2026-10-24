//! 客户端资源包交互包：资源包回应与分块请求。
//!
//! Client resource pack interaction packets: resource pack responses and
//! chunk requests.

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;
use uuid::Uuid;

/// 资源包回应中的单条记录：资源包 UUID 与版本。
///
/// A single entry in a resource pack response: the pack UUID and version.
#[derive(Clone, Debug)]
pub struct ResponseEntry {
    /// 资源包的 UUID 字符串。
    ///
    /// The UUID string of the resource pack.
    pub uuid: String,
    /// 资源包的版本字符串。
    ///
    /// The version string of the resource pack.
    pub version: String,
}

/// 资源包回应包（0x08），客户端报告其资源包处理状态。
///
/// Resource pack client response packet (0x08), where the client reports the
/// status of its resource pack handling.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ResourcePackClientResponse {
    /// 回应状态，取值见 `ResourcePackClientResponse::STATUS_*`。
    ///
    /// The response status; see `ResourcePackClientResponse::STATUS_*`.
    pub response_status: u8,
    /// 需要下载的资源包条目列表，仅在状态为 `STATUS_SEND_PACKS` 时存在。
    ///
    /// The list of resource pack entries to download; present only when the
    /// status is `STATUS_SEND_PACKS`.
    pub entries: Vec<ResponseEntry>,
}

impl ResourcePackClientResponse {
    /// 客户端拒绝全部资源包。
    ///
    /// The client refused all resource packs.
    pub const STATUS_REFUSED: u8 = 1;
    /// 客户端请求服务端发送资源包。
    ///
    /// The client asks the server to send the resource packs.
    pub const STATUS_SEND_PACKS: u8 = 2;
    /// 客户端已持有全部资源包。
    ///
    /// The client already has all resource packs.
    pub const STATUS_HAVE_ALL_PACKS: u8 = 3;
    /// 客户端已完成资源包加载。
    ///
    /// The client finished loading the resource packs.
    pub const STATUS_COMPLETED: u8 = 4;
    /// 构造一个空状态（状态值 0）的资源包回应包。
    ///
    /// Constructs a resource pack response packet with the empty status value
    /// `0`.
    pub fn new() -> Self {
        Self {
            response_status: 0,
            entries: vec![],
        }
    }
}

impl Writer for ResourcePackClientResponse {
    /// 资源包回应包无需回写，编码为空操作。
    ///
    /// The resource pack response packet is never written back; encoding is a
    /// no-op.
    ///
    /// - `_buf`: 输出字节写入器（未使用）
    fn write(&self, _buf: &mut ByteWriter) -> Result<(), Error> {
        Ok(())
    }
}

impl Reader<ResourcePackClientResponse> for ResourcePackClientResponse {
    /// 从字节流中解码一个资源包回应包，按协议版本选择布局。
    ///
    /// Decodes a resource pack response packet from a byte stream, choosing
    /// the layout by protocol version.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足、条目格式非法或状态字段缺失时返回 IO 错误。
    fn read(buf: &mut ByteReader) -> Result<ResourcePackClientResponse, Error> {
        let mut packet = ResourcePackClientResponse::new();
        let protocol = crate::protocol::version::current_protocol_version();
        if protocol >= 2168 {
            // Status byte is zero-based (add 1 after reading), then a type-enum string.
            // Entries exist only in the SEND_PACKS status; the count is a var-unsigned int.
            let status_wire = buf.read_u8()?;
            packet.response_status = status_wire.wrapping_add(1);
            let _type_string = buf.read_string()?;
            if packet.response_status == Self::STATUS_SEND_PACKS {
                let entries_len = buf.read_var_u32()? as usize;
                for _ in 0..entries_len {
                    let entry = buf.read_string()?;
                    let entry: Vec<&str> = entry.splitn(2, '_').collect();
                    packet.entries.push(ResponseEntry {
                        uuid: entry
                            .get(0)
                            .ok_or(Error::other("Cannot get uuid"))?
                            .to_string(),
                        version: entry
                            .get(1)
                            .ok_or(Error::other("Cannot get version"))?
                            .to_string(),
                    })
                }
            }
        } else {
            packet.response_status = buf.read_u8()?;
            let entries_len = buf.read_i16_le()? as u16;
            for _ in 0..entries_len {
                let entry = buf.read_string()?;
                let entry: Vec<&str> = entry.splitn(2, '_').collect();
                packet.entries.push(ResponseEntry {
                    uuid: entry
                        .get(0)
                        .ok_or(Error::other("Cannot get uuid"))?
                        .to_string(),
                    version: entry
                        .get(1)
                        .ok_or(Error::other("Cannot get version"))?
                        .to_string(),
                })
            }
        }
        Ok(packet)
    }
}

/// 资源包分块请求包（0x54），客户端请求某个资源包的一段数据。
///
/// Resource pack chunk request packet (0x54), where the client requests one
/// slice of a resource pack.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ResourcePackChunkRequest {
    /// 被请求的资源包 UUID。
    ///
    /// The UUID of the requested resource pack.
    pub pack_id: Uuid,
    /// 被请求的分块序号。
    ///
    /// The index of the requested chunk.
    pub chunk_index: u32,
}

impl Writer for ResourcePackChunkRequest {
    /// 资源包分块请求包无需回写，编码为空操作。
    ///
    /// The resource pack chunk request packet is never written back; encoding
    /// is a no-op.
    ///
    /// - `_buf`: 输出字节写入器（未使用）
    fn write(&self, _buf: &mut ByteWriter) -> Result<(), Error> {
        Ok(())
    }
}

impl Reader<ResourcePackChunkRequest> for ResourcePackChunkRequest {
    /// 从字节流中解码一个资源包分块请求包。
    ///
    /// Decodes a resource pack chunk request packet from a byte stream.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或资源包 UUID 非法时返回 IO 错误。
    fn read(buf: &mut ByteReader) -> Result<ResourcePackChunkRequest, Error> {
        let raw = buf.read_string()?;
        let parts: Vec<&str> = raw.splitn(2, '_').collect();
        let uuid_str = parts[0];
        let pack_id = Uuid::parse_str(uuid_str)
            .map_err(|e| Error::other(format!("Invalid pack_id UUID: {}", e)))?;
        let chunk_index = buf.read_i32_le()? as u32;
        Ok(ResourcePackChunkRequest {
            pack_id,
            chunk_index,
        })
    }
}
