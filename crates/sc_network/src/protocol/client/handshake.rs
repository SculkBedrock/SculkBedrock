use sc_binary::BinaryIo;
use sc_network_macros::MinecraftPacket;

#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct RequestNetworkSettings {
    pub protocol_version: u32,
}
#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct ClientToServerHandshake {}
