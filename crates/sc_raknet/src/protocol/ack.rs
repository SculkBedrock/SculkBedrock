pub const ACK: u8 = 0xc0;
pub const NACK: u8 = 0xa0;

use sc_binary::{
    interfaces::{Reader, Writer},
    types::u24,
    BinaryIo, ByteReader, ByteWriter,
};

const MAX_ACK_RECORDS: usize = 1024;

pub(crate) trait Ackable {
    type NackItem;

    /// When an ack packet is received.
    /// We should ack the queue
    fn ack(&mut self, _: Ack) {}

    /// When an NACK packet is received.
    /// We should nack the queue
    /// This should return the packets that need to be resent.
    fn nack(&mut self, _: Ack) -> Vec<Self::NackItem> {
        Vec::new()
    }
}

/// An ack record.
/// A record holds a single or range of acked packets.
/// No real complexity other than that.
#[derive(Debug, Clone, BinaryIo)]
#[repr(u8)]
pub enum Record {
    Single(SingleRecord) = 1,
    Range(RangeRecord) = 0,
}

#[derive(Debug, Clone)]
pub struct SingleRecord {
    pub sequence: u24,
}

impl Reader<SingleRecord> for SingleRecord {
    fn read(buf: &mut ByteReader) -> Result<SingleRecord, std::io::Error> {
        Ok(SingleRecord {
            sequence: buf.read_u24_le()?.into(),
        })
    }
}

impl Writer for SingleRecord {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        buf.write_u24_le(self.sequence)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RangeRecord {
    pub start: u24,
    pub end: u24,
}

impl Reader<RangeRecord> for RangeRecord {
    fn read(buf: &mut ByteReader) -> Result<RangeRecord, std::io::Error> {
        Ok(RangeRecord {
            start: buf.read_u24_le()?.into(),
            end: buf.read_u24_le()?.into(),
        })
    }
}

impl Writer for RangeRecord {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        buf.write_u24_le(self.start)?;
        buf.write_u24_le(self.end)?;
        Ok(())
    }
}

#[allow(dead_code)]
impl RangeRecord {
    /// Fixes the end of the range if it is lower than the start.
    pub fn fix(&mut self) {
        if self.end < self.start {
            std::mem::swap(&mut self.start, &mut self.end);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ack {
    pub id: u8,
    pub count: u16,
    pub records: Vec<Record>,
}

impl Ack {
    pub fn new(count: u16, nack: bool, records: Vec<Record>) -> Self {
        Self {
            id: if nack { 0xa0 } else { 0xc0 },
            count,
            records,
        }
    }

    pub fn is_nack(&self) -> bool {
        self.id == 0xa0
    }

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
