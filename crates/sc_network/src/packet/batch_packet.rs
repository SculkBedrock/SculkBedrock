use crate::protocol::{MinecraftPacket, MinecraftPackets};
use log::{debug, trace};
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_log::t_log;
use std::collections::HashMap;
use std::io::Error;
use std::sync::Mutex;
use std::sync::OnceLock;

const PLAYER_AUTH_INPUT_PACKET_ID: u32 = 0x90;

fn should_log_read_packet(packet_id: u32) -> bool {
    packet_id != PLAYER_AUTH_INPUT_PACKET_ID
}

#[derive(Clone, Debug)]
pub struct BatchPacket {
    packets: Vec<MinecraftPackets>,
    index: usize,
}

impl BatchPacket {
    pub fn new() -> Self {
        Self {
            packets: vec![],
            index: 0,
        }
    }

    pub fn from_vec(packets: Vec<MinecraftPackets>) -> Self {
        Self { packets, index: 0 }
    }

    pub fn single<P: MinecraftPacket>(packet: P) -> Self {
        Self {
            packets: vec![packet.to_packets()],
            index: 0,
        }
    }

    pub fn push_packet<P: MinecraftPacket>(&mut self, packet: P) -> &mut Self {
        self.packets.push(packet.to_packets());
        self
    }

    pub fn packets(&self) -> &[MinecraftPackets] {
        &self.packets
    }
}

impl Iterator for BatchPacket {
    type Item = MinecraftPackets;

    fn next(&mut self) -> Option<Self::Item> {
        self.index += 1;
        self.packets
            .get(self.index - 1)
            .and_then(|packet| Some(packet.clone()))
    }
}

impl Reader<BatchPacket> for BatchPacket {
    fn read(buf: &mut ByteReader) -> Result<BatchPacket, Error> {
        if buf.read_u8()? == 0xfe {
            let mut packets = Vec::new();
            while buf.as_slice().len() != 0 {
                let packet = buf.read_sized_slice()?;
                let mut packet_reader = ByteReader::from(packet);
                let packet_flags = packet_reader.read_var_u32()?;
                let packet_id = packet_flags & 0x3ff;
                if should_log_read_packet(packet_id) {
                    trace!("Batch Packet Read >> 0x{:x}", packet_id);
                }
                let packet_data = packet_reader.as_slice();

                // MinecraftPackets uses a u16 enum discriminator internally, while Bedrock
                // batch packets carry the packet id as a varuint in the per-packet header.
                let mut byte_writer = ByteWriter::new();
                if packet_id > u16::MAX as u32 {
                    log::debug!("packet >> skipped unsupported packet id 0x{:x}", packet_id);
                    continue;
                }
                byte_writer.write_u16(packet_id as u16)?;
                byte_writer.write(packet_data)?;
                let mut packet_reader = ByteReader::from(byte_writer.as_slice());

                match MinecraftPackets::read(&mut packet_reader) {
                    Ok(packet) => {
                        log_player_auth_input_block_actions(&packet, packet_data.len());
                        if should_log_read_packet(packet_id) {
                            debug!(
                                "Batch Packet Read << {} (0x{:02x}, {}B)",
                                packet_variant_name(&packet),
                                packet_id,
                                packet_data.len()
                            );
                        }
                        // Item-request diagnostics: also record raw hex on
                        // successful 0x93 decode (160B cap); execution-time
                        // rejections are otherwise unattributable.
                        if packet_id == 0x93 {
                            let preview = packet_data
                                .get(..packet_data.len().min(160))
                                .unwrap_or(packet_data);
                            debug!("ItemStackRequest wire {:02x?}", preview);
                        }

                        static RECEIVED: OnceLock<Mutex<HashMap<u32, u32>>> = OnceLock::new();
                        let counts = RECEIVED.get_or_init(|| Mutex::new(HashMap::new()));
                        let mut counts = counts.lock().unwrap();
                        let n = counts.entry(packet_id).or_insert(0);
                        *n += 1;
                        let total: u32 = counts.values().sum();
                        if total % 200 == 0 {
                            let snapshot: Vec<(u32, u32)> =
                                counts.iter().map(|(k, v)| (*k, *v)).collect();
                            log::debug!("packet >> client packet stats {total}: {snapshot:?}");
                        }
                        packets.push(packet);
                    }
                    Err(error) => {
                        static SKIPPED: OnceLock<Mutex<HashMap<u32, u32>>> = OnceLock::new();
                        let counts = SKIPPED.get_or_init(|| Mutex::new(HashMap::new()));
                        let mut counts = counts.lock().unwrap();
                        let n = counts.entry(packet_id).or_insert(0);
                        *n += 1;
                        let n = *n;
                        drop(counts);
                        if n == 1 || n % 100 == 0 {
                            let data = packet_data
                                .get(..packet_data.len().min(64))
                                .unwrap_or(packet_data);
                            log::warn!(
                                "{}",
                                t_log!(
                                    "console.packet.skipped",
                                    id = format!("{packet_id:02x}"),
                                    count = n,
                                    error = error,
                                    data = format!("{data:02x?}")
                                )
                            );
                        }
                        trace!(
                            "Batch Packet Read >> skipped packet 0x{:02x}: {}",
                            packet_id,
                            error
                        );
                    }
                }
            }
            Ok(BatchPacket { packets, index: 0 })
        } else {
            Err(Error::other("Not batch packet"))
        }
    }
}

impl Writer for BatchPacket {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(0xfe)?;
        for packet in &self.packets {
            let byte_writer = packet.write_to_bytes()?;
            let packet_bytes = byte_writer.as_slice();
            if packet_bytes.len() < 2 {
                return Err(Error::other("Minecraft packet missing u16 packet id"));
            }
            let packet_id = u16::from_be_bytes([packet_bytes[0], packet_bytes[1]]);
            let packet_data = &packet_bytes[2..packet_bytes.len()];

            debug!(
                "Batch Packet Write >> {}{} (0x{:x}, {}B)",
                packet_variant_name(packet),
                packet_entity_hint(packet),
                packet_id,
                packet_bytes.len()
            );

            // The old path serialized into a temporary `ByteWriter::new()`
            // first, then copied it back with `buf.write_slice()` (one extra
            // whole-batch copy per batch).
            //
            // Note `write_slice` is not a pure append: it first writes a
            // varint length prefix equal to the staged buffer length. The
            // exact sub-frame length must therefore be computed first: encode
            // the id varint into a 5-byte stack array (u32 varint max),
            // length = id_len + payload length. Output bytes match the old
            // implementation byte for byte.
            let header = packet_id as u32 | (0 << 10) | (0 << 12);
            let mut id_bytes = [0u8; 5];
            let mut cursor = 0usize;
            let mut shifted = header;
            while shifted >= 0x80 {
                id_bytes[cursor] = (shifted as u8) | 0x80;
                cursor += 1;
                shifted >>= 7;
            }
            id_bytes[cursor] = shifted as u8;
            cursor += 1;

            let frame_len = cursor + packet_data.len();
            if frame_len > u32::MAX as usize {
                return Err(Error::other("Minecraft packet frame exceeds varint length"));
            }
            buf.write_var_u32(frame_len as u32)?;
            buf.write(&id_bytes[..cursor])?;
            buf.write(packet_data)?;
        }
        Ok(())
    }
}

fn log_player_auth_input_block_actions(packet: &MinecraftPackets, packet_size: usize) {
    let MinecraftPackets::PlayerAuthInput(input) = packet else {
        return;
    };
    if input.block_actions.is_empty()
        && !input.has_input(crate::protocol::client::movement::AuthInputAction::PerformBlockActions)
    {
        return;
    }

    debug!(
        "Batch Packet Read << PlayerAuthInput block actions: packet={}B flags={:?} actions={:?}",
        packet_size, input.input_data, input.block_actions
    );
}

/// Variant name as a compile-time constant string, zero allocation.
///
/// Formatting the whole packet body per send would serialize large payloads
/// (a 74KB `LevelChunk`) on the hot path, and `debug!` arguments always
/// evaluate at the default log level.
fn packet_variant_name(packet: &MinecraftPackets) -> &'static str {
    packet.packet_name()
}

/// Debug hint for entity packets: AddItemEntity/MoveEntityAbsolute/
/// RemoveEntity append the entity runtime id; others contribute nothing.
fn packet_entity_hint(packet: &MinecraftPackets) -> String {
    match packet {
        MinecraftPackets::AddItemEntity(p) => format!(" entity={}", p.entity_runtime_id),
        MinecraftPackets::MoveEntityAbsolute(p) => format!(" entity={}", p.entity_id),
        MinecraftPackets::RemoveEntity(p) => format!(" entity={}", p.entity_id),
        MinecraftPackets::TakeItemEntity(p) => format!(
            " item={} collector={}",
            p.item_entity_id, p.target_entity_id
        ),
        _ => String::new(),
    }
}

#[derive(Clone, Debug)]
pub struct OriginBatchPacket {
    packets: Vec<Vec<u8>>,
    index: usize,
}

impl OriginBatchPacket {
    pub fn new() -> Self {
        Self {
            packets: vec![],
            index: 0,
        }
    }

    pub fn from_vec(packets: Vec<Vec<u8>>) -> Self {
        Self { packets, index: 0 }
    }

    pub fn single(packet: Vec<u8>) -> Self {
        Self {
            packets: vec![packet],
            index: 0,
        }
    }

    pub fn push_packet(&mut self, packet: Vec<u8>) -> &mut Self {
        self.packets.push(packet);
        self
    }

    pub fn to_batch_packet(&self) -> Result<BatchPacket, Error> {
        BatchPacket::read(&mut ByteReader::from(self.write_to_bytes()?))
    }
}

impl Iterator for OriginBatchPacket {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        self.index += 1;
        self.packets
            .get(self.index - 1)
            .and_then(|packet| Some(packet.clone()))
    }
}

impl Reader<OriginBatchPacket> for OriginBatchPacket {
    fn read(buf: &mut ByteReader) -> Result<OriginBatchPacket, Error> {
        if buf.read_u8()? == 0xfe {
            let mut packets = Vec::new();
            while buf.as_slice().len() != 0 {
                if let Ok(packet) = buf.read_sized_slice() {
                    let mut packet_reader = ByteReader::from(packet);
                    let packet_flags = packet_reader.read_var_u32()?;
                    let packet_id = packet_flags & 0x3ff;
                    if should_log_read_packet(packet_id) {
                        trace!("Origin Batch Packet Read >> 0x{:x}", packet_id);
                    }
                    let packet_data = packet_reader.as_slice();

                    let mut byte_writer = ByteWriter::new();
                    if packet_id > u16::MAX as u32 {
                        log::debug!(
                            "origin packet >> skipped unsupported packet id 0x{:x}",
                            packet_id
                        );
                        continue;
                    }
                    byte_writer.write_u16(packet_id as u16)?;
                    byte_writer.write(packet_data)?;
                    let bytes = byte_writer.as_slice();

                    packets.push(bytes.to_vec());
                }
            }
            Ok(OriginBatchPacket { packets, index: 0 })
        } else {
            Err(Error::other("Not batch packet"))
        }
    }
}

impl Writer for OriginBatchPacket {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(0xfe)?;
        for packet in &self.packets {
            let packet_bytes = packet.as_slice();
            if packet_bytes.len() < 2 {
                return Err(Error::other("origin packet missing u16 packet id"));
            }
            let packet_id = u16::from_be_bytes([packet_bytes[0], packet_bytes[1]]) as u32;
            let packet_data = &packet_bytes[2..packet_bytes.len()];

            let mut byte_writer = ByteWriter::new();
            byte_writer.write_var_u32(packet_id | (0 << 10) | (0 << 12))?;
            byte_writer.write(packet_data)?;
            let packet_bytes = byte_writer.as_slice();

            buf.write_slice(packet_bytes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::client::container::ContainerClose;
    use crate::protocol::server::misc::SetTime;
    use sc_binary::interfaces::Reader;

    #[test]
    fn player_auth_input_read_logging_is_suppressed() {
        assert!(!should_log_read_packet(PLAYER_AUTH_INPUT_PACKET_ID));
        assert!(should_log_read_packet(0x2f));
    }

    #[test]
    fn unknown_packet_does_not_discard_valid_packet_in_same_batch() {
        let mut writer = ByteWriter::new();
        writer.write_u8(0xfe).unwrap();

        let valid = [0x45, 0x06, 0x04];
        writer.write_slice(&valid).unwrap();
        let unknown = [0x38];
        writer.write_slice(&unknown).unwrap();

        let mut reader = ByteReader::from(writer.as_slice());
        let batch = BatchPacket::read(&mut reader).unwrap();
        let packets: Vec<_> = batch.collect();
        assert_eq!(packets.len(), 1);
        match &packets[0] {
            MinecraftPackets::RequestChunkRadius(packet) => {
                assert_eq!(packet.radius, 3);
                assert_eq!(packet.max_radius, 4);
            }
            _ => panic!("expected RequestChunkRadius"),
        }
    }

    #[test]
    fn high_packet_id_is_not_truncated_to_a_low_packet_id() {
        let mut writer = ByteWriter::new();
        writer.write_u8(0xfe).unwrap();

        let mut high_id = ByteWriter::new();
        high_id.write_var_u32(0x138).unwrap();
        writer.write_slice(high_id.as_slice()).unwrap();
        writer.write_slice(&[0x45, 0x06, 0x04]).unwrap();

        let mut reader = ByteReader::from(writer.as_slice());
        let batch = BatchPacket::read(&mut reader).unwrap();
        let packets: Vec<_> = batch.collect();
        assert_eq!(packets.len(), 2);
        assert!(matches!(
            packets[0],
            MinecraftPackets::ServerboundLoadingScreen(_)
        ));
        assert!(matches!(
            packets[1],
            MinecraftPackets::RequestChunkRadius(_)
        ));
    }

    /// `BatchPacket::write` byte layout is pinned:
    /// `[0xfe]` then one `varint length prefix + sub-frame` per packet.
    ///
    /// Sub-frame = `varint(packet_id | 0<<10 | 0<<12)` + body.
    ///
    /// The prefix comes from `ByteWriter::write_slice` (which prepends a
    /// varint length, not a pure append). Lengths are computed up front and
    /// written directly into the outer buffer to avoid a whole-batch copy;
    /// reference bytes lock the prefix against accidental deletion.
    #[test]
    fn batch_frame_layout_matches_reference_bytes() {
        // SetTime { time: 1 } → [u16 id BE = 0x00 0x0a] [var_i32(1) = 0x02]
        // Sub-frame = [varint id 0x0a] [0x02], length 2
        let mut writer = ByteWriter::new();
        BatchPacket::single(SetTime { time: 1 })
            .write(&mut writer)
            .unwrap();
        assert_eq!(
            writer.as_slice(),
            &[0xfe, 0x02, 0x0a, 0x02],
            "BatchPacket single-packet sub-frame layout changed"
        );
    }

    /// Length prefixes must be correct per packet without cross-talk.
    /// The second packet uses `VoxelShapes` (id 0x151, 2-byte varint) to
    /// cover multi-byte id prefix computation.
    #[test]
    fn batch_frame_length_prefix_handles_multibyte_ids() {
        use crate::protocol::server::modern::VoxelShapes;

        let packets = vec![
            MinecraftPackets::SetTime(SetTime { time: 1 }),
            MinecraftPackets::VoxelShapes(VoxelShapes {
                shapes: Vec::new(),
                custom_shape_count: 0,
            }),
        ];
        let mut writer = ByteWriter::new();
        BatchPacket::from_vec(packets).write(&mut writer).unwrap();

        // Sub-frame 1: id 0x0a → varint [0x0a], body [0x02] (var_i32(1)), len=2
        // Sub-frame 2: id 0x151 → varint [0xd1 0x02], 4-byte body, len=6
        assert_eq!(
            writer.as_slice(),
            &[0xfe, 0x02, 0x0a, 0x02, 0x06, 0xd1, 0x02, 0x00, 0x00, 0x00, 0x00],
            "multi-packet length prefix or id varint encoding changed"
        );
    }

    /// Written batches must read back unchanged (round-trip).
    #[test]
    fn batch_write_round_trips_through_read() {
        let packets = vec![
            MinecraftPackets::SetTime(SetTime { time: 1 }),
            MinecraftPackets::SetTime(SetTime { time: 300 }),
            MinecraftPackets::SetTime(SetTime { time: 70_000 }),
        ];
        let mut writer = ByteWriter::new();
        BatchPacket::from_vec(packets).write(&mut writer).unwrap();

        let mut reader = ByteReader::from(writer.as_slice());
        let decoded: Vec<_> = BatchPacket::read(&mut reader).unwrap().collect();
        assert_eq!(decoded.len(), 3);
        for (decoded, expected) in decoded.iter().zip([1i32, 300, 70_000]) {
            match decoded {
                MinecraftPackets::SetTime(time) => assert_eq!(time.time, expected),
                other => panic!("unexpected variant: {other:?}"),
            }
        }
    }

    #[test]
    fn packet_variant_name_is_readable_for_diagnostics() {
        use crate::protocol::server::chunk::LevelChunk;
        let packet = MinecraftPackets::LevelChunk(LevelChunk {
            chunk_x: 0,
            chunk_z: 0,
            dimension: 0,
            sub_chunk_count: 1,
            cache_enabled: false,
            payload: vec![9, 1, 252, 1],
        });
        assert_eq!(packet_variant_name(&packet), "LevelChunk");
        let mut writer = ByteWriter::new();
        BatchPacket::from_vec(vec![packet])
            .write(&mut writer)
            .unwrap();
        assert!(!writer.as_slice().is_empty());
    }

    #[test]
    fn container_close_is_decoded_from_bedrock_batch() {
        let input = BatchPacket::single(ContainerClose {
            window_id: 0,
            container_type: -1,
            was_server_initiated: false,
        })
        .write_to_bytes()
        .unwrap();

        let mut reader = ByteReader::from(input.as_slice());
        let mut batch = BatchPacket::read(&mut reader).unwrap();
        let packet = batch.next().expect("ContainerClose should be decoded");

        match packet {
            MinecraftPackets::ContainerClose(packet) => {
                assert_eq!(packet.window_id, 0);
                assert_eq!(packet.container_type, -1);
                assert!(!packet.was_server_initiated);
            }
            other => panic!("expected ContainerClose, got {other:?}"),
        }
    }
}
