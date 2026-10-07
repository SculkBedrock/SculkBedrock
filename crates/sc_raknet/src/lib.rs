//! Pure-Rust RakNet transport layer.
//!
//! `Listener` binds a UDP port; `Connection` holds per-client state
//! (reliable/unreliable ordering, ACKs, fragments, RTT, health). The upper
//! layer (sc_network) adds encryption/compression and Minecraft codecs.

pub mod connection;
pub mod dump;
pub mod notify;
pub mod protocol;
pub mod server;
#[cfg(test)]
pub mod tests;
pub mod utils;
