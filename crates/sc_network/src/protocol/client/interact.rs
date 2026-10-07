//! Interact (0x21), client -> server.
//!
//! Bedrock sends this packet when the player presses E.  Opening the
//! player's own inventory is action 6; the optional coordinates are kept
//! because they are part of the wire format even though this action does not
//! use them.

use std::io::Error;

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

pub mod InteractAction {
    pub const VEHICLE_EXIT: u8 = 3;
    pub const MOUSEOVER: u8 = 4;
    pub const OPEN_NPC: u8 = 5;
    pub const OPEN_INVENTORY: u8 = 6;
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct Interact {
    pub action: u8,
    pub target: u64,
    pub has_position: bool,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Reader<Interact> for Interact {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let action = buf.read_u8()?;
        let target = buf.read_var_u64()?;
        let has_position = buf.read_bool()?;
        let (x, y, z) = if has_position {
            (buf.read_f32_le()?, buf.read_f32_le()?, buf.read_f32_le()?)
        } else {
            (0.0, 0.0, 0.0)
        };
        Ok(Self {
            action,
            target,
            has_position,
            x,
            y,
            z,
        })
    }
}

impl Writer for Interact {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.action)?;
        buf.write_var_u64(self.target)?;
        buf.write_bool(self.has_position)?;
        if self.has_position {
            buf.write_f32_le(self.x)?;
            buf.write_f32_le(self.y)?;
            buf.write_f32_le(self.z)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};

    #[test]
    fn open_inventory_round_trip_without_position() {
        let packet = Interact {
            action: InteractAction::OPEN_INVENTORY,
            target: 42,
            has_position: false,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), &[6, 42, 0]);

        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = Interact::read(&mut reader).unwrap();
        assert_eq!(decoded.action, InteractAction::OPEN_INVENTORY);
        assert_eq!(decoded.target, 42);
        assert!(!decoded.has_position);
    }
}
