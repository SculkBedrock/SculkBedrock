use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;
use uuid::Uuid;

/// CommandOutputPacket (0x4f)：命令执行结果回包。
///
/// CommandOutputPacket (0x4f): command execution result reply.
///
/// origin 三元组必须与触发它的 CommandRequest 一致，客户端据此把输出
/// 关联回对应的命令调用。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct CommandOutput {
    /// 来源类型字符串 id（如 "player"）。
    ///
    /// Origin type string id (e.g. "player").
    pub origin_type: String,
    /// 命令发起者 UUID。
    ///
    /// UUID of the command issuer.
    pub uuid: Uuid,
    /// 对应 CommandRequest 的请求 id。
    ///
    /// Request id matching the triggering CommandRequest.
    pub request_id: String,
    /// 玩家 id，固定 64 位小端。
    ///
    /// Player id, always fixed 64-bit little-endian.
    pub player_id: i64,
    /// 输出类型，见 [`CommandOutput`] 的 TYPE_* 常量。
    ///
    /// Output type, see the TYPE_* constants on [`CommandOutput`].
    pub output_type: u8,
    /// 成功执行的命令数量。
    ///
    /// Number of successfully executed commands.
    pub success_count: u32,
    /// 输出消息列表。
    ///
    /// Output messages.
    pub messages: Vec<CommandOutputMessage>,
    /// 仅当 output_type 为 [`CommandOutput::TYPE_DATA_SET`] 时写入。
    ///
    /// Written only when output_type is [`CommandOutput::TYPE_DATA_SET`].
    pub data_set: String,
}

/// CommandOutputMessage：单条命令输出消息。
///
/// CommandOutputMessage: a single command output message.
#[derive(Clone, Debug)]
pub struct CommandOutputMessage {
    /// 该条消息是否表示成功。
    ///
    /// Whether this message reports success.
    pub success: bool,
    /// 翻译键或原文；客户端缺少该键时按原文展示。
    ///
    /// Translation key or raw text; shown raw when the client lacks the key.
    pub message_id: String,
    /// 消息参数列表（按序填入翻译键占位符）。
    ///
    /// Message parameters (fill the translation key placeholders in order).
    pub parameters: Vec<String>,
}

impl CommandOutput {
    /// TYPE_LAST_OUTPUT：仅发送最后一条输出。
    ///
    /// TYPE_LAST_OUTPUT: send only the last output.
    pub const TYPE_LAST_OUTPUT: u8 = 1;
    /// TYPE_SILENT：静默，不显示输出。
    ///
    /// TYPE_SILENT: silent, no output shown.
    pub const TYPE_SILENT: u8 = 2;
    /// TYPE_ALL_OUTPUT：发送全部输出。
    ///
    /// TYPE_ALL_OUTPUT: send all outputs.
    pub const TYPE_ALL_OUTPUT: u8 = 3;
    /// TYPE_DATA_SET：携带 dataSet 字符串。
    ///
    /// TYPE_DATA_SET: carries a dataSet string.
    pub const TYPE_DATA_SET: u8 = 4;
}

impl Writer for CommandOutput {
    /// 写入 CommandOutput 包体。
    ///
    /// Writes the CommandOutput packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
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
    /// 解码 CommandOutput 包体。
    ///
    /// Decodes a CommandOutput packet body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 输出类型字符串未知、消息或参数数量超过 4096 上限、缓冲区不足时
    /// 返回 [`std::io::ErrorKind::InvalidData`] 错误。
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
