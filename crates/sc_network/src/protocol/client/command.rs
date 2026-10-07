use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;
use uuid::Uuid;

/// CommandRequestPacket (0x4d), sent when a client runs a command.
///
/// Layout: `command(string) + origin{type(string) + uuid(16B) +
/// requestId(string) + playerId(long LE)} + internal(bool) + version(string)`.
/// Origin type is a string ("player" and similar); playerId is always 8 bytes.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct CommandRequest {
    pub command: String,
    /// Origin type string ("player" and similar).
    pub origin_type: String,
    pub uuid: Uuid,
    pub request_id: String,
    pub player_id: i64,
    pub internal: bool,
    pub version: String,
}

impl Reader<CommandRequest> for CommandRequest {
    fn read(buf: &mut ByteReader) -> Result<CommandRequest, Error> {
        let command = buf.read_string()?;
        let origin_type = buf.read_string()?;
        let most = buf.read_u64_le()?;
        let least = buf.read_u64_le()?;
        let uuid = Uuid::from_u64_pair(most, least);
        let request_id = buf.read_string()?;
        let player_id = buf.read_i64_le()?;
        let internal = buf.read_bool()?;
        let version = buf.read_string()?;
        Ok(Self {
            command,
            origin_type,
            uuid,
            request_id,
            player_id,
            internal,
            version,
        })
    }
}

impl Writer for CommandRequest {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_string(&self.command)?;
        buf.write_string(&self.origin_type)?;
        buf.write_uuid(&self.uuid)?;
        buf.write_string(&self.request_id)?;
        buf.write_i64_le(self.player_id)?;
        buf.write_bool(self.internal)?;
        buf.write_string(&self.version)
    }
}
