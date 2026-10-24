use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// UpdateBlockPacket (0x15)：单方块变更广播。
///
/// UpdateBlockPacket (0x15): single-block change broadcast.
///
/// BlockPos 各分量均为 zigzag varint32 编码。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct UpdateBlock {
    /// 方块的 X 坐标。
    ///
    /// X coordinate of the block.
    pub x: i32,
    /// 方块的 Y 坐标。
    ///
    /// Y coordinate of the block.
    pub y: i32,
    /// 方块的 Z 坐标。
    ///
    /// Z coordinate of the block.
    pub z: i32,
    /// 方块状态哈希（StartGame 中 block_network_ids_hashed=true 时使用）。
    ///
    /// Hashed block network id (used when StartGame advertises
    /// block_network_ids_hashed=true).
    pub block_runtime_id: u32,
    /// 更新标志位，见 [`UpdateBlock`] 相关常量。
    ///
    /// Update flags, see the associated constants on [`UpdateBlock`].
    pub flags: u32,
    /// 0 = 第一方块层；1 = 第二层（如含水方块的水层）。
    ///
    /// 0 = first block layer; 1 = second layer (e.g. waterlogged layer).
    pub layer: u32,
}

impl UpdateBlock {
    /// 通知相邻方块更新（如触发红塔/方块更新）。
    ///
    /// Notify neighboring blocks (e.g. trigger block updates).
    pub const FLAG_NEIGHBORS: u32 = 0b0001;
    /// 该变更来自网络同步而非本地放置。
    ///
    /// The change originates from network sync, not local placement.
    pub const FLAG_NETWORK: u32 = 0b0010;
    /// 跳过客户端图形/粒子表现。
    ///
    /// Skip client-side graphics/particle effects.
    pub const FLAG_NO_GRAPHIC: u32 = 0b0100;
    /// 高优先级发送。
    ///
    /// Send with high priority.
    pub const FLAG_PRIORITY: u32 = 0b1000;
}

impl Writer for UpdateBlock {
    /// 将包体写入缓冲区：zigzag varint32 坐标 + 无符号 varint 字段。
    ///
    /// Writes the packet body: zigzag varint32 coordinates followed by
    /// unsigned varint fields.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        // BlockPos: x/y/z are all zigzag varint32. An unsigned-varint encoding
        // for y decodes underground positions to sky coordinates, so placed
        // blocks never apply on the client.
        buf.write_var_i32(self.x)?;
        buf.write_var_i32(self.y)?;
        buf.write_var_i32(self.z)?;
        // StartGame advertises blockIdsAreHashed=true. UpdateBlock therefore
        // carries the raw FNV1a block-state hash as an unsigned varint.
        buf.write_var_u32(self.block_runtime_id)?;
        buf.write_var_u32(self.flags)?;
        buf.write_var_u32(self.layer)
    }
}

impl Reader<UpdateBlock> for UpdateBlock {
    /// 从缓冲区解码 UpdateBlock 包体。
    ///
    /// Decodes an UpdateBlock packet body from the buffer.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或数据损坏时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<UpdateBlock, Error> {
        Ok(Self {
            x: buf.read_var_i32()?,
            y: buf.read_var_i32()?,
            z: buf.read_var_i32()?,
            block_runtime_id: buf.read_var_u32()?,
            flags: buf.read_var_u32()?,
            layer: buf.read_var_u32()?,
        })
    }
}

/// BlockEventPacket (0x1a)：方块事件广播（如箱子开盖/合盖动画）。
///
/// BlockEventPacket (0x1a): block event broadcast (e.g. chest lid opening/closing animation).
///
/// BlockPos 各分量及事件类型、事件数据均为 zigzag varint32 编码。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct BlockEvent {
    /// 方块的 X 坐标。
    ///
    /// X coordinate of the block.
    pub x: i32,
    /// 方块的 Y 坐标。
    ///
    /// Y coordinate of the block.
    pub y: i32,
    /// 方块的 Z 坐标。
    ///
    /// Z coordinate of the block.
    pub z: i32,
    /// 事件类型（如 1 = 更改箱子状态）。
    ///
    /// Event type (e.g. 1 = change chest state).
    pub event_type: i32,
    /// 事件附加数据（如 0 = 关闭, 1 = 打开）。
    ///
    /// Event data (e.g. 0 = closed, 1 = open).
    pub event_data: i32,
}

impl BlockEvent {
    /// 更改箱子开合状态事件类型。
    ///
    /// Change chest open/close state event type.
    pub const TYPE_CHANGE_CHEST_STATE: i32 = 1;
}

impl Writer for BlockEvent {
    /// 写入 BlockEvent 包体。
    ///
    /// Writes the BlockEvent packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.x)?;
        buf.write_var_i32(self.y)?;
        buf.write_var_i32(self.z)?;
        buf.write_var_i32(self.event_type)?;
        buf.write_var_i32(self.event_data)
    }
}

impl Reader<BlockEvent> for BlockEvent {
    /// 从缓冲区解码 BlockEvent 包体。
    ///
    /// Decodes a BlockEvent packet body from the buffer.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或数据损坏时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<BlockEvent, Error> {
        Ok(Self {
            x: buf.read_var_i32()?,
            y: buf.read_var_i32()?,
            z: buf.read_var_i32()?,
            event_type: buf.read_var_i32()?,
            event_data: buf.read_var_i32()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_block_round_trips_including_negative_y() {
        let packet = UpdateBlock {
            x: -5,
            y: -64,
            z: 17,
            block_runtime_id: 0xDEADBEEF,
            flags: UpdateBlock::FLAG_NEIGHBORS | UpdateBlock::FLAG_NETWORK,
            layer: 0,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(
            writer.as_slice(),
            &[
                0x09, // x=-5 (zigzag varint32)
                0x7f, // y=-64 (zigzag varint32: 127)
                0x22, // z=17 (zigzag varint32)
                0xef, 0xfd, 0xb6, 0xf5, 0x0d, // raw unsigned hash 0xDEADBEEF
                0x03, // flags
                0x00, // layer
            ],
            "BlockPos fields are all zigzag varint32 (official docs BlockPos, \
             minecraft-data 1.26.40 BlockCoordinates)",
        );
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = UpdateBlock::read(&mut reader).unwrap();
        assert_eq!(decoded.x, -5);
        assert_eq!(decoded.y, -64);
        assert_eq!(decoded.z, 17);
        assert_eq!(decoded.block_runtime_id, 0xDEADBEEF);
        assert_eq!(decoded.flags, 0b0011);
        assert_eq!(decoded.layer, 0);
    }

    #[test]
    fn block_event_round_trips() {
        let packet = BlockEvent {
            x: 12,
            y: 64,
            z: -3,
            event_type: BlockEvent::TYPE_CHANGE_CHEST_STATE,
            event_data: 1,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = BlockEvent::read(&mut reader).unwrap();
        assert_eq!(decoded.x, 12);
        assert_eq!(decoded.y, 64);
        assert_eq!(decoded.z, -3);
        assert_eq!(decoded.event_type, 1);
        assert_eq!(decoded.event_data, 1);
    }
}
