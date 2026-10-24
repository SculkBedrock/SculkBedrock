/// ACK 报文的 ID：确认收到。
///
/// Packet ID of an ACK message: confirms receipt.
pub const ACK: u8 = 0xc0;
/// NACK 报文的 ID：报告丢失、请求重传。
///
/// Packet ID of a NACK message: reports loss and requests retransmission.
pub const NACK: u8 = 0xa0;

use sc_binary::{
    interfaces::{Reader, Writer},
    types::u24,
    BinaryIo, ByteReader, ByteWriter,
};

/// 单个 ACK/NACK 报文允许的最大记录数。
///
/// Maximum number of records allowed in a single ACK/NACK message.
const MAX_ACK_RECORDS: usize = 1024;

/// 可被 ACK/NACK 驱动的队列行为抽象。
///
/// Abstraction for queue behavior driven by ACK/NACK.
pub(crate) trait Ackable {
    /// NACK 携带的待重传条目类型。
    ///
    /// The type of items to be retransmitted, carried by a NACK.
    type NackItem;

    /// 收到 ACK 报文时调用：应对队列中的相应条目做确认。
    ///
    /// Called when an ack packet is received; the queue should be acked.
    fn ack(&mut self, _: Ack) {}

    /// 收到 NACK 报文时调用：应对队列中的相应条目做否定确认，
    /// 并返回需要重传的报文。
    ///
    /// Called when a NACK packet is received; the queue should be nacked
    /// and the packets that need to be resent returned.
    fn nack(&mut self, _: Ack) -> Vec<Self::NackItem> {
        Vec::new()
    }
}

/// 一条 ACK 记录：保存单个或一段被确认的序号，除此之外没有更多复杂度。
///
/// An ack record: holds a single or range of acked packets, with no real
/// complexity other than that.
#[derive(Debug, Clone, BinaryIo)]
#[repr(u8)]
pub enum Record {
    /// 单序号记录（判别值 1）。
    ///
    /// Single-sequence record (discriminant 1).
    Single(SingleRecord) = 1,
    /// 序号区间记录（判别值 0）。
    ///
    /// Sequence-range record (discriminant 0).
    Range(RangeRecord) = 0,
}

/// 单序号 ACK 记录。
///
/// A single-sequence ACK record.
#[derive(Debug, Clone)]
pub struct SingleRecord {
    /// 被确认的 24 位序号。
    ///
    /// The acknowledged 24-bit sequence number.
    pub sequence: u24,
}

impl Reader<SingleRecord> for SingleRecord {
    /// 从缓冲区读取 24 位序号，构造单序号记录。
    ///
    /// Reads a 24-bit sequence from the buffer into a single record.
    fn read(buf: &mut ByteReader) -> Result<SingleRecord, std::io::Error> {
        Ok(SingleRecord {
            sequence: buf.read_u24_le()?.into(),
        })
    }
}

impl Writer for SingleRecord {
    /// 把单序号记录的 24 位序号写入缓冲区。
    ///
    /// Writes the single record's 24-bit sequence into the buffer.
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        buf.write_u24_le(self.sequence)?;
        Ok(())
    }
}

/// 序号区间 ACK 记录（闭区间）。
///
/// A sequence-range ACK record (inclusive).
#[derive(Debug, Clone)]
pub struct RangeRecord {
    /// 区间起始序号。
    ///
    /// Start of the range.
    pub start: u24,
    /// 区间结束序号。
    ///
    /// End of the range.
    pub end: u24,
}

impl Reader<RangeRecord> for RangeRecord {
    /// 从缓冲区读取起止序号，构造区间记录。
    ///
    /// Reads both endpoints from the buffer into a range record.
    fn read(buf: &mut ByteReader) -> Result<RangeRecord, std::io::Error> {
        Ok(RangeRecord {
            start: buf.read_u24_le()?.into(),
            end: buf.read_u24_le()?.into(),
        })
    }
}

impl Writer for RangeRecord {
    /// 把区间记录的起止序号写入缓冲区。
    ///
    /// Writes both endpoints of the range record into the buffer.
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        buf.write_u24_le(self.start)?;
        buf.write_u24_le(self.end)?;
        Ok(())
    }
}

#[allow(dead_code)]
impl RangeRecord {
    /// 当区间终点小于起点时交换二者，修正区间方向。
    ///
    /// Swaps the endpoints when the end is lower than the start, fixing
    /// the range direction.
    pub fn fix(&mut self) {
        if self.end < self.start {
            std::mem::swap(&mut self.start, &mut self.end);
        }
    }
}

/// ACK/NACK 报文：记录列表加计数。
///
/// An ACK/NACK message: a list of records plus a count.
#[derive(Debug, Clone)]
pub struct Ack {
    /// 报文 ID（[`ACK`] 或 [`NACK`]）。
    ///
    /// Packet ID ([`ACK`] or [`NACK`]).
    pub id: u8,
    /// 记录条数（与 `records.len()` 一致）。
    ///
    /// Number of records (matches `records.len()`).
    pub count: u16,
    /// 记录列表。
    ///
    /// The list of records.
    pub records: Vec<Record>,
}

impl Ack {
    /// 直接以记录列表构造报文。
    ///
    /// Constructs a message directly from a record list.
    ///
    /// - `count`: 记录条数
    /// - `nack`: 为 `true` 时是 NACK，否则是 ACK
    /// - `records`: 记录列表
    pub fn new(count: u16, nack: bool, records: Vec<Record>) -> Self {
        Self {
            id: if nack { 0xa0 } else { 0xc0 },
            count,
            records,
        }
    }

    /// 是否为 NACK 报文。
    ///
    /// Whether this is a NACK message.
    pub fn is_nack(&self) -> bool {
        self.id == 0xa0
    }

    /// 由序号集合构造 ACK/NACK：排序去重后把连续序号合并为区间。
    ///
    /// 记录数超过 [`MAX_ACK_RECORDS`] 时截断；空输入产生空报文。
    ///
    /// Builds an ACK/NACK from a set of sequences: sorts and dedups, then
    /// merges consecutive sequences into inclusive ranges.
    ///
    /// Truncates when the record count exceeds [`MAX_ACK_RECORDS`]; an
    /// empty input yields an empty message.
    pub fn from_records(mut sequences: Vec<u32>, nack: bool) -> Self {
        // there at least one record
        if sequences.len() > 0 {
            // these sequences may not be in order.
            for seq in sequences.iter_mut() {
                *seq &= 0x00FF_FFFF;
            }
            sequences.sort_unstable();
            sequences.dedup();

            // Build ACK/NACK records, grouping consecutive sequences into
            // inclusive ranges.  Using Option<(start, end)> avoids the
            // ambiguity that the previous `Range(0..0)` initial value had with
            // an actual sequence number 0 (which caused seq 0 to always be
            // emitted as a standalone Single record, or even duplicated).
            let mut ack_records: Vec<Record> = Vec::new();
            let mut current: Option<(u32, u32)> = None;

            for &seq in sequences.iter() {
                match &mut current {
                    None => {
                        current = Some((seq, seq));
                    }
                    Some((start, end)) => {
                        if seq == *end + 1 {
                            *end = seq;
                        } else {
                            Self::flush_range(&mut ack_records, *start, *end);
                            current = Some((seq, seq));
                        }
                    }
                }
            }

            if let Some((start, end)) = current {
                Self::flush_range(&mut ack_records, start, end);
            }

            ack_records.truncate(MAX_ACK_RECORDS);
            let count = u16::try_from(ack_records.len()).unwrap_or(u16::MAX);
            Self::new(count, nack, ack_records)
        } else {
            Self::new(0_u16, nack, Vec::new())
        }
    }

    /// 把一段连续序号落为记录：单点用 `Single`，否则用 `Range`。
    ///
    /// Flushes a run of consecutive sequences into a record: `Single`
    /// when start equals end, `Range` otherwise.
    fn flush_range(records: &mut Vec<Record>, start: u32, end: u32) {
        if start == end {
            records.push(Record::Single(SingleRecord {
                sequence: start.into(),
            }));
        } else {
            records.push(Record::Range(RangeRecord {
                start: start.into(),
                end: end.into(),
            }));
        }
    }
}

impl Writer for Ack {
    /// 把 ACK/NACK 报文编码写入缓冲区。
    ///
    /// ID 非法、记录数超限或与计数不符时返回 `InvalidData`。
    ///
    /// Encodes the ACK/NACK message into the buffer.
    ///
    /// Returns `InvalidData` on an invalid ID, too many records, or a
    /// count that mismatches the records.
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        if self.id != ACK && self.id != NACK {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid ACK packet id",
            ));
        }
        if self.records.len() > MAX_ACK_RECORDS || self.count as usize != self.records.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ACK record count does not match records",
            ));
        }
        buf.write_u8(self.id)?;
        buf.write_u16(self.count)?;
        for record in &self.records {
            buf.write(record.write_to_bytes()?.as_slice())?;
        }
        Ok(())
    }
}

impl Reader<Ack> for Ack {
    /// 从缓冲区解码 ACK/NACK 报文。
    ///
    /// ID 非法或记录数超过 [`MAX_ACK_RECORDS`] 时返回 `InvalidData`。
    ///
    /// Decodes an ACK/NACK message from the buffer.
    ///
    /// Returns `InvalidData` when the ID is invalid or the record count
    /// exceeds [`MAX_ACK_RECORDS`].
    fn read(buf: &mut ByteReader) -> Result<Ack, std::io::Error> {
        let id = buf.read_u8()?;
        if id != ACK && id != NACK {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid ACK packet id",
            ));
        }
        let count = buf.read_u16()?;
        if usize::from(count) > MAX_ACK_RECORDS {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ACK record count exceeds limit",
            ));
        }
        let mut records: Vec<Record> = Vec::new();

        for _ in 0..usize::from(count) {
            let record = buf.read_type::<Record>()?;
            records.push(record);
        }

        Ok(Ack { id, count, records })
    }
}
