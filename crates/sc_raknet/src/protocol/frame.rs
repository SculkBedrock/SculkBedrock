use crate::protocol::reliability::Reliability;
use crate::utils::seq24;
use log::debug;
use sc_binary::{
    interfaces::{Reader, Writer},
    BinaryIo, ByteReader, ByteWriter,
};

const MAX_FRAMES_PER_PACKET: usize = 128;
const MAX_FRAME_BODY_SIZE: usize = u16::MAX as usize / 8;

/// The information for the given fragment.
/// This is used to determine how to reassemble the frame.
#[derive(Debug, Clone, BinaryIo)]
pub struct FragmentMeta {
    pub(crate) size: u32,
    pub(crate) id: u16,
    pub(crate) index: u32,
}

impl FragmentMeta {
    /// Creates a new fragment meta with the given size, id, and index.
    pub fn new(size: u32, id: u16, index: u32) -> Self {
        Self { size, id, index }
    }
}

/// Frames are a encapsulation of a packet or packets.
/// They are used to send packets to the connection in a reliable way.
#[derive(Debug, Clone)]
pub struct FramePacket {
    pub sequence: u32,
    pub frames: Vec<Frame>,
    pub reliability: Reliability,
}

impl FramePacket {
    /// Creates an empty frame packet.
    pub fn new() -> Self {
        Self {
            sequence: 0,
            frames: Vec::new(),
            reliability: Reliability::ReliableOrd,
        }
    }
}

impl Reader<FramePacket> for FramePacket {
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

/// An individual data frame, these are constructed from a payload.
#[derive(Debug, Clone)]
pub struct Frame {
    /// The flags for this frame, the first 3 bits are reserved for the reliability while the 4th
    /// bit is used to represent if this is a fragment.
    pub flags: u8,
    /// The length of the body of the frame.
    /// This is sized to 24 bits internally, so any number here must be within that range.
    pub size: u16,
    /// The Reliable index of the frame (if reliable)
    pub reliable_index: Option<u32>,
    /// The sequenced index of the frame (if sequenced)
    /// This is used to determine the position in frame list.
    pub sequence_index: Option<u32>,
    /// The order index of the frame (if ordered)
    /// This is used to determine the position in frame list,
    /// This is different from the sequence index in that it is
    /// used more to sequence packets in a specific manner.
    pub order_index: Option<u32>,
    /// The order channel of the frame (if ordered)
    /// This is used to store order information for the frame.
    pub order_channel: Option<u8>,
    /// The information for fragmentation (if the frame is split into parts)
    /// This is used to determine how to reassemble the frame.
    pub fragment_meta: Option<FragmentMeta>,
    /// The reliability of this frame, this is essentially used to save frames and send them back if
    /// they are lost. Otherwise, the frame is sent unreliably.
    pub reliability: Reliability,
    /// The body of the frame, this is the payload of the frame.
    pub body: Vec<u8>,
}

impl Frame {
    /// Initializes a new empty frame that is Unreliable.
    /// This is usually used to inject data into.
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

    /// Initializes a new frame with the given reliability.
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

    /// Whether or not the frame is fragmented.
    pub fn is_fragmented(&self) -> bool {
        self.fragment_meta.is_some()
    }

    /// Whether or not the frame is sequenced and reliable.
    pub fn is_sequenced(&self) -> bool {
        self.reliability.is_sequenced()
    }

    pub fn with_meta(mut self, meta: FragmentMeta) -> Self {
        self.fragment_meta = Some(meta);
        self
    }
}

impl Reader<Frame> for Frame {
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
