//! Bidirectional container window packets.

use std::io::Error;

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

/// ContainerClose (0x2f).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ContainerClose {
    pub window_id: u8,
    pub container_type: i8,
    pub was_server_initiated: bool,
}

impl Reader<ContainerClose> for ContainerClose {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            window_id: buf.read_u8()?,
            container_type: buf.read_u8()? as i8,
            was_server_initiated: buf.read_bool()?,
        })
    }
}

impl Writer for ContainerClose {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.window_id)?;
        buf.write_u8(self.container_type as u8)?;
        buf.write_bool(self.was_server_initiated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};

    #[test]
    fn container_close_round_trip() {
        let packet = ContainerClose {
            window_id: 0,
            container_type: -1,
            was_server_initiated: false,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), &[0, 0xff, 0]);

        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ContainerClose::read(&mut reader).unwrap();
        assert_eq!(decoded.window_id, 0);
        assert_eq!(decoded.container_type, -1);
        assert!(!decoded.was_server_initiated);
    }
}
