use std::io::{Error, ErrorKind};

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

pub mod TextBodyType {
    pub const MESSAGE_ONLY: u8 = 0;
    pub const AUTHOR_AND_MESSAGE: u8 = 1;
    pub const MESSAGE_AND_PARAMS: u8 = 2;
}

pub mod TextMessageType {
    pub const RAW: u8 = 0;
    pub const CHAT: u8 = 1;
    pub const TRANSLATE: u8 = 2;
    pub const POPUP: u8 = 3;
    pub const JUKEBOX_POPUP: u8 = 4;
    pub const TIP: u8 = 5;
    pub const SYSTEM_MESSAGE: u8 = 6;
    pub const WHISPER: u8 = 7;
    pub const ANNOUNCEMENT: u8 = 8;
    pub const OBJECT_WHISPER: u8 = 9;
    pub const OBJECT: u8 = 10;
    pub const OBJECT_ANNOUNCEMENT: u8 = 11;
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct TextPacket {
    pub localize: bool,
    pub body_type: u8,
    pub message_type: u8,
    pub source: String,
    pub message: String,
    pub parameters: Vec<String>,
    pub senders_xuid: String,
    pub platform_id: String,
    pub filtered_message: String,
}

impl TextPacket {
    pub fn system_message(message: impl Into<String>) -> Self {
        Self {
            localize: false,
            body_type: TextBodyType::MESSAGE_ONLY,
            message_type: TextMessageType::SYSTEM_MESSAGE,
            source: String::new(),
            message: message.into(),
            parameters: Vec::new(),
            senders_xuid: String::new(),
            platform_id: String::new(),
            filtered_message: String::new(),
        }
    }

    fn body_type_for_message(message_type: u8) -> Option<u8> {
        match message_type {
            TextMessageType::RAW
            | TextMessageType::TIP
            | TextMessageType::SYSTEM_MESSAGE
            | TextMessageType::OBJECT_WHISPER
            | TextMessageType::OBJECT
            | TextMessageType::OBJECT_ANNOUNCEMENT => Some(TextBodyType::MESSAGE_ONLY),
            TextMessageType::CHAT | TextMessageType::WHISPER | TextMessageType::ANNOUNCEMENT => {
                Some(TextBodyType::AUTHOR_AND_MESSAGE)
            }
            TextMessageType::TRANSLATE
            | TextMessageType::POPUP
            | TextMessageType::JUKEBOX_POPUP => Some(TextBodyType::MESSAGE_AND_PARAMS),
            _ => None,
        }
    }
}

impl Writer for TextPacket {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        let body_type = Self::body_type_for_message(self.message_type)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "unknown text message type"))?;
        buf.write_bool(self.localize || self.message_type == TextMessageType::TRANSLATE)?;
        buf.write_u8(body_type)?;
        buf.write_u8(self.message_type)?;
        match body_type {
            TextBodyType::AUTHOR_AND_MESSAGE => {
                buf.write_string(&self.source)?;
                buf.write_string(if self.message.is_empty() {
                    " "
                } else {
                    &self.message
                })?;
            }
            TextBodyType::MESSAGE_AND_PARAMS => {
                buf.write_string(if self.message.is_empty() {
                    " "
                } else {
                    &self.message
                })?;
                if self.parameters.len() > 4 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "text parameters exceed four",
                    ));
                }
                buf.write_var_u32(self.parameters.len() as u32)?;
                for parameter in &self.parameters {
                    buf.write_string(parameter)?;
                }
            }
            TextBodyType::MESSAGE_ONLY => buf.write_string(if self.message.is_empty() {
                " "
            } else {
                &self.message
            })?,
            _ => return Err(Error::new(ErrorKind::InvalidData, "unknown text body type")),
        }
        buf.write_string(&self.senders_xuid)?;
        buf.write_string(&self.platform_id)?;
        // filteredMessage is an unconditional string (empty if unset),
        // with no bool prefix.
        buf.write_string(&self.filtered_message)?;
        Ok(())
    }
}

impl Reader<TextPacket> for TextPacket {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let localize = buf.read_bool()?;
        let body_type = buf.read_u8()?;
        let message_type = buf.read_u8()?;
        let (source, message, parameters) = match body_type {
            TextBodyType::MESSAGE_ONLY => (String::new(), buf.read_string()?, Vec::new()),
            TextBodyType::AUTHOR_AND_MESSAGE => {
                let source = buf.read_string()?;
                let message = buf.read_string()?;
                (source, message, Vec::new())
            }
            TextBodyType::MESSAGE_AND_PARAMS => {
                let message = buf.read_string()?;
                let count = buf.read_var_u32()? as usize;
                if count > 4 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "text parameters exceed four",
                    ));
                }
                let mut parameters = Vec::with_capacity(count);
                for _ in 0..count {
                    parameters.push(buf.read_string()?);
                }
                (String::new(), message, parameters)
            }
            _ => return Err(Error::new(ErrorKind::InvalidData, "unknown text body type")),
        };
        let senders_xuid = buf.read_string()?;
        let platform_id = buf.read_string()?;
        let filtered_message = buf.read_string()?;
        Ok(Self {
            localize,
            body_type,
            message_type,
            source,
            message,
            parameters,
            senders_xuid,
            platform_id,
            filtered_message,
        })
    }
}
