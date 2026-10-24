use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};

/// RakNet 协议常量与 `Magic` 握手字节。
///
/// RakNet protocol constants and the `Magic` handshake bytes.
pub mod ack;
/// 帧编解码（可靠序号、分片等）。
///
/// Frame encoding/decoding (reliable indices, fragments, ...).
pub mod frame;
/// Minecraft Bedrock 协议侧辅助（版本相关常量等）。
///
/// Minecraft Bedrock protocol helpers (version-related constants, ...).
pub mod mcbe;
/// 离线/在线报文定义。
///
/// Offline/online packet definitions.
pub mod packet;
/// 帧可靠性等级。
///
/// Frame reliability levels.
pub mod reliability;

/// RakNet 离线握手魔数：未连接 ping/pong 与 open connection 报文均以它开头。
///
/// RakNet offline magic: unconnected ping/pong and open-connection messages
/// all start with it.
pub(crate) const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x0, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

/// UDP 数据报中单个 RakNet 帧的最大头部开销（字节）。
///
/// Maximum per-frame header overhead (in bytes) inside a RakNet datagram.
pub const RAKNET_HEADER_FRAME_OVERHEAD: u16 = 20 + 8 + 8 + 4 + 20;

/// 单个消息允许的最大分片数。
///
/// Maximum number of fragments allowed for a single message.
pub const MAX_FRAGS: u32 = 1024;

/// RakNet 魔数标记类型：读写 16 字节 [`MAGIC`]。
///
/// 仅作类型标记，不携带数据；读入时会校验魔数是否匹配。
///
/// Marker type for the RakNet magic: reads/writes the 16-byte [`MAGIC`].
///
/// Holds no data; reading validates the magic matches.
#[derive(Debug, Clone)]
pub struct Magic;

impl Magic {
    /// 构造一个魔数标记。
    ///
    /// Constructs a magic marker.
    pub fn new() -> Self {
        Self {}
    }
}

impl Reader<Magic> for Magic {
    /// 从缓冲区读 16 字节并校验等于 [`MAGIC`]，否则返回 `InvalidData`。
    ///
    /// Reads 16 bytes and verifies they equal [`MAGIC`]; returns
    /// `InvalidData` otherwise.
    fn read(buf: &mut ByteReader) -> Result<Magic, std::io::Error> {
        let mut magic = [0u8; 16];
        buf.read(&mut magic)?;

        if magic != MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid magic",
            ));
        }

        Ok(Magic)
    }
}

impl Writer for Magic {
    /// 向缓冲区写入 [`MAGIC`] 的 16 字节。
    ///
    /// Writes the 16 bytes of [`MAGIC`] into the buffer.
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        buf.write(&MAGIC)?;
        Ok(())
    }
}
