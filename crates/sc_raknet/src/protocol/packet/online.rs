use crate::protocol::packet::RakPacket;
use crate::register_packets;
use std::net::SocketAddr;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{BinaryIo, ByteReader, ByteWriter};

/// 在线报文枚举：连接建立后的 RakNet 控制报文。
///
/// 判别值即报文 ID。
///
/// Online packet enum: RakNet control messages after the connection is
/// established.
///
/// The discriminants are the packet IDs.
#[derive(BinaryIo, Clone, Debug)]
#[repr(u8)]
pub enum OnlinePacket {
    /// 连接保活 Ping（ID 0x00）。
    ///
    /// Keep-alive ping (ID 0x00).
    ConnectedPing(ConnectedPing) = 0x00,
    /// 连接保活 Pong（ID 0x03）。
    ///
    /// Keep-alive pong (ID 0x03).
    ConnectedPong(ConnectedPong) = 0x03,
    /// 连接丢失通知（ID 0x04）。
    ///
    /// Connection lost notification (ID 0x04).
    LostConnection(LostConnection) = 0x04,
    /// 连接请求（ID 0x09）。
    ///
    /// Connection request (ID 0x09).
    ConnectionRequest(ConnectionRequest) = 0x09,
    /// 连接接受（ID 0x10）。
    ///
    /// Connection accepted (ID 0x10).
    ConnectionAccept(ConnectionAccept) = 0x10,
    /// 新连接通知（ID 0x13）。
    ///
    /// New connection notification (ID 0x13).
    NewConnection(NewConnection) = 0x13,
    /// 断开连接通知（ID 0x15）。
    ///
    /// Disconnect notification (ID 0x15).
    Disconnect(Disconnect) = 0x15,
}

register_packets! {
    Online is OnlinePacket,
    ConnectedPing,
    ConnectedPong,
    LostConnection,
    ConnectionRequest,
    ConnectionAccept,
    NewConnection,
    Disconnect
}

/// 连接 Ping：向对端发送本端时间戳。
///
/// Connected Ping: sends the local timestamp to the peer.
#[derive(Clone, Debug, BinaryIo)]
pub struct ConnectedPing {
    /// 本报文发送给对端的时间。
    ///
    /// The time you sent the packet to the peer.
    pub time: i64,
}

/// 连接 Pong：回显收到的 Ping 时间与本端发送时间。
///
/// Connected Pong: echoes the received ping time and the local send time.
#[derive(Clone, Debug, BinaryIo)]
pub struct ConnectedPong {
    /// 对端发送 [`ConnectedPing`] 的时间。
    ///
    /// The time that the peer sent the [`ConnectedPing`] packet.
    pub ping_time: i64,
    /// 本端发送 [`ConnectedPong`] 给对端的时间。
    ///
    /// The time that you sent the [`ConnectedPong`] packet to the peer.
    pub pong_time: i64,
}

/// 连接请求：客户端申请加入连接。
///
/// Connection request: a client asks to join the connection.
#[derive(Clone, Debug, BinaryIo)]
pub struct ConnectionRequest {
    /// 客户端标识。
    ///
    /// The client identifier.
    pub client_id: i64,
    /// 客户端发起请求的时间。
    ///
    /// The time the request was made.
    pub time: i64,
    /// 是否启用安全层（Bedrock 为 false）。
    ///
    /// Whether the security layer is enabled (false on Bedrock).
    pub security: bool,
}

/// 连接接受：服务端确认连接并回送地址与时间信息。
///
/// Connection accepted: the server confirms the connection and echoes
/// address and time information.
#[derive(Clone, Debug)]
pub struct ConnectionAccept {
    /// 连入客户端的地址。
    ///
    /// The address of the client connecting.
    pub client_address: SocketAddr,
    /// 服务端的系统索引。
    ///
    /// The system index of the server.
    pub system_index: i16,
    /// 服务端的内部 ID 列表（备用地址）：客户端无法直连服务端时使用。
    ///
    /// The internal ids of the server or alternative IPs of the server:
    /// addresses the client will use if it can't connect to the server.
    pub internal_ids: Vec<SocketAddr>,
    /// 客户端 `ConnectionRequest` 中携带的时间戳。
    ///
    /// The time of the timestamp the client sent with `ConnectionRequest`.
    pub request_time: i64,
    /// 服务端当前时间。
    ///
    /// The time on the server.
    pub timestamp: i64,
}

impl Reader<ConnectionAccept> for ConnectionAccept {
    /// 依次读取客户端地址、系统索引、至多 20 个内部地址与两个时间戳。
    ///
    /// Reads the client address, system index, up to 20 internal
    /// addresses, and the two timestamps in order.
    fn read(buf: &mut ByteReader) -> std::io::Result<Self> {
        let client_address = buf.read_type::<SocketAddr>()?;

        // read the system index, this is
        let system_index = buf.read_i16()?;
        let mut internal_ids = Vec::<SocketAddr>::new();

        for _ in 0..20 {
            // Only read another 7-byte address if the request_time + timestamp
            // (16 bytes) can still follow it; otherwise stop so the trailing
            // i64 pair parses cleanly.
            if buf.as_slice().len() < 7 + 16 {
                break;
            }
            internal_ids.push(buf.read_type::<SocketAddr>()?);
        }

        let request_time = buf.read_i64()?;
        let timestamp = buf.read_i64()?;

        Ok(Self {
            client_address,
            system_index,
            internal_ids,
            request_time,
            timestamp,
        })
    }
}

impl Writer for ConnectionAccept {
    /// 依次写入客户端地址、系统索引、内部地址列表与两个时间戳。
    ///
    /// 内部地址超过 20 个时返回 `InvalidInput`。
    ///
    /// Writes the client address, system index, internal address list,
    /// and the two timestamps in order.
    ///
    /// Returns `InvalidInput` when there are more than 20 internal ids.
    fn write(&self, buf: &mut ByteWriter) -> std::io::Result<()> {
        buf.write_type::<SocketAddr>(&self.client_address)?;
        buf.write_i16(self.system_index)?;

        if self.internal_ids.len() > 20 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Too many internal id's",
            ));
        }

        for internal_id in &self.internal_ids {
            buf.write_type::<SocketAddr>(internal_id)?;
        }

        buf.write_i64(self.request_time)?;
        buf.write_i64(self.timestamp)?;

        Ok(())
    }
}

/// 新连接通知：客户端告知服务端连接已建立。
///
/// New connection notification: the client tells the server the connection
/// is established.
#[derive(Clone, Debug)]
pub struct NewConnection {
    /// 服务端的外部 IP 地址。
    ///
    /// The external IP address of the server.
    pub server_address: SocketAddr,
    /// 服务端的内部 IP 地址列表。
    ///
    /// The internal IP addresses of the server.
    pub system_address: Vec<SocketAddr>,
    /// 客户端 `ConnectionRequest` 中携带的时间戳。
    ///
    /// The time of the timestamp the client sent with `ConnectionRequest`.
    pub request_time: i64,
    /// 服务端当前时间。
    ///
    /// The time on the server.
    pub timestamp: i64,
}

impl Reader<NewConnection> for NewConnection {
    /// 读取服务端地址、至多 20 个系统地址与两个时间戳。
    ///
    /// Reads the server address, up to 20 system addresses, and the two
    /// timestamps.
    fn read(buf: &mut ByteReader) -> std::io::Result<Self> {
        let server_address = buf.read_type::<SocketAddr>()?;

        let mut system_address = Vec::<SocketAddr>::new();

        for _ in 0..20 {
            // Only read another 7-byte address if the request_time + timestamp
            // (16 bytes) can still follow it; otherwise stop so the trailing
            // i64 pair parses cleanly. (Prismarine clients send 10 system
            // addresses; the old `< 16` condition consumed one address too
            // many and made the final i64 read fail, so NewIncomingConnection
            // frames were mis-forwarded to the game layer.)
            if buf.as_slice().len() < 7 + 16 {
                break;
            }
            system_address.push(buf.read_type::<SocketAddr>()?);
        }

        let request_time = buf.read_i64()?;
        let timestamp = buf.read_i64()?;

        Ok(Self {
            server_address,
            system_address,
            request_time,
            timestamp,
        })
    }
}

impl Writer for NewConnection {
    /// 依次写入服务端地址、系统地址列表与两个时间戳。
    ///
    /// 系统地址超过 20 个时返回 `InvalidInput`。
    ///
    /// Writes the server address, system address list, and the two
    /// timestamps in order.
    ///
    /// Returns `InvalidInput` when there are more than 20 addresses.
    fn write(&self, buf: &mut ByteWriter) -> std::io::Result<()> {
        buf.write_type::<SocketAddr>(&self.server_address)?;

        if self.system_address.len() > 20 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Too many internal id's",
            ));
        }

        for system_address in &self.system_address {
            buf.write_type::<SocketAddr>(system_address)?;
        }

        buf.write_i64(self.request_time)?;
        buf.write_i64(self.timestamp)?;

        Ok(())
    }
}

/// 断开连接通知：告知客户端断开连接。
///
/// A disconnect notification: tells the client to disconnect.
#[derive(Clone, Debug, BinaryIo)]
pub struct Disconnect {}

/// 连接丢失通知：客户端失去与服务端的连接时发送。
///
/// A connection lost notification, sent by the client when it loses
/// connection to the server.
#[derive(Clone, Debug, BinaryIo)]
pub struct LostConnection {}
