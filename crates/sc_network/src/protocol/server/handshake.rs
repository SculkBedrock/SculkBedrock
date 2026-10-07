use crate::utils::compression_algorithm::CompressionAlgorithm;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{BinaryIo, ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

#[derive(Clone, Debug, MinecraftPacket)]
pub struct NetworkSettings {
    pub compression_threshold: i16,
    pub compression_algorithm: CompressionAlgorithm,
    pub client_throttle_enabled: bool,
    pub client_throttle_threshold: i8,
    pub client_throttle_scalar: f32,
}

impl Reader<NetworkSettings> for NetworkSettings {
    fn read(buf: &mut ByteReader) -> Result<NetworkSettings, Error> {
        let compression_threshold = buf.read_i16_le()?;
        let compression_algorithm = CompressionAlgorithm::read(buf)?;
        let client_throttle_enabled = buf.read_bool()?;
        let client_throttle_threshold = buf.read_i8()?;
        let client_throttle_scalar = buf.read_f32_le()?;
        Ok(NetworkSettings {
            compression_threshold,
            compression_algorithm,
            client_throttle_enabled,
            client_throttle_threshold,
            client_throttle_scalar,
        })
    }
}

impl Writer for NetworkSettings {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_i16_le(self.compression_threshold)?;
        self.compression_algorithm.write(buf)?;
        buf.write_bool(self.client_throttle_enabled)?;
        buf.write_i8(self.client_throttle_threshold)?;
        buf.write_f32_le(self.client_throttle_scalar)?;
        Ok(())
    }
}

#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct ServerToClientHandshake {
    pub jwt: String,
}

#[cfg(test)]
mod tests {
    use super::NetworkSettings;
    use crate::utils::compression_algorithm::CompressionAlgorithm;
    use sc_binary::interfaces::Writer;
    use sc_binary::ByteWriter;

    #[test]
    fn network_settings_matches_pnx_2168_zlib_layout() {
        let packet = NetworkSettings {
            compression_threshold: 1,
            compression_algorithm: CompressionAlgorithm::Zlib,
            client_throttle_enabled: false,
            client_throttle_threshold: 0,
            client_throttle_scalar: 0.0,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();

        assert_eq!(
            writer.as_slice(),
            &[
                0x01, 0x00, // compression threshold
                0x00, 0x00, // zlib
                0x00, // client throttle disabled
                0x00, // client throttle threshold
                0x00, 0x00, 0x00, 0x00, // throttle scalar
            ]
        );
    }
}
