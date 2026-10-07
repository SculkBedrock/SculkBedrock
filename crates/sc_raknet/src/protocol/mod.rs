use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};

pub mod ack;
pub mod frame;
pub mod mcbe;
pub mod packet;
pub mod reliability;
pub(crate) const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x0, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

pub const RAKNET_HEADER_FRAME_OVERHEAD: u16 = 20 + 8 + 8 + 4 + 20;

pub const MAX_FRAGS: u32 = 1024;

#[derive(Debug, Clone)]
pub struct Magic;

impl Magic {
    pub fn new() -> Self {
        Self {}
    }
}

impl Reader<Magic> for Magic {
    fn read(buf: &mut ByteReader) -> Result<Magic, std::io::Error> {
        let mut magic = [0u8; 16];
        buf.read(&mut magic)?;

        if magic != MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid magic",
            ));
        }

        Ok(Magic)
    }
}

impl Writer for Magic {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
        buf.write(&MAGIC)?;
        Ok(())
    }
}
