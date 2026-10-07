//! Bedrock item-entity packets.

use std::io::Error;

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

use crate::protocol::client::transaction::{write_item_data, ItemData};
use crate::protocol::server::misc::{write_entity_metadata, EntityMetadataEntry};

/// AddItemEntity/AddItemActor (0x0f).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct AddItemEntity {
    pub entity_unique_id: i64,
    pub entity_runtime_id: u64,
    pub item: ItemData,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub motion_x: f32,
    pub motion_y: f32,
    pub motion_z: f32,
    /// Actor metadata. An empty list leaves client-side entity state
    /// incomplete (pickup animation does not play).
    pub metadata: Vec<EntityMetadataEntry>,
}

impl Writer for AddItemEntity {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i64(self.entity_unique_id)?;
        buf.write_var_u64(self.entity_runtime_id)?;
        // AddItemActorSerializer_v2168 serializes the item with the same
        // NetworkItemStackDescriptor layout as InventoryContent.
        write_item_data(buf, &self.item)?;
        buf.write_f32_le(self.x)?;
        buf.write_f32_le(self.y)?;
        buf.write_f32_le(self.z)?;
        buf.write_f32_le(self.motion_x)?;
        buf.write_f32_le(self.motion_y)?;
        buf.write_f32_le(self.motion_z)?;
        // v291+ AddItemEntity carries actor metadata followed by isFromFishing.
        write_entity_metadata(buf, &self.metadata)?;
        buf.write_bool(false)
    }
}

impl Reader<AddItemEntity> for AddItemEntity {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "AddItemEntity decode is unsupported",
        ))
    }
}

/// TakeItemEntity/TakeItemActor (0x11).
///
/// Wire order: picked-up item actor runtime id first, collector second.
/// The client plays the absorb animation from the first id toward the second.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct TakeItemEntity {
    /// Runtime id of the item actor being picked up.
    pub item_entity_id: u64,
    /// Runtime id of the actor picking the item up (usually the player).
    pub target_entity_id: u64,
}

impl Writer for TakeItemEntity {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.item_entity_id)?;
        buf.write_var_u64(self.target_entity_id)
    }
}

impl Reader<TakeItemEntity> for TakeItemEntity {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "TakeItemEntity decode is unsupported",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::Writer;

    #[test]
    fn add_item_entity_writer_ends_with_empty_metadata_and_fishing_flag() {
        let packet = AddItemEntity {
            entity_unique_id: 1,
            entity_runtime_id: 2,
            item: ItemData::default(),
            x: 1.0,
            y: 2.0,
            z: 3.0,
            motion_x: 0.0,
            motion_y: 0.0,
            motion_z: 0.0,
            metadata: Vec::new(),
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();

        let bytes = writer.as_slice();
        assert!(bytes.len() >= 2);
        assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]);
    }

    #[test]
    fn add_item_entity_uses_network_item_stack_descriptor_layout() {
        let packet = AddItemEntity {
            entity_unique_id: 1,
            entity_runtime_id: 2,
            item: ItemData {
                runtime_id: 5,
                count: 2,
                damage: 3,
                ..ItemData::default()
            },
            x: 1.0,
            y: 2.0,
            z: 3.0,
            motion_x: 0.0,
            motion_y: 0.0,
            motion_z: 0.0,
            metadata: Vec::new(),
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();

        // unique id, runtime id, then the v2168 NetworkItemStackDescriptor:
        // u16le runtime, u16le count, uvarint damage, usingNetId(true),
        // varint netId, uvarint block runtime id, then the 10-byte default
        // user data behind its uvarint length prefix.
        assert_eq!(
            &writer.as_slice()[..10],
            &[0x02, 0x02, 0x05, 0x00, 0x02, 0x00, 0x03, 0x01, 0x00, 0x00]
        );
    }

    #[test]
    fn take_item_entity_writes_item_before_collector() {
        let packet = TakeItemEntity {
            item_entity_id: 3,
            target_entity_id: 7,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        // Wire order is item first, collector second.
        assert_eq!(writer.as_slice(), &[0x03, 0x07]);
    }

    #[test]
    fn take_item_entity_packet_id_is_0x11() {
        use crate::protocol::MinecraftPackets;

        let packet = TakeItemEntity {
            item_entity_id: 3,
            target_entity_id: 7,
        };
        let bytes = MinecraftPackets::TakeItemEntity(packet)
            .write_to_bytes()
            .unwrap();
        // BinaryIo enum discriminants are u16 BE; 0x10 is a retired slot,
        // take-item-actor is 0x11.
        assert_eq!(&bytes.as_slice()[..2], &[0x00, 0x11]);
    }
}
