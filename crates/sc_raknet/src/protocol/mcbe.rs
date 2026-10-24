use crate::protocol::Magic;
use sc_binary::BinaryIo;
use sc_utils::game::structs::motd::Motd;

/// 未连接 Pong：RakNet 层响应未连接 Ping 的服务端信息报文。
///
/// Unconnected Pong: the RakNet-level reply to an unconnected Ping,
/// carrying server information.
#[derive(Debug, Clone, BinaryIo)]
pub struct UnconnectedPong {
    /// 客户端 Ping 时携带、原样回显的时间戳。
    ///
    /// Timestamp echoed back from the client's ping.
    pub timestamp: u64,
    /// 服务端标识串（Minecraft Bedrock 通常填充为 0）。
    ///
    /// Server identifier (usually zero-filled on Minecraft Bedrock).
    pub server_id: u64,
    /// RakNet 离线魔数。
    ///
    /// The RakNet offline magic.
    pub magic: Magic,
    /// 服务端 MOTD（名称、协议版本、玩家数等）。
    ///
    /// Server MOTD (name, protocol version, player counts, ...).
    pub motd: Motd,
}
