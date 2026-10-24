//! 客户端发往服务端的协议包定义。
//!
//! Client-to-server protocol packet definitions.

use sc_binary::BinaryIo;
use sc_network_macros::MinecraftPacket;

pub mod action;
pub mod command;
pub mod container;
pub mod crafting_request;
pub mod handshake;
pub mod interact;
pub mod login;
pub mod movement;
pub mod resource_packs;
pub mod transaction;
pub mod world;

/// 客户端缓存状态包（0x81），告知服务端客户端本地资源缓存的可用性。
///
/// Client cache status packet (0x81), telling the server whether the client
/// has usable local resource caches.
#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct ClientCacheStatus {
    /// 客户端是否已缓存全部所需资源。
    ///
    /// Whether the client has cached all required resources.
    pub supported: bool,
}
