//! RakNet 传输层的纯 Rust 实现。
//!
//! Pure-Rust RakNet transport layer.
//!
//! `Listener` 绑定 UDP 端口；`Connection` 维护每个客户端的状态
//! （可靠/不可靠排序、ACK、分片、RTT、健康检查）。上层（sc_network）
//! 在此基础上叠加加密/压缩与 Minecraft 编解码器。

pub mod connection;
pub mod dump;
pub mod notify;
pub mod protocol;
pub mod server;
pub mod utils;
