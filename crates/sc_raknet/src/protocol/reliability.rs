/// RakNet 帧可靠性等级（帧标志位高 3 位）。
///
/// RakNet frame reliability level (top 3 bits of the frame flags byte).
#[derive(Clone, Debug, Copy)]
#[repr(u8)]
pub enum Reliability {
    /// 不可靠（无确认）。
    ///
    /// Unreliable (with no ack).
    Unreliable = 0,
    /// 不可靠但带序号。
    ///
    /// Unreliable with a sequence.
    UnreliableSeq,
    /// 可靠。
    ///
    /// Reliable.
    Reliable,
    /// 可靠有序（按到达顺序投递，乱序即丢弃）。
    ///
    /// Reliable ordered (delivered in arrival order; drops when out of order).
    ReliableOrd,
    /// 可靠且排序：既可靠又按序号排序。
    ///
    /// Reliably sequenced **AND** ordered.
    ReliableSeq,
    /// 带 ACK 的不可靠帧，不在网络上使用。
    ///
    /// Never used over the wire.
    UnreliableAck,
    /// 带 ACK 的可靠帧，不在网络上使用。
    ///
    /// Never used over the wire.
    ReliableAck,
    /// 带 ACK 的可靠有序帧，不在网络上使用。
    ///
    /// Never used over the wire.
    ReliableOrdAck,
}

impl Reliability {
    /// 从帧标志字节解析可靠性（取高 3 位）。
    ///
    /// 无效值不会报错，退化为 [`Reliability::Unreliable`]。
    ///
    /// Parses the reliability from a frame flags byte (top 3 bits).
    ///
    /// Invalid values do not error; they degrade to
    /// [`Reliability::Unreliable`].
    pub fn from_flags(flags: u8) -> Self {
        match (flags & 224) >> 5 {
            0 => Reliability::Unreliable,
            1 => Reliability::UnreliableSeq,
            2 => Reliability::Reliable,
            3 => Reliability::ReliableOrd,
            4 => Reliability::ReliableSeq,
            5 => Reliability::UnreliableAck,
            6 => Reliability::ReliableAck,
            7 => Reliability::ReliableOrdAck,
            // we shouldn't error, but we'll just return unreliable
            _ => Reliability::Unreliable,
        }
    }

    /// 把 [`Reliability`] 转换为位标志，用于编码帧头。
    ///
    /// Converts the [`Reliability`] into a bit flag, useful for encoding
    /// the reliability into a packet.
    pub fn to_flags(&self) -> u8 {
        match self {
            Reliability::Unreliable => 0 << 5,
            Reliability::UnreliableSeq => 1 << 5,
            Reliability::Reliable => 2 << 5,
            Reliability::ReliableOrd => 3 << 5,
            Reliability::ReliableSeq => 4 << 5,
            Reliability::UnreliableAck => 5 << 5,
            Reliability::ReliableAck => 6 << 5,
            Reliability::ReliableOrdAck => 7 << 5,
        }
    }

    /// 是否为有序/排序类型（需要序号字段）。
    ///
    /// Whether this is an ordered/sequenced type (needs a sequence field).
    pub fn is_ordered(&self) -> bool {
        match self {
            Self::UnreliableSeq | Self::ReliableOrd | Self::ReliableSeq | Self::ReliableOrdAck => {
                true
            }
            _ => false,
        }
    }

    /// 是否为可靠类型（需要 ACK 与可靠帧索引）。
    ///
    /// Whether this is a reliable type (needs an ACK and reliable index).
    pub fn is_reliable(&self) -> bool {
        match self {
            Self::Reliable | Self::ReliableOrd | Self::ReliableSeq | Self::ReliableOrdAck => true,
            _ => false,
        }
    }

    /// 是否为不可靠类型。
    ///
    /// Whether this is an unreliable type.
    pub fn is_unreliable(&self) -> bool {
        match self {
            Self::Unreliable | Self::UnreliableSeq | Self::UnreliableAck => true,
            _ => false,
        }
    }

    /// 是否为纯排序类型（Sequenced：只保留最新序号）。
    ///
    /// Whether this is a purely sequenced type (Sequenced: only the latest
    /// sequence is kept).
    pub fn is_sequenced(&self) -> bool {
        match self {
            Self::UnreliableSeq | Self::ReliableSeq => true,
            _ => false,
        }
    }

    /// 是否为排序或有序类型（携带序号索引字段）。
    ///
    /// Whether this is sequenced or ordered (carries an ordering index).
    pub fn is_sequenced_or_ordered(&self) -> bool {
        match self {
            Self::UnreliableSeq | Self::ReliableSeq | Self::ReliableOrd | Self::ReliableOrdAck => {
                true
            }
            _ => false,
        }
    }

    /// 校验该可靠性是否为 ACK 类型（[`Ack`]）。
    /// 主要在内部用于判断是否需要发送 ACK。
    ///
    /// Verifies that the reliability is an ack ([`Ack`]).
    /// This is primarily used internally to determine whether or not to
    /// send an ack.
    ///
    /// [`Ack`]: crate::protocol::ack::Ack
    pub fn is_ack(&self) -> bool {
        match self {
            Self::UnreliableAck | Self::ReliableAck | Self::ReliableOrdAck => true,
            _ => false,
        }
    }
}
