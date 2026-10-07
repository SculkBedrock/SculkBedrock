use crate::protocol::packet::offline::OfflinePacket;
use crate::protocol::packet::online::OnlinePacket;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};

pub mod offline;
pub mod online;

#[derive(Debug, Clone)]
pub enum RakPacket {
    Offline(OfflinePacket),
    Online(OnlinePacket),
}

impl RakPacket {
    pub fn is_online(&self) -> bool {
        match self {
            RakPacket::Online(_) => true,
            _ => false,
        }
    }

    pub fn get_offline(&self) -> Option<&OfflinePacket> {
        match self {
            RakPacket::Offline(packet) => Some(packet),
            _ => None,
        }
    }

    pub fn get_online(&self) -> Option<&OnlinePacket> {
        match self {
            RakPacket::Online(packet) => Some(packet),
            _ => None,
        }
    }
}

impl Writer for RakPacket {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        match self {
            RakPacket::Offline(packet) => packet.write(buf),
            RakPacket::Online(packet) => packet.write(buf),
        }
    }
}

impl Reader<RakPacket> for RakPacket {
    fn read(buf: &mut ByteReader) -> Result<RakPacket, std::io::Error> {
        if let Ok(packet) = OfflinePacket::read(buf) {
            return Ok(RakPacket::Offline(packet));
        }
        if let Ok(packet) = OnlinePacket::read(buf) {
            return Ok(RakPacket::Online(packet));
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid packet",
        ))
    }
}

impl From<OfflinePacket> for RakPacket {
    fn from(packet: OfflinePacket) -> Self {
        RakPacket::Offline(packet)
    }
}

impl From<OnlinePacket> for RakPacket {
    fn from(packet: OnlinePacket) -> Self {
        RakPacket::Online(packet)
    }
}

impl From<RakPacket> for OnlinePacket {
    fn from(packet: RakPacket) -> Self {
        match packet {
            RakPacket::Online(packet) => packet,
            _ => panic!("Invalid packet conversion"),
        }
    }
}

impl From<RakPacket> for OfflinePacket {
    fn from(packet: RakPacket) -> Self {
        match packet {
            RakPacket::Offline(packet) => packet,
            _ => panic!("Invalid packet conversion"),
        }
    }
}

#[macro_export]
macro_rules! register_packets {
    ($name: ident is $kind: ident, $($packet: ident),*) => {
        $(
            impl From<$packet> for $kind {
                fn from(packet: $packet) -> Self {
                    $kind::$packet(packet)
                }
            }

            impl From<$packet> for RakPacket {
                fn from(packet: $packet) -> Self {
                    $kind::$packet(packet).into()
                }
            }

            impl From<$kind> for $packet {
                fn from(packet: $kind) -> Self {
                    match packet {
                        $kind::$packet(packet) => packet.into(),
                        _ => panic!("Invalid packet conversion"),
                    }
                }
            }

            impl From<RakPacket> for $packet {
                fn from(packet: RakPacket) -> Self {
                    match packet {
                        RakPacket::$name(packet) => packet.into(),
                        _ => panic!("Invalid packet conversion"),
                    }
                }
            }
        )*
    };
}
