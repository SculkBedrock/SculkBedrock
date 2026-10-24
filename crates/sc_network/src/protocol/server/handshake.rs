use crate::utils::compression_algorithm::CompressionAlgorithm;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{BinaryIo, ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// NetworkSettingsPacket：协商压缩阈值/算法与客户端限流参数。
///
/// NetworkSettingsPacket: negotiates the compression threshold/algorithm
/// and client throttling parameters.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct NetworkSettings {
    /// 压缩阈值（字节）；小于该值不压缩。
    ///
    /// Compression threshold in bytes; smaller payloads are not compressed.
    pub compression_threshold: i16,
    /// 压缩算法。
    ///
    /// Compression algorithm.
    pub compression_algorithm: CompressionAlgorithm,
    /// 是否启用客户端限流。
    ///
    /// Whether client throttling is enabled.
    pub client_throttle_enabled: bool,
    /// 限流触发阈值。
    ///
    /// Throttle trigger threshold.
    pub client_throttle_threshold: i8,
    /// 限流标量。
    ///
    /// Throttle scalar.
    pub client_throttle_scalar: f32,
}

impl Reader<NetworkSettings> for NetworkSettings {
    /// 解码 NetworkSettings 包体。
    ///
    /// Decodes a NetworkSettings packet body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或数据损坏时返回解码错误。
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
    /// 写入 NetworkSettings 包体。
    ///
    /// Writes the NetworkSettings packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_i16_le(self.compression_threshold)?;
        self.compression_algorithm.write(buf)?;
        buf.write_bool(self.client_throttle_enabled)?;
        buf.write_i8(self.client_throttle_threshold)?;
        buf.write_f32_le(self.client_throttle_scalar)?;
        Ok(())
    }
}

/// ServerToClientHandshakePacket：登录握手第二阶段的服务端响应。
///
/// ServerToClientHandshakePacket: the server's second-stage login handshake
/// response.
#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct ServerToClientHandshake {
    /// 握手 JWT（含加密/身份协商数据）。
    ///
    /// Handshake JWT (carries encryption/identity negotiation data).
    pub jwt: String,
}

#[cfg(test)]
mod tests {
    use super::NetworkSettings;
    use crate::utils::compression_algorithm::CompressionAlgorithm;
    use sc_binary::interfaces::Writer;
    use sc_binary::ByteWriter;

    #[test]
    fn network_settings_matches_2168_zlib_layout() {
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
