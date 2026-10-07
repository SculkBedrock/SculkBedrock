//! Shared pre-serialized packet batches (login constant-packet send path).
//!
//! [`RawBatchPacket`](crate::packet::raw_batch::RawBatchPacket) holds
//! complete serialized packets as `Arc<Vec<u8>>` (`[u16 id BE][payload]`,
//! the `MinecraftPackets::write_to_bytes` output format). ItemComponent /
//! CreativeContent / BiomeDefinitionList / AvailableEntityIdentifiers and
//! other multi-MB login constant packets are built and serialized once,
//! shared across players by reference count; each send only applies
//! per-connection compression and encryption.

use crate::protocol::MinecraftPacket;
use sc_binary::interfaces::Writer;
use sc_binary::ByteWriter;
use std::io::Error;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct RawBatchPacket {
    packets: Vec<Arc<Vec<u8>>>,
}

impl RawBatchPacket {
    pub fn single(packet: Arc<Vec<u8>>) -> Self {
        Self {
            packets: vec![packet],
        }
    }

    /// Serialize a structured packet once (called when writing the login cache).
    ///
    /// Output matches the [`crate::packet::batch_packet::BatchPacket`]
    /// per-packet format exactly: `[u16 id BE][payload]`.
    pub fn from_packet<P: MinecraftPacket>(packet: P) -> Result<Self, Error> {
        let bytes = packet_to_bytes(packet)?;
        Ok(Self {
            packets: vec![Arc::new(bytes)],
        })
    }

    pub fn packets(&self) -> &[Arc<Vec<u8>>] {
        &self.packets
    }
}

/// Structured packet to serialized bytes (`[u16 id BE][payload]`).
pub fn packet_to_bytes<P: MinecraftPacket>(packet: P) -> Result<Vec<u8>, Error> {
    let writer = packet.to_packets().write_to_bytes()?;
    Ok(writer.as_slice().to_vec())
}

impl Writer for RawBatchPacket {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(0xfe)?;
        for packet in &self.packets {
            let packet_bytes = packet.as_slice();
            if packet_bytes.len() < 2 {
                return Err(Error::other("raw packet missing u16 packet id"));
            }
            let packet_id = u16::from_be_bytes([packet_bytes[0], packet_bytes[1]]) as u32;
            let mut byte_writer = ByteWriter::new();
            byte_writer.write_var_u32(packet_id | (0 << 10) | (0 << 12))?;
            byte_writer.write(&packet_bytes[2..])?;
            buf.write_slice(byte_writer.as_slice())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::batch_packet::BatchPacket;
    use crate::protocol::server::misc::SetTime;
    use crate::protocol::MinecraftPackets;
    use sc_binary::interfaces::{Reader, Writer as _};
    use sc_binary::ByteReader;

    #[test]
    fn raw_batch_wire_format_matches_structured_batch() {
        let packet = SetTime { time: 12345 };

        let raw = RawBatchPacket::from_packet(packet.clone()).unwrap();
        let mut raw_writer = ByteWriter::new();
        raw.write(&mut raw_writer).unwrap();

        let mut structured_writer = ByteWriter::new();
        BatchPacket::single(packet)
            .write(&mut structured_writer)
            .unwrap();

        // Batch-level wire bytes match exactly (same packet, same framing).
        assert_eq!(raw_writer.as_slice(), structured_writer.as_slice());
    }

    #[test]
    fn raw_batch_round_trips_through_batch_reader() {
        let raw = RawBatchPacket::from_packet(SetTime { time: 777 }).unwrap();
        let mut writer = ByteWriter::new();
        raw.write(&mut writer).unwrap();

        let mut reader = ByteReader::from(writer.as_slice());
        let batch = BatchPacket::read(&mut reader).unwrap();
        let packets: Vec<_> = batch.collect();
        assert_eq!(packets.len(), 1);
        assert!(matches!(packets[0], MinecraftPackets::SetTime(_)));
    }
}
