use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// UpdateBlockPacket (0x15): single-block change broadcast. BlockPos
/// components are all zigzag varint32.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct UpdateBlock {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Hashed block network id (StartGame block_network_ids_hashed=true).
    pub block_runtime_id: u32,
    pub flags: u32,
    /// 0 = block layer, 1 = second layer (e.g. waterlogged).
    pub layer: u32,
}

impl UpdateBlock {
    pub const FLAG_NEIGHBORS: u32 = 0b0001;
    pub const FLAG_NETWORK: u32 = 0b0010;
    pub const FLAG_NO_GRAPHIC: u32 = 0b0100;
    pub const FLAG_PRIORITY: u32 = 0b1000;
}

impl Writer for UpdateBlock {
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
}
