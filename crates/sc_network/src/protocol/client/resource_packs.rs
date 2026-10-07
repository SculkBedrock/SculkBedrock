use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ResponseEntry {
    pub uuid: String,
    pub version: String,
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct ResourcePackClientResponse {
    pub response_status: u8,
    pub entries: Vec<ResponseEntry>,
}

impl ResourcePackClientResponse {
    pub const STATUS_REFUSED: u8 = 1;
    pub const STATUS_SEND_PACKS: u8 = 2;
    pub const STATUS_HAVE_ALL_PACKS: u8 = 3;
    pub const STATUS_COMPLETED: u8 = 4;
    pub fn new() -> Self {
        Self {
            response_status: 0,
            entries: vec![],
        }
    }
}

impl Writer for ResourcePackClientResponse {
    fn write(&self, _buf: &mut ByteWriter) -> Result<(), Error> {
        Ok(())
    }
}

impl Reader<ResourcePackClientResponse> for ResourcePackClientResponse {
    fn read(buf: &mut ByteReader) -> Result<ResourcePackClientResponse, Error> {
        let mut packet = ResourcePackClientResponse::new();
        let protocol = crate::protocol::version::current_protocol_version();
        if protocol >= 2168 {
            // Status byte is zero-based (add 1 after reading), followed by
            // a type enum string. Entries exist only for SEND_PACKS,
            // with a varuint count.
            let status_wire = buf.read_u8()?;
            packet.response_status = status_wire.wrapping_add(1);
            let _type_string = buf.read_string()?;
            if packet.response_status == Self::STATUS_SEND_PACKS {
                let entries_len = buf.read_var_u32()? as usize;
                for _ in 0..entries_len {
                    let entry = buf.read_string()?;
                    let entry: Vec<&str> = entry.splitn(2, '_').collect();
                    packet.entries.push(ResponseEntry {
                        uuid: entry
                            .get(0)
                            .ok_or(Error::other("Cannot get uuid"))?
                            .to_string(),
                        version: entry
                            .get(1)
                            .ok_or(Error::other("Cannot get version"))?
                            .to_string(),
                    })
                }
            }
        } else {
            packet.response_status = buf.read_u8()?;
            let entries_len = buf.read_i16_le()? as u16;
            for _ in 0..entries_len {
                let entry = buf.read_string()?;
                let entry: Vec<&str> = entry.splitn(2, '_').collect();
                packet.entries.push(ResponseEntry {
                    uuid: entry
                        .get(0)
                        .ok_or(Error::other("Cannot get uuid"))?
                        .to_string(),
                    version: entry
                        .get(1)
                        .ok_or(Error::other("Cannot get version"))?
                        .to_string(),
                })
            }
        }
        Ok(packet)
    }
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct ResourcePackChunkRequest {
    pub pack_id: Uuid,
    pub chunk_index: u32,
}

impl Writer for ResourcePackChunkRequest {
    fn write(&self, _buf: &mut ByteWriter) -> Result<(), Error> {
        Ok(())
    }
}

impl Reader<ResourcePackChunkRequest> for ResourcePackChunkRequest {
    fn read(buf: &mut ByteReader) -> Result<ResourcePackChunkRequest, Error> {
        let raw = buf.read_string()?;
        let parts: Vec<&str> = raw.splitn(2, '_').collect();
        let uuid_str = parts[0];
        let pack_id = Uuid::parse_str(uuid_str)
            .map_err(|e| Error::other(format!("Invalid pack_id UUID: {}", e)))?;
        let chunk_index = buf.read_i32_le()? as u32;
        Ok(ResourcePackChunkRequest {
            pack_id,
            chunk_index,
        })
    }
}
