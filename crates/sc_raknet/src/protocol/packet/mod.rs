use crate::protocol::packet::offline::OfflinePacket;
use crate::protocol::packet::online::OnlinePacket;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};

/// 离线报文定义（握手阶段）。
///
/// Offline packet definitions (handshake phase).
pub mod offline;
/// 在线报文定义（连接建立后）。
///
/// Online packet definitions (after the connection is established).
pub mod online;

/// RakNet 报文的顶层枚举：离线或在线。
///
/// Top-level RakNet packet enum: offline or online.
#[derive(Debug, Clone)]
pub enum RakPacket {
    /// 离线报文变体。
    ///
    /// The offline packet variant.
    Offline(OfflinePacket),
    /// 在线报文变体。
    ///
    /// The online packet variant.
    Online(OnlinePacket),
}

impl RakPacket {
    /// 是否为在线报文。
    ///
    /// Whether this is an online packet.
    pub fn is_online(&self) -> bool {
        match self {
            RakPacket::Online(_) => true,
            _ => false,
        }
    }

    /// 取离线报文引用；非离线报文返回 `None`。
    ///
    /// Borrows the offline packet; `None` when not offline.
    pub fn get_offline(&self) -> Option<&OfflinePacket> {
        match self {
            RakPacket::Offline(packet) => Some(packet),
            _ => None,
        }
    }

    /// 取在线报文引用；非在线报文返回 `None`。
    ///
    /// Borrows the online packet; `None` when not online.
    pub fn get_online(&self) -> Option<&OnlinePacket> {
        match self {
            RakPacket::Online(packet) => Some(packet),
            _ => None,
        }
    }
}

impl Writer for RakPacket {
    /// 按变体分派到底层报文的写逻辑。
    ///
    /// Dispatches to the underlying packet's write logic by variant.
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        match self {
            RakPacket::Offline(packet) => packet.write(buf),
            RakPacket::Online(packet) => packet.write(buf),
        }
    }
}

impl Reader<RakPacket> for RakPacket {
    /// 依次尝试按离线、在线报文解析；都失败时返回 `InvalidData`。
    ///
    /// Tries offline then online parsing; returns `InvalidData` when both
    /// fail.
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
    /// 把离线报文包进 [`RakPacket::Offline`]。
    ///
    /// Wraps an offline packet into [`RakPacket::Offline`].
    fn from(packet: OfflinePacket) -> Self {
        RakPacket::Offline(packet)
    }
}

impl From<OnlinePacket> for RakPacket {
    /// 把在线报文包进 [`RakPacket::Online`]。
    ///
    /// Wraps an online packet into [`RakPacket::Online`].
    fn from(packet: OnlinePacket) -> Self {
        RakPacket::Online(packet)
    }
}

impl From<RakPacket> for OnlinePacket {
    /// 取出在线报文；变体不匹配时 panic。
    ///
    /// Extracts the online packet; panics on a variant mismatch.
    fn from(packet: RakPacket) -> Self {
        match packet {
            RakPacket::Online(packet) => packet,
            _ => panic!("Invalid packet conversion"),
        }
    }
}

impl From<RakPacket> for OfflinePacket {
    /// 取出离线报文；变体不匹配时 panic。
    ///
    /// Extracts the offline packet; panics on a variant mismatch.
    fn from(packet: RakPacket) -> Self {
        match packet {
            RakPacket::Offline(packet) => packet,
            _ => panic!("Invalid packet conversion"),
        }
    }
}

/// 批量注册报文转换：为每个报文类型生成与 `$kind`、`RakPacket` 的双向
/// `From` 实现。
///
/// 展开产物（对每个 `$packet`）：
/// - `impl From<$packet> for $kind`
/// - `impl From<$packet> for RakPacket`
/// - `impl From<$kind> for $packet`（变体不匹配时 panic）
/// - `impl From<RakPacket> for $packet`（变体不匹配时 panic）
///
/// Bulk-registers packet conversions: generates bidirectional `From`
/// implementations between each packet type, `$kind`, and `RakPacket`.
///
/// Expanded forms (per `$packet`):
/// - `impl From<$packet> for $kind`
/// - `impl From<$packet> for RakPacket`
/// - `impl From<$kind> for $packet` (panics on variant mismatch)
/// - `impl From<RakPacket> for $packet` (panics on variant mismatch)
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
