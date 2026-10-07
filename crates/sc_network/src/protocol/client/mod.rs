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

#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct ClientCacheStatus {
    pub supported: bool,
}
