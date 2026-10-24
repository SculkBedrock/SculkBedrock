//! ItemStackResponsePacket (0x94)：合成/容器操作的结果回包。
//!
//! ItemStackResponsePacket (0x94): result reply for crafting/container
//! operations.
//!
//! 布局：单元素数组；每项 `u8 result`（SUCCESS=0、ERROR=1）、zigzag
//! varint 请求 id、无条件 `bool(true)`，随后是容器存在性 bool，存在时
//! 跟容器数组。该包始终只携带单元素（一次请求对应一个
//! `CraftResponse`）：`slots` 按容器分组后成为 `containers`，
//! `success=false` 编码 ERROR（无容器）。容器与槽位编码：
//! `FullContainerName` 为 `u8 containerEnum + Optional(dynamicId)`
//! （缺失 dynamic id 时为单个 `bool(false)` 字节）；槽位为
//! `u8 requestedSlot + u8 slot + u8 count + varint netId + string
//! customName + varint durabilityCorrection`。
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::{Error, ErrorKind};

/// 服务端下发的一条 `ItemStackResponse` 记录。
///
/// One server-issued `ItemStackResponse` entry.
///
/// `MinecraftPackets::ItemStackResponse` 在包数组层面始终只有单元素，
/// 因此该结构体直接承载一次请求的全部槽位更新，没有额外的条目层。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ItemStackResponse {
    /// 本次请求涉及的槽位更新列表。
    ///
    /// Slot updates covered by this request.
    pub slots: Vec<sc_game::craft_inventory::CraftSlotUpdate>,
    /// 对应请求 id（zigzag varint32）。
    ///
    /// Matching request id (zigzag varint32).
    pub request_id: i32,
    /// 是否成功（false 编码为 ERROR）。
    ///
    /// Whether the request succeeded (false encodes ERROR).
    pub success: bool,
}

impl Writer for ItemStackResponse {
    /// 写入 ItemStackResponse 包体（按容器分组槽位）。
    ///
    /// Writes the ItemStackResponse body (slots grouped by container).
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        use sc_game::craft_inventory::CraftContainer;

        // Single-entry array.
        buf.write_var_u32(1)?;
        // Result ordinal: SUCCESS=0 / ERROR=1.
        buf.write_u8(if self.success { 0 } else { 1 })?;
        // Signed zigzag varint request id.
        buf.write_var_i32(self.request_id)?;
        buf.write_bool(true)?;
        if !self.success {
            // ERROR carries no containers.
            buf.write_bool(false)?;
            return Ok(());
        }
        let mut containers =
            std::collections::BTreeMap::<u8, Vec<&sc_game::craft_inventory::CraftSlotUpdate>>::new(
            );
        for slot in &self.slots {
            // Container ids: 28=HOTBAR, 29=INVENTORY, 59=CURSOR,
            // 60=CREATED_OUTPUT.
            let kind = match slot.container {
                CraftContainer::Inventory => 29,
                CraftContainer::Hotbar => 28,
                CraftContainer::CombinedInventory => 12,
                CraftContainer::Grid => 13,
                CraftContainer::Cursor => 59,
                CraftContainer::Output => 60,
                // Furnace slots sync via SyncFurnaceWindow, never through
                // ItemStackResponse updates (see `updates()`).
                CraftContainer::Furnace => continue,
                // Bedrock FullContainerName containerEnum for LEVEL_ENTITY (chests, barrels) is 7.
                CraftContainer::Chest => slot.container_enum.unwrap_or(7),
            };
            containers.entry(kind).or_default().push(slot);
        }
        if containers.is_empty() {
            buf.write_bool(false)?;
            return Ok(());
        }
        buf.write_bool(true)?;
        // helper.writeArray(containers, writeItemStackResponseContainer).
        buf.write_var_u32(containers.len() as u32)?;
        for (kind, slots) in containers {
            write_container(buf, kind, &slots)?;
        }
        Ok(())
    }
}

/// `helper::writeItemStackResponseContainer`：写入一个容器组
/// （`FullContainerName` + 槽位数组）。
///
/// `helper::writeItemStackResponseContainer`: writes one container group
/// (`FullContainerName` + slot array).
///
/// - `buf`: 输出字节写入器
/// - `kind`: 容器枚举值
/// - `slots`: 该容器下的槽位更新
///
/// # Errors
/// 写入缓冲区失败时返回 IO 错误。
fn write_container(
    buf: &mut ByteWriter,
    kind: u8,
    slots: &[&sc_game::craft_inventory::CraftSlotUpdate],
) -> Result<(), Error> {
    // FullContainerName: u8 containerEnum + Optional(dynamicId).
    // Dynamic id is uniform per container; echo the first slot, or
    // bool(false) when absent.
    buf.write_u8(kind)?;
    match slots.first().and_then(|slot| slot.dynamic) {
        Some(dynamic) => {
            buf.write_bool(true)?;
            buf.write_u32_le(dynamic)?;
        }
        None => buf.write_bool(false)?,
    }
    buf.write_var_u32(slots.len() as u32)?;
    for slot in slots {
        write_slot(buf, slot)?;
    }
    Ok(())
}

/// 槽位条目布局：`u8 slot + u8 hotbarSlot + u8 count + bool + 可选
/// varint netId + string customName + 可选 filteredName + varint
/// durabilityCorrection`。
///
/// Slot entry layout: `u8 slot + u8 hotbarSlot + u8 count + bool +
/// optional varint netId + string customName + optional filteredName +
/// varint durabilityCorrection`.
///
/// 注意：
/// - `slot` 与 `hotbarSlot` 携带相同的网络槽位值，写两遍；
/// - `writeBoolean(true)` 无条件写入；netId varint 仅在 id 为正时跟上，
///   否则写第二个 `false`；
/// - `customName` 始终是一个（此处为空）字符串；缺失
///   `filteredCustomName` 时只写 `false`；durabilityCorrection 携带
///   物品耐久。
fn write_slot(
    buf: &mut ByteWriter,
    slot: &sc_game::craft_inventory::CraftSlotUpdate,
) -> Result<(), Error> {
    buf.write_u8(slot.slot)?;
    buf.write_u8(slot.slot)?;
    buf.write_u8(slot.stack.count.min(255) as u8)?;
    buf.write_bool(true)?;
    buf.write_bool(slot.net_id > 0)?;
    if slot.net_id > 0 {
        buf.write_var_i32(slot.net_id)?;
    }
    buf.write_string("")?;
    buf.write_bool(false)?;
    buf.write_var_i32(slot.stack.damage as i32)?;
    Ok(())
}

impl Reader<ItemStackResponse> for ItemStackResponse {
    /// 不支持解码：该包仅由服务端下发。
    ///
    /// Decoding is unsupported: this packet is server-only.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(ErrorKind::Unsupported, "server-only response"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_entries_match_2168_serializer_branches() {
        // Success with zero containers writes true,false (no array length).
        // Failure uses the same shape (ERROR carries no containers).
        for (success, golden) in [(true, vec![1, 0, 1, 1, 0]), (false, vec![1, 1, 1, 1, 0])] {
            let mut writer = ByteWriter::new();
            ItemStackResponse {
                slots: Vec::new(),
                request_id: -1,
                success,
            }
            .write(&mut writer)
            .unwrap();
            assert_eq!(writer.as_slice(), golden);
        }
    }

    #[test]
    fn nonempty_success_matches_2168_container_layout() {
        use sc_game::craft_inventory::{CraftContainer as C, CraftSlotUpdate};
        let mut writer = ByteWriter::new();
        ItemStackResponse {
            request_id: -1,
            success: true,
            slots: vec![
                CraftSlotUpdate {
                    container: C::Inventory,
                    slot: 35,
                    stack: sc_item::ItemStack::new(1, 1),
                    net_id: 7,
                    dynamic: None,
                    container_enum: None,
                },
                CraftSlotUpdate {
                    container: C::Grid,
                    slot: 30,
                    stack: sc_item::ItemStack::empty(),
                    net_id: 0,
                    dynamic: None,
                    container_enum: None,
                },
                CraftSlotUpdate {
                    container: C::Cursor,
                    slot: 0,
                    stack: sc_item::ItemStack::empty(),
                    net_id: 0,
                    dynamic: None,
                    container_enum: None,
                },
                CraftSlotUpdate {
                    container: C::Output,
                    slot: 50,
                    stack: sc_item::ItemStack::empty(),
                    net_id: 0,
                    dynamic: None,
                    container_enum: None,
                },
            ],
        }
        .write(&mut writer)
        .unwrap();
        assert_eq!(
            writer.as_slice(),
            &[
                1, 0, 1, 1, 1, 4, 13, 0, 1, 30, 30, 0, 1, 0, 0, 0, 0, 29, 0, 1, 35, 35, 1, 1, 1,
                14, 0, 0, 0, 59, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 60, 0, 1, 50, 50, 0, 1, 0, 0, 0, 0,
            ]
        );
    }

    #[test]
    fn grid_dynamic_echoes_window_id() {
        // Workstation window: grid container echoes dynamic=1 (window id).
        use sc_game::craft_inventory::{CraftContainer as C, CraftSlotUpdate};
        let mut writer = ByteWriter::new();
        ItemStackResponse {
            request_id: -1,
            success: true,
            slots: vec![CraftSlotUpdate {
                container: C::Grid,
                slot: 32,
                stack: sc_item::ItemStack::new(1, 2),
                net_id: 9,
                dynamic: Some(1),
                container_enum: None,
            }],
        }
        .write(&mut writer)
        .unwrap();
        assert_eq!(
            writer.as_slice(),
            &[
                1, 0, 1, 1, 1, 1, // 1 entry, ok, req -1, true, true, 1 container
                13, 1, 1, 0, 0, 0, 1, // Grid + dynamic(1) + 1 slot
                32, 32, 2, 1, 1, 18, 0, 0,
                0, // slot,hotbar,count,bools,net 9,custom,durability
            ]
        );
    }

    #[test]
    fn chest_container_encodes_level_entity() {
        use sc_game::craft_inventory::{CraftContainer as C, CraftSlotUpdate};
        let mut writer = ByteWriter::new();
        ItemStackResponse {
            request_id: 42,
            success: true,
            slots: vec![CraftSlotUpdate {
                container: C::Chest,
                slot: 5,
                stack: sc_item::ItemStack::new(10, 3),
                net_id: 15,
                dynamic: None,
                container_enum: Some(7),
            }],
        }
        .write(&mut writer)
        .unwrap();
        assert_eq!(
            writer.as_slice(),
            &[
                1, 0, 84, 1, 1, 1, // 1 entry, ok, zigzag 42 (= 84), true, true, 1 container
                7, 0, 1, // container 7 (LEVEL_ENTITY) + dynamic(false) + 1 slot
                5, 5, 3, 1, 1, 30, 0, 0, 0, // slot 5, hotbar 5, count 3, bools, net_id 15 (zigzag 30), custom, durability
            ]
        );
    }
}
