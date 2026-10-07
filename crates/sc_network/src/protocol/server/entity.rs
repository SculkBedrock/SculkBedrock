use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_entity::MinecraftEntityId;
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::{NbtValue, SCNBTByteWriter};
use sc_network_macros::MinecraftPacket;
use sc_packloader::definitions::attribute::EntityAttributes;
use std::io::Error;
use std::sync::Arc;

#[derive(Clone, Debug, MinecraftPacket)]
pub struct AvailableEntityIdentifiers {
    pub nbt: Arc<NbtValue>,
}

impl Writer for AvailableEntityIdentifiers {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_nbt::<BedrockNetworkNbt>(&self.nbt)?;
        Ok(())
    }
}

impl Reader<AvailableEntityIdentifiers> for AvailableEntityIdentifiers {
    fn read(_buf: &mut ByteReader) -> Result<AvailableEntityIdentifiers, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "entity packet decode is unsupported",
        ))
    }
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct UpdateAttributes {
    pub entity_id: MinecraftEntityId,
    pub attributes: EntityAttributes,
    pub frame: i64,
}

impl Writer for UpdateAttributes {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.entity_id.0)?;
        buf.write_var_u32(self.attributes.len() as u32)?;
        for (name, attribute) in self.attributes.iter() {
            let attribute = attribute.read();
            buf.write_f32_le(attribute.min_value)?;
            buf.write_f32_le(attribute.max_value)?;
            buf.write_f32_le(attribute.current_value)?;
            buf.write_f32_le(attribute.default_min_value)?;
            buf.write_f32_le(attribute.default_max_value)?;
            buf.write_f32_le(attribute.default_value)?;
            buf.write_string(&name)?;
            buf.write_var_u32(0)?; // Modifiers (none)
        }
        buf.write_var_u64(self.frame as u64)
    }
}

impl Reader<UpdateAttributes> for UpdateAttributes {
    fn read(_buf: &mut ByteReader) -> Result<UpdateAttributes, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "entity packet decode is unsupported",
        ))
    }
}

/// ActorEvent ids for item stack sync.
pub mod actor_event {
    /// UPDATE_STACK_SIZE: stack count sync after item merge.
    pub const UPDATE_STACK_SIZE: u8 = 50;
}

/// EntityEvent/ActorEvent (0x1b).
///
/// Layout: varint64 target entity runtime id, u8 event id (see
/// [`actor_event`]), zigzag varint32 data, trailing
/// `fire_at_position: option<vec3f>`. An absent option encodes as one 0x00
/// byte.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct EntityEvent {
    pub entity_runtime_id: u64,
    pub event_type: u8,
    pub data: i32,
}

impl Writer for EntityEvent {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.entity_runtime_id)?;
        buf.write_u8(self.event_type)?;
        buf.write_var_i32(self.data)?;
        // fire_at_position absent: single 0x00 byte.
        buf.write_u8(0)
    }
}

impl Reader<EntityEvent> for EntityEvent {
    fn read(_buf: &mut ByteReader) -> Result<EntityEvent, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "entity event packet decode is unsupported",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::Writer;

    #[test]
    fn entity_event_writes_runtime_varint_then_u8_event_then_varint_data() {
        let packet = EntityEvent {
            entity_runtime_id: 300,
            event_type: actor_event::UPDATE_STACK_SIZE,
            data: 64,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        // varuint64(300) = 0xAC 0x02, u8 50, zigzag varint32(64) = 128 = 0x80 0x01,
        // Trailing fire_at_position option absent: 0x00.
        assert_eq!(writer.as_slice(), &[0xAC, 0x02, 50, 0x80, 0x01, 0x00]);
    }
}
