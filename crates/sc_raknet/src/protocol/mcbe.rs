use crate::protocol::Magic;
use sc_binary::BinaryIo;
use sc_utils::game::structs::motd::Motd;

#[derive(Debug, Clone, BinaryIo)]
pub struct UnconnectedPong {
    pub timestamp: u64,
    pub server_id: u64,
    pub magic: Magic,
    pub motd: Motd,
}
