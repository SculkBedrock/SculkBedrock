use std::error::Error;
use std::fmt::Display;
use std::io;

pub mod batch_packet;
pub mod decoder;
pub mod dump;
pub mod encoder;
pub mod raw_batch;

#[derive(Debug)]
pub enum PacketEncryptionError {
    UnsupportedCompressor,
    IoError(io::Error),
    PacketReadError(io::Error),
}

impl Display for PacketEncryptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for PacketEncryptionError {}
