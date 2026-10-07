use crate::protocol::mcbe::UnconnectedPong;
use crate::protocol::packet::RakPacket;
use crate::protocol::{Magic, RAKNET_HEADER_FRAME_OVERHEAD};
use crate::register_packets;
use std::net::SocketAddr;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{BinaryIo, ByteReader, ByteWriter};

#[derive(Clone, Debug, BinaryIo)]
#[repr(u8)]
pub enum OfflinePacket {
    UnconnectedPing(UnconnectedPing) = 0x01,
    UnconnectedPong(UnconnectedPong) = 0x1c,
    OpenConnectRequest(OpenConnectRequest) = 0x05,
    OpenConnectReply(OpenConnectReply) = 0x06,
    SessionInfoRequest(SessionInfoRequest) = 0x07,
    SessionInfoReply(SessionInfoReply) = 0x08,
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

#[derive(Debug, Clone, BinaryIo)]
pub struct UnconnectedPing {
    pub timestamp: u64,
    pub magic: Magic,
    pub client_id: i64,
}

#[derive(Debug, Clone)]
pub struct OpenConnectRequest {
    pub protocol: u8,  // 9
    pub mtu_size: u16, // 500
}

impl Reader<OpenConnectRequest> for OpenConnectRequest {
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

#[derive(Debug, Clone, BinaryIo)]
pub struct OpenConnectReply {
    pub magic: Magic,
    pub server_id: u64,
    pub security: bool,
    pub mtu_size: u16,
}

#[derive(Debug, Clone, BinaryIo)]
pub struct SessionInfoRequest {
    pub magic: Magic,
    /// The socket address of the peer you are sending
    /// this packet to.
    pub address: SocketAddr,
    /// The mtu size of the peer you are sending this packet to.
    pub mtu_size: u16,
    /// Your internal client id.
    pub client_id: i64,
}

#[derive(Debug, Clone, BinaryIo)]
pub struct SessionInfoReply {
    pub magic: Magic,
    pub server_id: u64,
    pub client_address: SocketAddr,
    pub mtu_size: u16,
    pub security: bool,
}

#[derive(Debug, Clone, BinaryIo)]
pub struct IncompatibleProtocolVersion {
    pub protocol: u8,
    pub magic: Magic,
    pub server_id: u64,
}
