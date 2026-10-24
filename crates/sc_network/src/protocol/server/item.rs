use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::writer::NbtWriter;
use sc_nbt::NbtValue;
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// ItemComponentPacket：物品组件注册表（ItemComponentPacket）。
///
/// ItemComponentPacket: item component registry packet.
///
/// 逐项写入组件名、运行时 id、是否组件化、版本与 NBT 数据。
#[derive(Debug, Clone, MinecraftPacket)]
pub struct ItemComponent {
    /// 组件条目列表。
    ///
    /// Component entries.
    entries: Vec<ItemComponentEntry>,
}

impl ItemComponent {
    /// 构造空的组件注册表。
    ///
    /// Builds an empty component registry.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
    /// 设置组件条目列表。
    ///
    /// Sets the component entry list.
    ///
    /// - `entries`: 条目列表
    pub fn set_entries(&mut self, entries: Vec<ItemComponentEntry>) {
        self.entries = entries;
    }
    /// 获取组件条目列表。
    ///
    /// Returns the component entry list.
    pub fn get_entries(&self) -> &Vec<ItemComponentEntry> {
        &self.entries
    }
}

impl Reader<ItemComponent> for ItemComponent {
    /// 不支持解码：服务端只发送组件注册表。
    ///
    /// Decoding is unsupported: the server only sends the registry.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<ItemComponent, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ItemComponent decode is unsupported",
        ))
    }
}

impl Writer for ItemComponent {
    /// 写入 ItemComponent 包体。
    ///
    /// Writes the ItemComponent packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// NBT 序列化或写入缓冲区失败时返回错误。
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

/// ItemComponentEntry：单个物品组件条目。
///
/// ItemComponentEntry: a single item component entry.
#[derive(Debug, Clone)]
pub struct ItemComponentEntry {
    /// 组件名称。
    ///
    /// Component name.
    pub name: String,
    /// 组件运行时 id。
    ///
    /// Component runtime id.
    pub runtime_id: u16,
    /// 是否基于组件系统。
    ///
    /// Whether the item is component-based.
    pub is_component_based: bool,
    /// 组件数据版本。
    ///
    /// Component data version.
    pub version: i32,
    /// 组件 NBT 数据。
    ///
    /// Component NBT data.
    pub data: CompoundNbt,
}

impl ItemComponentEntry {
    /// 用名称与 NBT 数据构造条目（运行时 id 为 0、版本为 0）。
    ///
    /// Builds an entry from a name and NBT data (runtime id 0, version 0).
    ///
    /// - `name`: 组件名称
    /// - `data`: 组件 NBT 数据
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
