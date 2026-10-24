use std::error::Error;
use std::fmt::Display;
use std::io;

use sc_error::{capture_site, ScError, ScSite};

pub mod batch_packet;
pub mod decoder;
pub mod dump;
pub mod encoder;
pub mod raw_batch;

#[derive(Debug, ScError)]
pub enum PacketEncryptionError {
    /// 不支持的压缩算法判别值。
    ///
    /// Unsupported compression algorithm discriminant.
    #[sc_error(code = "network.packet.unsupported_compressor")]
    UnsupportedCompressor {
        /// 说明错误原因的细节。
        ///
        /// Detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 底层 I/O 失败。
    ///
    /// Underlying I/O failure.
    #[sc_error(code = "network.packet.io")]
    IoError {
        /// 底层 I/O 错误。
        ///
        /// Underlying I/O error.
        source: io::Error,
        /// 说明错误原因的细节。
        ///
        /// Detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
    /// 数据包体解析失败。
    ///
    /// Packet body read failure.
    #[sc_error(code = "network.packet.read")]
    PacketReadError {
        /// 底层 I/O 错误。
        ///
        /// Underlying I/O error.
        source: io::Error,
        /// 说明错误原因的细节。
        ///
        /// Detail explaining the cause.
        detail: String,
        /// 构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
}

impl PacketEncryptionError {
    /// 构造一个 unsupported-compressor 错误，并捕获调用位置。
    ///
    /// Build an unsupported-compressor error, capturing the call site.
    #[track_caller]
    pub fn unsupported_compressor(detail: String) -> Self {
        Self::UnsupportedCompressor {
            detail,
            site: capture_site(),
        }
    }

    /// 构造一个 I/O 错误，并捕获调用位置。
    ///
    /// Build an I/O error, capturing the call site.
    #[track_caller]
    pub fn io(source: io::Error) -> Self {
        let detail = source.to_string();
        Self::IoError {
            source,
            detail,
            site: capture_site(),
        }
    }

    /// 构造一个 packet-read 错误，并捕获调用位置。
    ///
    /// Build a packet-read error, capturing the call site.
    #[track_caller]
    pub fn packet_read(source: io::Error) -> Self {
        let detail = source.to_string();
        Self::PacketReadError {
            source,
            detail,
            site: capture_site(),
        }
    }
}

impl Display for PacketEncryptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedCompressor { detail, site } => {
                write!(
                    f,
                    "[network.packet.unsupported_compressor] {detail} (at {site})"
                )
            }
            Self::IoError { detail, site, .. } => {
                write!(f, "[network.packet.io] {detail} (at {site})")
            }
            Self::PacketReadError { detail, site, .. } => {
                write!(f, "[network.packet.read] {detail} (at {site})")
            }
        }
    }
}

impl Error for PacketEncryptionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IoError { source, .. } => Some(source),
            Self::PacketReadError { source, .. } => Some(source),
            _ => None,
        }
    }
}


