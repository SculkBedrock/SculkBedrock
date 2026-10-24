use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// CreativeContentPacket (0x91)：向客户端发送创造模式物品列表。
///
/// CreativeContentPacket (0x91): sends the creative-mode item list to the
/// client. The packet is required even outside creative mode (an empty list
/// is valid).
///
/// 布局：分组数（var_u32）；每组：类别（i32_le）+ 名称（string）+
/// 图标（序列化槽位数据）；物品数（var_u32）；每件物品：
/// creative_net_id（var_u32）+ 槽位数据 + group_id（var_u32）。
#[derive(Debug, Clone, MinecraftPacket)]
pub struct CreativeContent {
    /// 创造模式物品分组列表。
    ///
    /// Creative item groups.
    pub groups: Vec<CreativeItemGroup>,
    /// 创造模式物品列表。
    ///
    /// Creative items.
    pub items: Vec<CreativeItem>,
}

impl CreativeContent {
    /// 构造空的 CreativeContent 包（非创造或旁观者模式使用）。
    ///
    /// Builds an empty CreativeContent packet (used in non-creative or
    /// spectator mode).
    pub fn empty() -> Self {
        Self {
            groups: Vec::new(),
            items: Vec::new(),
        }
    }
}

impl Default for CreativeContent {
    /// 默认实现等价于 [`CreativeContent::empty`]。
    ///
    /// The default implementation is equivalent to [`CreativeContent::empty`].
    fn default() -> Self {
        Self::empty()
    }
}

/// 创造模式物品分组：类别、名称与图标槽位数据。
///
/// Creative item group: category, name and icon slot data.
#[derive(Debug, Clone)]
pub struct CreativeItemGroup {
    /// 分组类别 id。
    ///
    /// Category id of the group.
    pub category: i32,
    /// 分组显示名称。
    ///
    /// Display name of the group.
    pub name: String,
    /// 图标物品的序列化槽位数据。
    ///
    /// Serialized slot data of the icon item.
    pub icon_data: Vec<u8>, // Serialized slot data
}

/// 创造模式物品：创造网络 id、槽位数据与所属分组 id。
///
/// Creative item: creative network id, slot data and owning group id.
#[derive(Debug, Clone)]
pub struct CreativeItem {
    /// 物品在创造模式列表中的网络 id。
    ///
    /// Network id of the item within the creative list.
    pub creative_net_id: u32,
    /// 物品的序列化槽位数据。
    ///
    /// Serialized slot data of the item.
    pub slot_data: Vec<u8>, // Serialized slot data
    /// 所属分组 id。
    ///
    /// Id of the owning group.
    pub group_id: u32,
}

impl Writer for CreativeContent {
    /// 写入 CreativeContent 包体（仅发送方向使用）。
    ///
    /// Writes the CreativeContent packet body (send direction only).
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
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
    /// 不支持解码：服务端只发送 CreativeContent。
    ///
    /// Decoding is unsupported: the server only sends CreativeContent.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "CreativeContent decode is unsupported",
        ))
    }
}
