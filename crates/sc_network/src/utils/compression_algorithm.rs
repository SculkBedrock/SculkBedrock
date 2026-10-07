use flate2::bufread::{DeflateDecoder, DeflateEncoder};
use flate2::Compression;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use snap::read::{FrameDecoder, FrameEncoder};
use std::io::{Error, Read};

/// Maximum decompressed payload accepted for one compressed frame.
///
/// The Bedrock protocol caps a single logical packet well below this, so a
/// larger output is either corruption or a decompression bomb. Without a bound,
/// `read_to_end` would allocate whatever the payload claims (T18).
pub const MAX_DECOMPRESSED_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompressionAlgorithm {
    Zlib,
    Snappy,
}

impl CompressionAlgorithm {
    pub fn get_bytes(&self) -> u8 {
        match self {
            CompressionAlgorithm::Zlib => 0x00,
            CompressionAlgorithm::Snappy => 0x01,
        }
    }
    /// Decompress one frame, refusing output beyond [`MAX_DECOMPRESSED_BYTES`].
    ///
    /// The decoder is read through a `take` limit plus one byte, so an
    /// over-long payload is rejected *before* the full buffer is allocated.
    pub fn decode(&self, buf: &[u8]) -> Result<Vec<u8>, Error> {
        let mut vec = Vec::new();
        match self {
            CompressionAlgorithm::Zlib => {
                let mut reader = DeflateDecoder::new(buf);
                read_bounded(&mut reader, &mut vec)?;
            }
            CompressionAlgorithm::Snappy => {
                let mut frame_decoder = FrameDecoder::new(buf);
                read_bounded(&mut frame_decoder, &mut vec)?;
            }
        }
        Ok(vec)
    }

    pub fn encode(&self, buf: &[u8]) -> Result<Vec<u8>, Error> {
        match self {
            CompressionAlgorithm::Zlib => {
                let mut vec = vec![];
                let mut writer = DeflateEncoder::new(buf, Compression::default());
                match writer.read_to_end(&mut vec) {
                    Ok(_) => Ok(vec),
                    Err(error) => Err(error),
                }
            }
            CompressionAlgorithm::Snappy => {
                let mut frame_encoder = FrameEncoder::new(buf);
                let mut vec = vec![];
                frame_encoder.read_to_end(&mut vec)?;
                Ok(vec)
            }
        }
    }
}

/// Read at most [`MAX_DECOMPRESSED_BYTES`] into `out`.
fn read_bounded<R: Read>(reader: &mut R, out: &mut Vec<u8>) -> Result<(), Error> {
    let mut limited = reader.take(MAX_DECOMPRESSED_BYTES as u64 + 1);
    limited.read_to_end(out)?;
    if out.len() > MAX_DECOMPRESSED_BYTES {
        out.clear();
        out.shrink_to_fit();
        return Err(Error::other(
            "decompressed payload exceeds the per-frame byte budget",
        ));
    }
    Ok(())
}

impl Reader<CompressionAlgorithm> for CompressionAlgorithm {
    fn read(buf: &mut ByteReader) -> Result<CompressionAlgorithm, Error> {
        Ok(match buf.read_i16_le()? {
            0 => CompressionAlgorithm::Zlib,
            1 => CompressionAlgorithm::Snappy,
            _ => return Err(Error::other("Cannot read compression algorithm.")),
        })
    }
}

impl Writer for CompressionAlgorithm {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_i16_le(match self {
            CompressionAlgorithm::Zlib => 0,
            CompressionAlgorithm::Snappy => 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deflate(input: &[u8]) -> Vec<u8> {
        use flate2::write::DeflateEncoder;
        use std::io::Write as _;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(input).expect("compress");
        encoder.finish().expect("finish")
    }

    #[test]
    fn decode_round_trips_a_normal_payload() {
        let payload = b"ur chunk payload".repeat(16);
        let compressed = deflate(&payload);
        let decoded = CompressionAlgorithm::Zlib
            .decode(&compressed)
            .expect("decode");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn decode_refuses_output_beyond_the_budget() {
        let payload = vec![0u8; MAX_DECOMPRESSED_BYTES + 1];
        let compressed = deflate(&payload);
        let error = CompressionAlgorithm::Zlib
            .decode(&compressed)
            .expect_err("oversized decompression must be refused");
        assert!(error.to_string().contains("byte budget"));
    }

    #[test]
    fn decode_refuses_a_compression_bomb_without_buffering_it() {
        // A highly compressible payload expands far beyond the budget while
        // occupying very few input bytes.
        let payload = vec![0u8; MAX_DECOMPRESSED_BYTES * 4];
        let compressed = deflate(&payload);
        assert!(
            compressed.len() < MAX_DECOMPRESSED_BYTES / 8,
            "the fixture must be small on the wire"
        );
        assert!(CompressionAlgorithm::Zlib.decode(&compressed).is_err());
    }

    #[test]
    fn decode_rejects_malformed_input() {
        assert!(CompressionAlgorithm::Zlib
            .decode(&[0xFF, 0xFF, 0xFF])
            .is_err());
    }
}
