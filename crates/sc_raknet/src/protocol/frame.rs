use crate::protocol::reliability::Reliability;
use crate::utils::seq24;
use log::debug;
use sc_binary::{
    interfaces::{Reader, Writer},
    BinaryIo, ByteReader, ByteWriter,
};

/// 单个数据报中允许承载的最大帧数。
///
/// Maximum number of frames allowed inside a single datagram.
const MAX_FRAMES_PER_PACKET: usize = 128;
/// 单帧体的最大长度（位长字段为 16 位，需留一位余量）。
///
/// Maximum body size of a single frame (the bit-length field is 16 bits,
/// leaving one bit of headroom).
const MAX_FRAME_BODY_SIZE: usize = u16::MAX as usize / 8;

/// 给定分片的元信息，用于确定如何重组该帧。
///
/// The information for the given fragment; used to determine how to
/// reassemble the frame.
#[derive(Debug, Clone, BinaryIo)]
pub struct FragmentMeta {
    /// 分片所属消息的总大小（字节）。
    ///
    /// Total size (in bytes) of the message this fragment belongs to.
    pub(crate) size: u32,
    /// 分片所属消息的标识，同一消息的各分片相同。
    ///
    /// Identifier of the owning message; identical across its fragments.
    pub(crate) id: u16,
    /// 该分片在消息中的序号（从 0 开始）。
    ///
    /// Index of this fragment within the message (starting at 0).
    pub(crate) index: u32,
}

impl FragmentMeta {
    /// 用给定的大小、标识与序号创建分片元信息。
    ///
    /// Creates a new fragment meta with the given size, id, and index.
    pub fn new(size: u32, id: u16, index: u32) -> Self {
        Self { size, id, index }
    }
}

/// 帧数据包：一个或多个帧的封装，用于以可靠方式发送数据。
///
/// Frames are an encapsulation of a packet or packets; they are used to
/// send packets to the connection in a reliable way.
#[derive(Debug, Clone)]
pub struct FramePacket {
    /// 数据报序号（24 位，写回时截断）。
    ///
    /// Datagram sequence number (24-bit, truncated when written back).
    pub sequence: u32,
    /// 本数据报携带的帧列表。
    ///
    /// The frames carried by this datagram.
    pub frames: Vec<Frame>,
    /// 解析时固定的可靠性等级（总是 [`Reliability::ReliableOrd`]）。
    ///
    /// Reliability fixed at parse time (always
    /// [`Reliability::ReliableOrd`]).
    pub reliability: Reliability,
}

impl FramePacket {
    /// 创建空的数据报帧包（可靠有序、序号 0）。
    ///
    /// Creates an empty frame packet (reliable ordered, sequence 0).
    pub fn new() -> Self {
        Self {
            sequence: 0,
            frames: Vec::new(),
            reliability: Reliability::ReliableOrd,
        }
    }
}

impl Reader<FramePacket> for FramePacket {
    /// 从缓冲区解码帧数据报：校验报文 ID、读序号并循环读帧。
    ///
    /// 帧数超过 [`MAX_FRAMES_PER_PACKET`] 或帧解析失败时返回
    /// `InvalidData`。
    ///
    /// Decodes a frame datagram from the buffer: validates the packet ID,
    /// reads the sequence, then reads frames in a loop.
    ///
    /// Returns `InvalidData` when the frame count exceeds
    /// [`MAX_FRAMES_PER_PACKET`] or a frame fails to parse.
    fn read(buf: &mut ByteReader) -> Result<FramePacket, std::io::Error> {
        // FRAME PACKET HEADER
        let id = buf.read_u8()?;
        match id {
            0x80..=0x8d => {}
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid Frame Packet ID",
                ))
            }
        }
        let mut frames: Vec<Frame> = Vec::new();

        let sequence = buf.read_u24_le()?;

        while !buf.as_slice().is_empty() {
            if frames.len() >= MAX_FRAMES_PER_PACKET {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "frame count exceeds limit",
                ));
            }
            let frame_pos = buf.read_type::<Frame>();
            if let Ok(frame) = frame_pos {
                frames.push(frame);
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid frame payload",
                ));
            }
        }

        Ok(FramePacket {
            sequence,
            frames,
            reliability: Reliability::ReliableOrd,
        })
    }
}

impl Writer for FramePacket {
    /// 把帧数据报编码写入缓冲区（报文 ID 固定为 0x84）。
    ///
    /// 帧数超过 [`MAX_FRAMES_PER_PACKET`] 时返回 `InvalidData`。
    ///
    /// Encodes the frame datagram into the buffer (packet ID is always
    /// 0x84).
    ///
    /// Returns `InvalidData` when the frame count exceeds
    /// [`MAX_FRAMES_PER_PACKET`].
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        if self.frames.len() > MAX_FRAMES_PER_PACKET {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "frame count exceeds limit",
            ));
        }
        buf.write_u8(0x84)?;
        buf.write_u24_le(seq24(self.sequence))?;

        for frame in &self.frames {
            buf.write(frame.write_to_bytes()?.as_slice())?;
        }

        Ok(())
    }
}

/// 单个数据帧，由负载构造而来。
///
/// An individual data frame, constructed from a payload.
#[derive(Debug, Clone)]
pub struct Frame {
    /// 帧标志：前 3 位存放可靠性，第 4 位表示是否为分片。
    ///
    /// The flags for this frame: the first 3 bits are reserved for the
    /// reliability while the 4th bit represents whether this is a
    /// fragment.
    pub flags: u8,
    /// 帧体长度（内部按 24 位存储，取值必须在对应范围内）。
    ///
    /// The length of the body of the frame, sized to 24 bits internally,
    /// so any value here must be within that range.
    pub size: u16,
    /// 帧的可靠索引（可靠帧存在）。
    ///
    /// The reliable index of the frame (present if reliable).
    pub reliable_index: Option<u32>,
    /// 帧的排序索引（排序帧存在），用于确定帧在列表中的位置。
    ///
    /// The sequenced index of the frame (if sequenced); used to determine
    /// the position in the frame list.
    pub sequence_index: Option<u32>,
    /// 帧的有序索引（有序帧存在），用于确定帧在列表中的位置；
    /// 与排序索引不同，它更偏向按指定方式编排数据包次序。
    ///
    /// The order index of the frame (if ordered); used to determine the
    /// position in the frame list. Unlike the sequence index, it is used
    /// more to sequence packets in a specific manner.
    pub order_index: Option<u32>,
    /// 帧的有序信道（有序帧存在），用于存放该帧的排序信息。
    ///
    /// The order channel of the frame (if ordered); used to store order
    /// information for the frame.
    pub order_channel: Option<u8>,
    /// 分片元信息（帧被拆分时存在），用于确定如何重组该帧。
    ///
    /// The information for fragmentation (if the frame is split into
    /// parts); used to determine how to reassemble the frame.
    pub fragment_meta: Option<FragmentMeta>,
    /// 帧的可靠性：本质用于在丢帧时重发；否则帧以不可靠方式发送。
    ///
    /// The reliability of this frame, essentially used to save frames and
    /// send them back if they are lost; otherwise the frame is sent
    /// unreliably.
    pub reliability: Reliability,
    /// 帧体，即帧携带的负载。
    ///
    /// The body of the frame, i.e. the payload carried by the frame.
    pub body: Vec<u8>,
}

impl Frame {
    /// 初始化一个空的不可靠帧，通常用于向其注入数据。
    ///
    /// Initializes a new empty frame that is Unreliable; usually used to
    /// inject data into.
    pub fn init() -> Self {
        Self {
            flags: 0,
            size: 0,
            reliable_index: None,
            sequence_index: None,
            order_index: None,
            order_channel: None,
            fragment_meta: None,
            reliability: Reliability::Unreliable,
            body: Vec::new(),
        }
    }

    /// 以给定可靠性初始化新帧；`body` 为 `None` 时帧体为空。
    ///
    /// Initializes a new frame with the given reliability; the body is
    /// empty when `body` is `None`.
    pub fn new(reliability: Reliability, body: Option<&[u8]>) -> Self {
        Self {
            flags: 0,
            size: body
                .map(|b| b.len().min(usize::from(u16::MAX)) as u16)
                .unwrap_or(0),
            reliable_index: None,
            sequence_index: None,
            order_index: None,
            order_channel: None,
            fragment_meta: None,
            reliability,
            body: body.unwrap_or(&[]).to_vec(),
        }
    }

    /// 该帧是否为分片。
    ///
    /// Whether or not the frame is fragmented.
    pub fn is_fragmented(&self) -> bool {
        self.fragment_meta.is_some()
    }

    /// 该帧是否既排序又可靠。
    ///
    /// Whether or not the frame is sequenced and reliable.
    pub fn is_sequenced(&self) -> bool {
        self.reliability.is_sequenced()
    }

    /// 附加分片元信息（builder 风格）。
    ///
    /// Attaches fragment metadata (builder style).
    pub fn with_meta(mut self, meta: FragmentMeta) -> Self {
        self.fragment_meta = Some(meta);
        self
    }
}

impl Reader<Frame> for Frame {
    /// 从缓冲区解码单帧：按可靠性读取可靠/排序/有序索引与分片元信息。
    ///
    /// 位长非字节对齐或帧体超出剩余输入时返回错误。
    ///
    /// Decodes a single frame from the buffer: reads reliable/sequenced/
    /// ordered indices and fragment metadata according to reliability.
    ///
    /// Errors when the bit length is not byte-aligned or the body exceeds
    /// the remaining input.
    fn read(buf: &mut ByteReader) -> Result<Frame, std::io::Error> {
        let mut frame = Frame::init();

        frame.flags = buf.read_u8()?;
        frame.reliability = Reliability::from_flags(frame.flags);

        let bit_size = buf.read_u16()?;
        if bit_size % 8 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "frame body bit length is not byte aligned",
            ));
        }
        frame.size = bit_size / 8;

        if frame.reliability.is_reliable() {
            frame.reliable_index = Some(buf.read_u24_le()?);
        }

        if frame.reliability.is_sequenced() {
            frame.sequence_index = Some(buf.read_u24_le()?);
        }

        if frame.reliability.is_ordered() {
            frame.order_index = Some(buf.read_u24_le()?);
            frame.order_channel = Some(buf.read_u8()?);
        }

        if (frame.flags & 0x10) > 0 {
            frame.fragment_meta = Some(FragmentMeta::read(buf)?);
        }

        let body_size = usize::from(frame.size);
        if body_size > buf.as_slice().len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "frame body exceeds remaining input",
            ));
        }
        let mut body = vec![0; body_size];
        buf.read(&mut body).map_err(|e| {
            debug!("[DECODE_ERR] Error reading frame body: {:?}", e);
            e
        })?;
        frame.body = body;

        Ok(frame)
    }
}

impl Writer for Frame {
    /// 把单帧编码写入缓冲区：写标志、位长与各索引后跟帧体。
    ///
    /// 帧体超过 [`MAX_FRAME_BODY_SIZE`] 或与声明位长不符时返回
    /// `InvalidData`。
    ///
    /// Encodes a single frame into the buffer: flags, bit length, the
    /// relevant indices, then the body.
    ///
    /// Returns `InvalidData` when the body exceeds
    /// [`MAX_FRAME_BODY_SIZE`] or mismatches the declared bit length.
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        if self.body.len() > MAX_FRAME_BODY_SIZE || usize::from(self.size) != self.body.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "frame body length does not fit declared bit length",
            ));
        }

        let mut flags = self.reliability.to_flags();

        // check whether or not this frame is fragmented, if it is, set the fragment flag
        if self.fragment_meta.is_some() {
            flags |= 0x10;
        }

        buf.write_u8(flags)?;
        let bit_size = u16::try_from(self.body.len() * 8).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "frame body is too large")
        })?;
        buf.write_u16(bit_size)?;

        if self.reliability.is_reliable() {
            buf.write_u24_le(seq24(self.reliable_index.unwrap_or(0)))?;
        }

        if self.reliability.is_sequenced() {
            buf.write_u24_le(seq24(self.sequence_index.unwrap_or(0)))?;
        }

        if self.reliability.is_ordered() {
            buf.write_u24_le(seq24(self.order_index.unwrap_or(0)))?;
            buf.write_u8(self.order_channel.unwrap_or(0))?;
        }

        if self.fragment_meta.is_some() {
            let fragment_meta = self.fragment_meta.as_ref().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "missing fragment metadata")
            })?;
            buf.write(fragment_meta.write_to_bytes()?.as_slice())?;
        }

        buf.write(&self.body)?;

        Ok(())
    }
}
