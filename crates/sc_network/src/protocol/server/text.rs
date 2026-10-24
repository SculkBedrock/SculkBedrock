use std::io::{Error, ErrorKind};

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

/// TextPacket 的 body type 命名空间（大小写对齐线协议名称）。
///
/// TextPacket body type namespace (casing matches the wire-protocol names).
#[allow(non_snake_case)]
pub mod TextBodyType {
    /// 仅消息文本。
    ///
    /// Message text only.
    pub const MESSAGE_ONLY: u8 = 0;
    /// 作者名 + 消息文本。
    ///
    /// Author name + message text.
    pub const AUTHOR_AND_MESSAGE: u8 = 1;
    /// 消息文本 + 参数列表。
    ///
    /// Message text + parameter list.
    pub const MESSAGE_AND_PARAMS: u8 = 2;
}

/// TextPacket 的 message type 命名空间（大小写对齐线协议名称）。
///
/// TextPacket message type namespace (casing matches the wire-protocol names).
#[allow(non_snake_case)]
pub mod TextMessageType {
    /// 原文消息。
    ///
    /// Raw message.
    pub const RAW: u8 = 0;
    /// 聊天消息。
    ///
    /// Chat message.
    pub const CHAT: u8 = 1;
    /// 需本地化翻译的消息（带参数）。
    ///
    /// Localized translate message (with parameters).
    pub const TRANSLATE: u8 = 2;
    /// 弹窗（Popup）。
    ///
    /// Popup.
    pub const POPUP: u8 = 3;
    /// 唱片机弹窗。
    ///
    /// Jukebox popup.
    pub const JUKEBOX_POPUP: u8 = 4;
    /// 提示条（Tip）。
    ///
    /// Tip.
    pub const TIP: u8 = 5;
    /// 系统消息。
    ///
    /// System message.
    pub const SYSTEM_MESSAGE: u8 = 6;
    /// 私聊耳语。
    ///
    /// Whisper.
    pub const WHISPER: u8 = 7;
    /// 公告。
    ///
    /// Announcement.
    pub const ANNOUNCEMENT: u8 = 8;
    /// 对物体的耳语。
    ///
    /// Object whisper.
    pub const OBJECT_WHISPER: u8 = 9;
    /// 物体消息。
    ///
    /// Object message.
    pub const OBJECT: u8 = 10;
    /// 物体公告。
    ///
    /// Object announcement.
    pub const OBJECT_ANNOUNCEMENT: u8 = 11;
}

/// TextPacket (0x0a)：聊天/系统/弹窗等文本消息包。
///
/// TextPacket (0x0a): chat, system, popup and other text message packet.
///
/// body type 由 message type 推导，不同 body type 的字段布局不同。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct TextPacket {
    /// 是否需要客户端本地化翻译。
    ///
    /// Whether the client should localize the message.
    pub localize: bool,
    /// 包体布局类型，见 [`TextBodyType`]。
    ///
    /// Body layout type, see [`TextBodyType`].
    pub body_type: u8,
    /// 消息类型，见 [`TextMessageType`]。
    ///
    /// Message type, see [`TextMessageType`].
    pub message_type: u8,
    /// 消息来源（作者名）。
    ///
    /// Message source (author name).
    pub source: String,
    /// 消息正文（翻译键或原文）。
    ///
    /// Message body (translation key or raw text).
    pub message: String,
    /// 翻译参数列表（最多 4 个）。
    ///
    /// Translation parameters (at most four).
    pub parameters: Vec<String>,
    /// 发送者 XUID。
    ///
    /// Sender XUID.
    pub senders_xuid: String,
    /// 发送者平台 id。
    ///
    /// Sender platform id.
    pub platform_id: String,
    /// 过滤后的消息；无过滤时为空串（无条件字符串，无 bool 前缀）。
    ///
    /// Filtered message; empty when unfiltered (an unconditional string with
    /// no bool prefix).
    pub filtered_message: String,
}

impl TextPacket {
    /// 构造一条系统消息 TextPacket。
    ///
    /// Builds a system-message TextPacket.
    ///
    /// - `message`: 消息正文
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

    /// 根据消息类型推导包体布局类型；未知类型返回 `None`。
    ///
    /// Derives the body layout type from the message type; returns `None`
    /// for unknown types.
    ///
    /// - `message_type`: 消息类型 id
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
    /// 写入 TextPacket 包体（按推导出的 body type 布局）。
    ///
    /// Writes the TextPacket body using the derived body-type layout.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 消息类型或包体类型未知、参数超过 4 个时返回
    /// [`std::io::ErrorKind::InvalidData`] 错误。
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
    /// 解码 TextPacket 包体。
    ///
    /// Decodes a TextPacket body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 包体类型未知、参数超过 4 个或缓冲区不足时返回错误。
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
