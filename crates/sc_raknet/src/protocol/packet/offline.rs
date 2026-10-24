use crate::protocol::mcbe::UnconnectedPong;
use crate::protocol::packet::RakPacket;
use crate::protocol::{Magic, RAKNET_HEADER_FRAME_OVERHEAD};
use crate::register_packets;
use std::net::SocketAddr;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{BinaryIo, ByteReader, ByteWriter};

/// 离线报文枚举：RakNet 握手阶段的所有报文类型。
///
/// 判别值即报文 ID。
///
/// Offline packet enum: every packet type of the RakNet handshake phase.
///
/// The discriminants are the packet IDs.
#[derive(Clone, Debug, BinaryIo)]
#[repr(u8)]
pub enum OfflinePacket {
    /// 未连接 Ping（ID 0x01）。
    ///
    /// Unconnected Ping (ID 0x01).
    UnconnectedPing(UnconnectedPing) = 0x01,
    /// 未连接 Pong（ID 0x1c）。
    ///
    /// Unconnected Pong (ID 0x1c).
    UnconnectedPong(UnconnectedPong) = 0x1c,
    /// 打开连接请求（ID 0x05）。
    ///
    /// Open connection request (ID 0x05).
    OpenConnectRequest(OpenConnectRequest) = 0x05,
    /// 打开连接响应（ID 0x06）。
    ///
    /// Open connection reply (ID 0x06).
    OpenConnectReply(OpenConnectReply) = 0x06,
    /// 会话信息请求（ID 0x07）。
    ///
    /// Session info request (ID 0x07).
    SessionInfoRequest(SessionInfoRequest) = 0x07,
    /// 会话信息响应（ID 0x08）。
    ///
    /// Session info reply (ID 0x08).
    SessionInfoReply(SessionInfoReply) = 0x08,
    /// 协议版本不兼容（ID 0x19）。
    ///
    /// Incompatible protocol version (ID 0x19).
    IncompatibleProtocolVersion(IncompatibleProtocolVersion) = 0x19,
}

register_packets! {
    Offline is OfflinePacket,
    UnconnectedPing,
    UnconnectedPong,
    OpenConnectRequest,
    OpenConnectReply,
    SessionInfoRequest,
    SessionInfoReply,
    IncompatibleProtocolVersion
}

/// 未连接 Ping：客户端探测服务端是否存在。
///
/// Unconnected Ping: a client probes whether the server exists.
#[derive(Debug, Clone, BinaryIo)]
pub struct UnconnectedPing {
    /// 客户端生成、由服务端在 Pong 中回显的时间戳。
    ///
    /// Client-generated timestamp echoed back by the server in the Pong.
    pub timestamp: u64,
    /// RakNet 离线魔数。
    ///
    /// The RakNet offline magic.
    pub magic: Magic,
    /// 客户端标识。
    ///
    /// The client identifier.
    pub client_id: i64,
}

/// 打开连接请求：客户端申请建立连接并探测 MTU。
///
/// Open connection request: a client asks to establish a connection and
/// probes the MTU.
#[derive(Debug, Clone)]
pub struct OpenConnectRequest {
    /// 客户端使用的 RakNet 协议版本（当前 Bedrock 为 9）。
    ///
    /// The RakNet protocol version used by the client (9 on current
    /// Bedrock).
    pub protocol: u8,
    /// 客户端可支持的 MTU 大小（Bedrock 通常为 500 起）。
    ///
    /// The MTU size supported by the client (usually starting at 500 on
    /// Bedrock).
    pub mtu_size: u16,
}

impl Reader<OpenConnectRequest> for OpenConnectRequest {
    /// 读取协议号，并按剩余报文长度推算客户端 MTU。
    ///
    /// Reads the protocol number and infers the client MTU from the
    /// remaining datagram length.
    fn read(buf: &mut ByteReader) -> Result<OpenConnectRequest, std::io::Error> {
        let len = buf.as_slice().len();
        buf.read_type::<Magic>()?;
        Ok(OpenConnectRequest {
            protocol: buf.read_u8()?,
            mtu_size: (len + RAKNET_HEADER_FRAME_OVERHEAD as usize) as u16,
        })
    }
}

impl Writer for OpenConnectRequest {
    /// 写入协议号，并按 MTU 差值补零填充。
    ///
    /// Writes the protocol number and zero-pads according to the MTU
    /// difference.
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        buf.write_type::<Magic>(&Magic::new())?;
        buf.write_u8(self.protocol)?;
        // padding
        // remove 28 bytes from the mtu size
        let mtu_size = self.mtu_size.saturating_sub(RAKNET_HEADER_FRAME_OVERHEAD);
        for _ in 0..mtu_size {
            buf.write_u8(0)?;
        }
        Ok(())
    }
}

/// 打开连接响应：服务端接受连接并通告协商后的 MTU。
///
/// Open connection reply: the server accepts the connection and announces
/// the negotiated MTU.
#[derive(Debug, Clone, BinaryIo)]
pub struct OpenConnectReply {
    /// RakNet 离线魔数。
    ///
    /// The RakNet offline magic.
    pub magic: Magic,
    /// 服务端标识。
    ///
    /// The server identifier.
    pub server_id: u64,
    /// 是否启用安全层（Bedrock 为 false）。
    ///
    /// Whether the security layer is enabled (false on Bedrock).
    pub security: bool,
    /// 协商后的 MTU 大小。
    ///
    /// The negotiated MTU size.
    pub mtu_size: u16,
}

/// 会话信息请求：连接建立后交换对端地址与 MTU。
///
/// Session info request: exchanges the peer address and MTU after the
/// connection is established.
#[derive(Debug, Clone, BinaryIo)]
pub struct SessionInfoRequest {
    /// RakNet 离线魔数。
    ///
    /// The RakNet offline magic.
    pub magic: Magic,
    /// 本报文发送目标的套接字地址。
    ///
    /// The socket address of the peer this packet is being sent to.
    pub address: SocketAddr,
    /// 报文发送目标的 MTU 大小。
    ///
    /// The MTU size of the peer this packet is being sent to.
    pub mtu_size: u16,
    /// 本端内部使用的客户端 ID。
    ///
    /// Your internal client id.
    pub client_id: i64,
}

/// 会话信息响应：回送服务端标识、对端地址与 MTU。
///
/// Session info reply: echoes the server identifier, peer address, and
/// MTU.
#[derive(Debug, Clone, BinaryIo)]
pub struct SessionInfoReply {
    /// RakNet 离线魔数。
    ///
    /// The RakNet offline magic.
    pub magic: Magic,
    /// 服务端标识。
    ///
    /// The server identifier.
    pub server_id: u64,
    /// 客户端地址。
    ///
    /// The client address.
    pub client_address: SocketAddr,
    /// 协商后的 MTU 大小。
    ///
    /// The negotiated MTU size.
    pub mtu_size: u16,
    /// 是否启用安全层（Bedrock 为 false）。
    ///
    /// Whether the security layer is enabled (false on Bedrock).
    pub security: bool,
}

/// 协议版本不兼容：服务端拒绝客户端版本时返回。
///
/// Incompatible protocol version: returned when the server rejects the
/// client version.
#[derive(Debug, Clone, BinaryIo)]
pub struct IncompatibleProtocolVersion {
    /// 服务端使用的 RakNet 协议版本。
    ///
    /// The RakNet protocol version used by the server.
    pub protocol: u8,
    /// RakNet 离线魔数。
    ///
    /// The RakNet offline magic.
    pub magic: Magic,
    /// 服务端标识。
    ///
    /// The server identifier.
    pub server_id: u64,
}
