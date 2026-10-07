use crate::packet::batch_packet::{BatchPacket, OriginBatchPacket};
use crate::packet::dump;
use crate::packet::raw_batch::RawBatchPacket;
use crate::packet::PacketEncryptionError;
use crate::utils::compression_algorithm::CompressionAlgorithm;
use crate::utils::encryption::SendCipher;
use sc_binary::interfaces::Writer;
use std::io::Error;

pub struct PackerEncoder {
    compression_algorithm: Option<CompressionAlgorithm>,
}

impl PackerEncoder {
    pub fn new() -> Self {
        Self {
            compression_algorithm: None,
        }
    }

    pub fn set_compression_algorithm(
        &mut self,
        compression_algorithm: Option<CompressionAlgorithm>,
    ) {
        self.compression_algorithm = compression_algorithm;
    }

    /// Synchronous encode: protocol serialization, then compression, then
    /// encryption ([`SendCipher`]).
    ///
    /// Crypto is microsecond-scale pure CPU work and stays synchronous: the
    /// caller passes `&mut SendCipher` inside the `send_cipher.write()`
    /// critical section, and the lock releases on synchronous return.
    pub fn encode(
        &self,
        packet: BatchPacket,
        cipher: Option<&mut SendCipher>,
    ) -> Result<Vec<u8>, PacketEncryptionError> {
        self.encode0(packet, cipher)
    }

    pub fn encode_origin(
        &self,
        packet: OriginBatchPacket,
        cipher: Option<&mut SendCipher>,
    ) -> Result<Vec<u8>, PacketEncryptionError> {
        self.encode0(packet, cipher)
    }

    /// Shared pre-serialized batch encoding (login constant-packet cache):
    /// same framing as [`BatchPacket`], with bodies from cached
    /// `Arc<Vec<u8>>` (no per-player rebuild or re-serialization).
    pub fn encode_raw(
        &self,
        packet: RawBatchPacket,
        cipher: Option<&mut SendCipher>,
    ) -> Result<Vec<u8>, PacketEncryptionError> {
        self.encode0(packet, cipher)
    }

    fn encode0<T: Writer>(
        &self,
        packet: T,
        cipher: Option<&mut SendCipher>,
    ) -> Result<Vec<u8>, PacketEncryptionError> {
        Ok(Self::finish(self.prepare(packet)?, cipher))
    }

    pub(crate) fn prepare<T: Writer>(&self, packet: T) -> Result<Vec<u8>, PacketEncryptionError> {
        let serialized = packet
            .write_to_bytes()
            .map_err(|e| PacketEncryptionError::IoError(e))?;
        let serialized = serialized.as_slice();
        let batch_plain = serialized.get(1..).ok_or_else(|| {
            PacketEncryptionError::IoError(Error::other("encoded packet batch is empty"))
        })?;
        dump::record_wire("out", None, "batch_plain", batch_plain, "uncompressed");

        // Compress directly from the serialized writer buffer. The previous
        // path first cloned the complete batch, then cloned it again without
        // its 0xfe prefix before compression.
        let bytes = if let Some(compression_algorithm) = self.compression_algorithm {
            let mut compressed = compression_algorithm
                .encode(batch_plain)
                .map_err(|e| PacketEncryptionError::IoError(e))?;
            compressed.reserve(1);
            compressed.insert(0, compression_algorithm.get_bytes());
            dump::record_wire("out", None, "batch_compressed", &compressed, "compressed");
            compressed
        } else {
            batch_plain.to_vec()
        };
        Ok(bytes)
    }

    pub(crate) fn finish(mut bytes: Vec<u8>, cipher: Option<&mut SendCipher>) -> Vec<u8> {
        if let Some(encryption) = cipher {
            bytes = encryption.encode(&bytes);
        }
        // Restore the transport prefix in place rather than allocating a
        // second full-size vector and copying the encoded body into it.
        bytes.reserve(1);
        bytes.insert(0, 0xfe);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::batch_packet::BatchPacket;
    use crate::protocol::server::login::PlayStatus;
    use crate::utils::compression_algorithm::CompressionAlgorithm;
    use crate::utils::encryption::SendCipher;

    fn sample_batch() -> BatchPacket {
        BatchPacket::single(PlayStatus { status: 3 })
    }

    fn legacy_encode(
        batch: BatchPacket,
        compression: Option<CompressionAlgorithm>,
        protocol_version: u32,
        key: Vec<u8>,
    ) -> Vec<u8> {
        let serialized = batch.write_to_bytes().expect("serialize legacy batch");
        let mut bytes = serialized.as_slice()[1..].to_vec();
        if let Some(algorithm) = compression {
            let mut framed = vec![algorithm.get_bytes()];
            framed.extend(algorithm.encode(&bytes).expect("legacy compression"));
            bytes = framed;
        }
        let mut cipher = SendCipher::new(key, protocol_version).expect("create legacy cipher");
        bytes = cipher.encode(&bytes);
        let mut result = vec![0xfe];
        result.extend(bytes);
        result
    }

    #[test]
    fn uncompressed_encoding_matches_serialized_batch_bytes() {
        let batch = sample_batch();
        let expected = batch
            .clone()
            .write_to_bytes()
            .expect("serialize expected batch")
            .as_slice()
            .to_vec();

        let actual = PackerEncoder::new()
            .encode(batch, None)
            .expect("encode uncompressed batch");

        assert_eq!(actual, expected);
    }

    #[test]
    fn compressed_encoding_keeps_transport_and_algorithm_prefixes() {
        for algorithm in [CompressionAlgorithm::Zlib, CompressionAlgorithm::Snappy] {
            let batch = sample_batch();
            let expected = batch
                .clone()
                .write_to_bytes()
                .expect("serialize expected batch");
            let mut encoder = PackerEncoder::new();
            encoder.set_compression_algorithm(Some(algorithm));

            let encoded = encoder
                .encode(batch, None)
                .expect("encode compressed batch");

            assert_eq!(encoded.first(), Some(&0xfe));
            assert_eq!(encoded.get(1), Some(&algorithm.get_bytes()));
            let decoded = algorithm
                .decode(&encoded[2..])
                .expect("decompress framed batch");
            assert_eq!(decoded, expected.as_slice()[1..]);
        }
    }

    #[test]
    fn encrypted_encoding_matches_legacy_bytes_for_both_cipher_profiles() {
        let key: Vec<u8> = (0u8..32).collect();
        for protocol_version in [400, 685] {
            for compression in [
                None,
                Some(CompressionAlgorithm::Zlib),
                Some(CompressionAlgorithm::Snappy),
            ] {
                let expected =
                    legacy_encode(sample_batch(), compression, protocol_version, key.clone());
                let mut encoder = PackerEncoder::new();
                encoder.set_compression_algorithm(compression);
                let mut cipher =
                    SendCipher::new(key.clone(), protocol_version).expect("create current cipher");

                let actual = encoder
                    .encode(sample_batch(), Some(&mut cipher))
                    .expect("encode encrypted batch");

                assert_eq!(
                    actual, expected,
                    "protocol={protocol_version}, {compression:?}"
                );
            }
        }
    }
}
