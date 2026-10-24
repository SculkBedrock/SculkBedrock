use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_entity::MinecraftEntityId;
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::{NbtValue, SCNBTByteWriter};
use sc_network_macros::MinecraftPacket;
use sc_packloader::definitions::attribute::EntityAttributes;
use std::io::Error;
use std::sync::Arc;

/// AvailableEntityIdentifiersPacket：向客户端声明全部可用实体标识。
///
/// AvailableEntityIdentifiersPacket: declares all available entity
/// identifiers to the client.
///
/// 负载为一份 NBT 实体清单，随 StartGame 一同发送。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct AvailableEntityIdentifiers {
    /// 实体标识清单的 NBT 根值。
    ///
    /// NBT root value of the entity identifier list.
    pub nbt: Arc<NbtValue>,
}

impl Writer for AvailableEntityIdentifiers {
    /// 以 Bedrock 网络 NBT 格式写入实体标识清单。
    ///
    /// Writes the entity identifier list using the Bedrock network NBT
    /// format.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// NBT 序列化失败时返回错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_nbt::<BedrockNetworkNbt>(&self.nbt)?;
        Ok(())
    }
}

impl Reader<AvailableEntityIdentifiers> for AvailableEntityIdentifiers {
    /// 不支持解码：服务端只发送实体标识清单。
    ///
    /// Decoding is unsupported: the server only sends the identifier list.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<AvailableEntityIdentifiers, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "entity packet decode is unsupported",
        ))
    }
}

/// UpdateAttributesPacket：同步实体的属性（生命值、移速等）。
///
/// UpdateAttributesPacket: synchronizes entity attributes (health, movement
/// speed, etc.).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct UpdateAttributes {
    /// 目标实体 id。
    ///
    /// Target entity id.
    pub entity_id: MinecraftEntityId,
    /// 属性集合（名称 → 属性句柄）。
    ///
    /// Attribute set (name -> attribute handle).
    pub attributes: EntityAttributes,
    /// 属性同步帧号，客户端用于丢弃过期更新。
    ///
    /// Attribute sync frame; the client drops stale updates.
    pub frame: i64,
}

impl Writer for UpdateAttributes {
    /// 写入属性包体：实体 id、属性表与帧号。
    ///
    /// Writes the attribute packet body: entity id, attribute table and
    /// frame.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
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
    /// 不支持解码：服务端只发送属性更新。
    ///
    /// Decoding is unsupported: the server only sends attribute updates.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<UpdateAttributes, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "entity packet decode is unsupported",
        ))
    }
}

/// ActorEvent 事件 id：物品堆叠同步相关。
///
/// ActorEvent ids used for item stack synchronization.
pub mod actor_event {
    /// HURT：受伤动画。
    ///
    /// HURT: damage animation.
    pub const HURT: u8 = 2;
    /// DEATH：死亡动画。
    ///
    /// DEATH: death animation.
    pub const DEATH: u8 = 3;
    /// UPDATE_STACK_SIZE：物品合并后的堆叠数量同步。
    ///
    /// UPDATE_STACK_SIZE: stack count sync after item merge.
    pub const UPDATE_STACK_SIZE: u8 = 50;
}

/// EntityEventPacket/ActorEventPacket (0x1b)：实体事件（受伤、死亡、
/// 堆叠同步等）。
///
/// EntityEventPacket/ActorEventPacket (0x1b): entity events (hurt, death,
/// stack sync, etc.).
///
/// 布局：varint64 目标实体运行时 id、u8 事件 id（见 [`actor_event`]）、
/// zigzag varint32 数据、尾部 `fire_at_position: option<vec3f>`；
/// 选项缺失时编码为单个 0x00 字节。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct EntityEvent {
    /// 目标实体运行时 id。
    ///
    /// Runtime id of the target entity.
    pub entity_runtime_id: u64,
    /// 事件类型 id，见 [`actor_event`]。
    ///
    /// Event type id, see [`actor_event`].
    pub event_type: u8,
    /// 事件附加数据（zigzag varint32）。
    ///
    /// Extra event data (zigzag varint32).
    pub data: i32,
}

impl Writer for EntityEvent {
    /// 写入 EntityEvent 包体。
    ///
    /// Writes the EntityEvent packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.entity_runtime_id)?;
        buf.write_u8(self.event_type)?;
        buf.write_var_i32(self.data)?;
        // fire_at_position absent: single 0x00 byte.
        buf.write_u8(0)
    }
}

impl Reader<EntityEvent> for EntityEvent {
    /// 不支持解码：服务端只发送实体事件。
    ///
    /// Decoding is unsupported: the server only sends entity events.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
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
