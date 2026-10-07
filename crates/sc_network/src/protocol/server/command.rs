use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;
use uuid::Uuid;

/// CommandOutputPacket (0x4f).
///
/// Command result reply. The origin triple must match the triggering
/// CommandRequest so the client links output to that command call.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct CommandOutput {
    /// Origin type string id ("player" and similar).
    pub origin_type: String,
    pub uuid: Uuid,
    pub request_id: String,
    /// Player id, always fixed 64-bit little-endian.
    pub player_id: i64,
    pub output_type: u8,
    pub success_count: u32,
    pub messages: Vec<CommandOutputMessage>,
    /// Written only when output_type is TYPE_DATA_SET.
    pub data_set: String,
}

#[derive(Clone, Debug)]
pub struct CommandOutputMessage {
    pub success: bool,
    /// Translation key or raw text; shown raw when the client lacks the key.
    pub message_id: String,
    pub parameters: Vec<String>,
}

impl CommandOutput {
    pub const TYPE_LAST_OUTPUT: u8 = 1;
    pub const TYPE_SILENT: u8 = 2;
    pub const TYPE_ALL_OUTPUT: u8 = 3;
    pub const TYPE_DATA_SET: u8 = 4;
}

impl Writer for CommandOutput {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        // Origin: string type + uuid + string requestId + fixed i64LE playerId.
        buf.write_string(&self.origin_type)?;
        buf.write_uuid(&self.uuid)?;
        buf.write_string(&self.request_id)?;
        buf.write_i64_le(self.player_id)?;
        // Output type is a string id.
        let type_str = match self.output_type {
            Self::TYPE_SILENT => "Silent",
            Self::TYPE_ALL_OUTPUT => "AllOutput",
            Self::TYPE_DATA_SET => "DataSet",
            _ => "LastOutput",
        };
        buf.write_string(type_str)?;
        // successCount is fixed 32-bit little-endian.
        buf.write_i32_le(self.success_count as i32)?;
        buf.write_var_u32(self.messages.len() as u32)?;
        for message in &self.messages {
            buf.write_string(&message.message_id)?;
            buf.write_bool(message.success)?;
            buf.write_var_u32(message.parameters.len() as u32)?;
            for parameter in &message.parameters {
                buf.write_string(parameter)?;
            }
        }
        // dataSet:optional(bool + string;v898 writeOptionalNull).
        let has_data_set = self.output_type == Self::TYPE_DATA_SET && !self.data_set.is_empty();
        buf.write_bool(has_data_set)?;
        if has_data_set {
            buf.write_string(&self.data_set)?;
        }
        Ok(())
    }
}

impl Reader<CommandOutput> for CommandOutput {
    fn read(buf: &mut ByteReader) -> Result<CommandOutput, Error> {
        let origin_type = buf.read_string()?;
        let most = buf.read_u64_le()?;
        let least = buf.read_u64_le()?;
        let uuid = Uuid::from_u64_pair(most, least);
        let request_id = buf.read_string()?;
        let player_id = buf.read_i64_le()?;
        let output_type = match buf.read_string()?.as_str() {
            "Silent" => Self::TYPE_SILENT,
            "AllOutput" => Self::TYPE_ALL_OUTPUT,
            "DataSet" => Self::TYPE_DATA_SET,
            "LastOutput" => Self::TYPE_LAST_OUTPUT,
            output_type => {
                return Err(Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown command output type: {output_type}"),
                ));
            }
        };
        let success_count = buf.read_i32_le()? as u32;
        let message_count = buf.read_var_u32()? as usize;
        if message_count > 4096 {
            return Err(Error::new(
                std::io::ErrorKind::InvalidData,
                "too many command output messages",
            ));
        }
        let mut messages = Vec::with_capacity(message_count);
        for _ in 0..message_count {
            let message_id = buf.read_string()?;
            let success = buf.read_bool()?;
            let parameter_count = buf.read_var_u32()? as usize;
            if parameter_count > 4096 {
                return Err(Error::new(
                    std::io::ErrorKind::InvalidData,
                    "too many command output parameters",
                ));
            }
            let mut parameters = Vec::with_capacity(parameter_count);
            for _ in 0..parameter_count {
                parameters.push(buf.read_string()?);
            }
            messages.push(CommandOutputMessage {
                success,
                message_id,
                parameters,
            });
        }
        let data_set = if buf.read_bool()? {
            buf.read_string()?
        } else {
            String::new()
        };
        Ok(Self {
            origin_type,
            uuid,
            request_id,
            player_id,
            output_type,
            success_count,
            messages,
            data_set,
        })
    }
}
