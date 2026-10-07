use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::writer::NbtWriter;
use sc_nbt::NbtValue;
use sc_network_macros::MinecraftPacket;
use std::io::Error;

#[derive(Debug, Clone, MinecraftPacket)]
pub struct ItemComponent {
    entries: Vec<ItemComponentEntry>,
}

impl ItemComponent {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
    pub fn set_entries(&mut self, entries: Vec<ItemComponentEntry>) {
        self.entries = entries;
    }
    pub fn get_entries(&self) -> &Vec<ItemComponentEntry> {
        &self.entries
    }
}

impl Reader<ItemComponent> for ItemComponent {
    fn read(_buf: &mut ByteReader) -> Result<ItemComponent, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ItemComponent decode is unsupported",
        ))
    }
}

impl Writer for ItemComponent {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.entries.len() as u32)?;
        for entry in &self.entries {
            buf.write_string(&entry.name)?;
            buf.write_u16_le(entry.runtime_id)?;
            buf.write_bool(entry.is_component_based)?;
            buf.write_var_i32(entry.version)?;
            let mut writer = NbtWriter::from_writer(buf);
            writer.write::<BedrockNetworkNbt>(&NbtValue::Compound(entry.data.clone()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ItemComponentEntry {
    pub name: String,
    pub runtime_id: u16,
    pub is_component_based: bool,
    pub version: i32,
    pub data: CompoundNbt,
}

impl ItemComponentEntry {
    pub fn new(name: String, data: CompoundNbt) -> Self {
        Self {
            name,
            runtime_id: 0,
            is_component_based: true,
            version: 0,
            data,
        }
    }
}
