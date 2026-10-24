//! 客户端握手与网络设置请求包。
//!
//! Client handshake and network settings request packets.

use sc_binary::BinaryIo;
use sc_network_macros::MinecraftPacket;

/// 请求网络设置包（0xc1），客户端在登录阶段索取压缩阈值等网络参数。
///
/// Request network settings packet (0xc1), where the client asks for network
/// parameters such as the compression threshold during the login phase.
#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct RequestNetworkSettings {
    /// 客户端使用的协议版本号。
    ///
    /// The protocol version used by the client.
    pub protocol_version: u32,
}

/// 客户端到服务端的握手包（0x04），标志登录流程开始。
///
/// Client-to-server handshake packet (0x04), marking the start of the login
/// flow.
#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct ClientToServerHandshake {}
