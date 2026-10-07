use crate::packet::batch_packet::{BatchPacket, OriginBatchPacket};
use crate::packet::dump;
use crate::packet::PacketEncryptionError;
use crate::utils::compression_algorithm::CompressionAlgorithm;
use crate::utils::encryption::RecvCipher;
use sc_binary::interfaces::Reader;
use sc_binary::ByteReader;

pub struct PackerDecoder {
    compression_algorithm: Option<CompressionAlgorithm>,
}

impl PackerDecoder {
    pub fn new() -> PackerDecoder {
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

    /// Synchronous decode: decryption ([`RecvCipher`]), then decompression,
    /// then protocol parsing.
    ///
    /// Crypto is microsecond-scale pure CPU work and stays synchronous: the
    /// caller passes `&mut RecvCipher` inside the `recv_cipher.write()`
    /// critical section, and the lock releases on synchronous return.
    pub fn decode(
        &self,
        packet_data: Vec<u8>,
        cipher: Option<&mut RecvCipher>,
    ) -> Result<BatchPacket, PacketEncryptionError> {
        self.decode0(packet_data, cipher)
    }

    pub fn decode_origin(
        &self,
        packet_data: Vec<u8>,
        cipher: Option<&mut RecvCipher>,
    ) -> Result<OriginBatchPacket, PacketEncryptionError> {
        self.decode0(packet_data, cipher)
    }

    fn decode0<T: Reader<T>>(
        &self,
        packet_data: Vec<u8>,
        cipher: Option<&mut RecvCipher>,
    ) -> Result<T, PacketEncryptionError> {
        dump::record_wire("in", None, "bedrock_wire", &packet_data, "batch");
        let mut data = packet_data.clone();
        if let Some(encryption) = cipher {
            let temp = encryption
                .decode(&packet_data[1..])
                .map_err(|e| PacketEncryptionError::IoError(e))?;
            data = vec![0xfe];
            data.extend(temp);
        }
        if let Some(compression) = self.compression_algorithm {
            if let Some(batch_compressed) = data.get(1..) {
                dump::record_wire(
                    "in",
                    None,
                    "batch_compressed",
                    batch_compressed,
                    "compressed",
                );
            }
            let bytes = match &data[1] {
                0 | 1 => {
                    // Zlib or Snappy compression.
                    compression
                        .decode(&data[2..])
                        .map_err(|e| PacketEncryptionError::IoError(e))?
                }
                255 => {
                    // Uncompressed payload.
                    data[2..].to_vec()
                }
                _ => {
                    // Unsupported compression format.
                    return Err(PacketEncryptionError::UnsupportedCompressor);
                }
            };
            // Rebuild the transport prefix.
            let mut vec = vec![0xfe];
            vec.extend(bytes);
            data = vec;
        }
        if let Some(batch_plain) = data.get(1..) {
            dump::record_wire("in", None, "batch_plain", batch_plain, "uncompressed");
        }
        Ok(T::read(&mut ByteReader::from(data))
            .map_err(|e| PacketEncryptionError::PacketReadError(e))?)
    }
}
