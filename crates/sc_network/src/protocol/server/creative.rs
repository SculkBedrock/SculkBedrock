use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// CreativeContent packet (0x91).
///
/// Sends the creative-mode item list to the client. The packet is required
/// even outside creative mode (an empty list is valid).
/// Layout: groups count (var_u32); per group: category (i32_le) + name
/// (string) + icon (slot); items count (var_u32); per item:
/// creative_net_id (var_u32) + slot data + group_id (var_u32).
#[derive(Debug, Clone, MinecraftPacket)]
pub struct CreativeContent {
    /// Creative item groups.
    pub groups: Vec<CreativeItemGroup>,
    /// Creative items.
    pub items: Vec<CreativeItem>,
}

impl CreativeContent {
    /// Empty CreativeContent packet (non-creative or spectator mode).
    pub fn empty() -> Self {
        Self {
            groups: Vec::new(),
            items: Vec::new(),
        }
    }
}

impl Default for CreativeContent {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, Clone)]
pub struct CreativeItemGroup {
    pub category: i32,
    pub name: String,
    pub icon_data: Vec<u8>, // Serialized slot data
}

#[derive(Debug, Clone)]
pub struct CreativeItem {
    pub creative_net_id: u32,
    pub slot_data: Vec<u8>, // Serialized slot data
    pub group_id: u32,
}

impl Writer for CreativeContent {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        // Group count.
        buf.write_var_u32(self.groups.len() as u32)?;
        for group in &self.groups {
            buf.write_u8(group.category as u8)?;
            buf.write_string(&group.name)?;
            buf.write(&group.icon_data)?;
        }

        // Item count.
        buf.write_var_u32(self.items.len() as u32)?;
        for item in &self.items {
            buf.write_var_u32(item.creative_net_id)?;
            buf.write(&item.slot_data)?;
            buf.write_var_u32(item.group_id)?;
        }

        Ok(())
    }
}

impl Reader<CreativeContent> for CreativeContent {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "CreativeContent decode is unsupported",
        ))
    }
}
