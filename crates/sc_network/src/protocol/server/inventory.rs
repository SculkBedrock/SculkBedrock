//! Server-side inventory packets.
//!
//! The packet structures are deliberately kept separate from the game
//! inventory model.  The network layer owns Bedrock's container metadata and
//! item descriptor variants; the game layer only supplies semantic stacks.

use std::io::{Error, ErrorKind};

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

use crate::protocol::client::transaction::{read_item_data, write_item_data, ItemData};

/// ContainerOpen (0x2e), server -> client.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ContainerOpen {
    pub window_id: u8,
    pub container_type: i8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub entity_id: i64,
}

impl ContainerOpen {
    pub const PLAYER_INVENTORY_TYPE: i8 = -1;

    pub fn player_inventory(x: i32, y: i32, z: i32, entity_id: u64) -> Self {
        Self {
            window_id: InventoryContent::SPECIAL_INVENTORY,
            container_type: Self::PLAYER_INVENTORY_TYPE,
            x,
            y,
            z,
            entity_id: entity_id as i64,
        }
    }
}

impl Writer for ContainerOpen {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.window_id)?;
        buf.write_u8(self.container_type as u8)?;
        // Bedrock `BlockPosition`: x/y/z are all zigzag varint32.
        buf.write_var_i32(self.x)?;
        buf.write_var_i32(self.y)?;
        buf.write_var_i32(self.z)?;
        buf.write_var_i64(self.entity_id)
    }
}

impl Reader<ContainerOpen> for ContainerOpen {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "ContainerOpen decode is unsupported",
        ))
    }
}

/// Bedrock container ids used by player inventory synchronization.
impl InventoryContent {
    pub const SPECIAL_INVENTORY: u8 = 0;
    pub const SPECIAL_OFFHAND: u8 = 0x77;
    pub const SPECIAL_ARMOR: u8 = 0x78;
    /// Cursor/container-update slot used by the initial inventory sync.
    pub const SPECIAL_CURSOR: u8 = 0x7c;

    pub const FULL_CONTAINER_INVENTORY: u32 = 28;
    pub const FULL_CONTAINER_ARMOR: u32 = 6;
    pub const FULL_CONTAINER_OFFHAND: u32 = 34;
    pub const FULL_CONTAINER_CURSOR: u32 = 59;
    /// Crafting input grid (container 13, matching the `ItemStackResponse`
    /// grid container). Workstation window contents must use it.
    pub const FULL_CONTAINER_CRAFTING_INPUT: u32 = 13;
}

/// InventoryContent (0x31), protocol v1001.
///
/// Slots use the full NetworkItemStackDescriptor.  `fullContainerName` uses
/// the standard container enum, while `storageItem` uses the ordinary
/// item layout without a net-id field.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct InventoryContent {
    pub container_id: u8,
    pub items: Vec<ItemData>,
    pub full_container_name_id: u32,
    /// FullContainerName dynamic id (optional; workstation window contents
    /// carry the window id, otherwise absent).
    pub full_container_dynamic_id: Option<u32>,
}

impl InventoryContent {
    pub fn new(container_id: u8, items: Vec<ItemData>) -> Self {
        let full_container_name_id = match container_id {
            Self::SPECIAL_ARMOR => 6,
            Self::SPECIAL_OFFHAND => 34,
            // Inventory contents use HOTBAR(28) for window 0, not 29.
            _ => 28,
        };
        Self {
            container_id,
            items,
            full_container_name_id,
            full_container_dynamic_id: None,
        }
    }

    pub fn with_full_container_name(mut self, full_container_name_id: u32) -> Self {
        self.full_container_name_id = full_container_name_id;
        self
    }

    pub fn with_full_container_dynamic_id(mut self, dynamic_id: Option<u32>) -> Self {
        self.full_container_dynamic_id = dynamic_id;
        self
    }
}

impl Writer for InventoryContent {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.container_id as u32)?;
        buf.write_var_u32(self.items.len() as u32)?;
        for item in &self.items {
            write_item_data(buf, item)?;
        }
        buf.write_var_u32(self.full_container_name_id)?;
        match self.full_container_dynamic_id {
            Some(dynamic_id) => {
                buf.write_bool(true)?;
                buf.write_u32_le(dynamic_id)?;
            }
            None => buf.write_bool(false)?,
        }
        write_storage_item(buf)
    }
}

impl Reader<InventoryContent> for InventoryContent {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let container_id = buf.read_var_u32()? as u8;
        let count = bounded_count(buf.read_var_u32()?)?;
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            items.push(read_item_data(buf)?);
        }

        let full_container_name_id = buf.read_var_u32()?;
        // Optional(dynamicId): LE u32, not varint.
        let full_container_dynamic_id = if buf.read_bool()? {
            Some(buf.read_u32_le()?)
        } else {
            None
        };
        let _storage = read_storage_item(buf)?;

        Ok(Self {
            container_id,
            items,
            full_container_name_id,
            full_container_dynamic_id,
        })
    }
}

/// An empty storage item is an ordinary item:
/// runtime=0, count=0, damage=0, hasNetId=false, block=0, userData=default.
fn write_storage_item(buf: &mut ByteWriter) -> Result<(), Error> {
    buf.write_u16_le(0)?;
    buf.write_u16_le(0)?;
    buf.write_var_u32(0)?;
    buf.write_bool(false)?;
    buf.write_var_u32(0)?;
    write_default_item_user_data(buf)
}

fn write_default_item_user_data(buf: &mut ByteWriter) -> Result<(), Error> {
    let mut user_data = ByteWriter::new();
    user_data.write_i16_le(0)?;
    user_data.write_i32_le(0)?;
    user_data.write_i32_le(0)?;
    buf.write_slice(user_data.as_slice())
}

fn read_storage_item(buf: &mut ByteReader) -> Result<ItemData, Error> {
    let runtime_id = buf.read_u16_le()?;
    let count = buf.read_u16_le()?;
    let damage = buf.read_var_u32()?;
    let has_net_id = buf.read_bool()?;
    let net_id = if has_net_id { buf.read_var_i32()? } else { 0 };
    let block_runtime_id = buf.read_var_u32()?;
    let user_data_len = buf.read_var_u32()? as usize;
    if user_data_len > 1 << 20 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "storage item user data is too large",
        ));
    }
    let user_data = if user_data_len == 0 {
        None
    } else {
        Some(buf.read_bytes(user_data_len)?.to_vec())
    };
    Ok(ItemData {
        runtime_id,
        count,
        damage,
        has_net_id,
        net_id,
        block_runtime_id,
        user_data,
    })
}

/// InventorySlot (0x32), v975 layout.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct InventorySlot {
    pub full_container_name_id: Option<u32>,
    pub full_container_dynamic_id: Option<u32>,
    pub container_id: u8,
    pub slot: u8,
    pub item: ItemData,
}

impl Writer for InventorySlot {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.container_id as u32)?;
        buf.write_var_u32(self.slot as u32)?;
        // v2168 always carries the full container name for player inventory
        // slots.  Omitting it shifts the item descriptor by two bytes and
        // makes the client decode the following fields as an invalid item.
        buf.write_bool(true)?;
        buf.write_var_u32(
            self.full_container_name_id
                .unwrap_or_else(|| full_container_name_id(self.container_id)),
        )?;
        match self.full_container_dynamic_id {
            Some(dynamic_id) => {
                buf.write_bool(true)?;
                buf.write_u32_le(dynamic_id)?;
            }
            None => buf.write_bool(false)?, // has dynamic id
        }
        buf.write_bool(false)?;
        write_item_data(buf, &self.item)
    }
}

impl Reader<InventorySlot> for InventorySlot {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let container_id = buf.read_var_u32()? as u8;
        let slot = buf.read_var_u32()? as u8;
        let (full_container_name_id, full_container_dynamic_id) = if buf.read_bool()? {
            let id = buf.read_var_u32()?;
            let dynamic_id = if buf.read_bool()? {
                Some(buf.read_u32_le()?)
            } else {
                None
            };
            (Some(id), dynamic_id)
        } else {
            (None, None)
        };
        if buf.read_bool()? {
            let _storage = read_storage_item(buf)?;
        }
        Ok(Self {
            full_container_name_id,
            full_container_dynamic_id,
            container_id,
            slot,
            item: read_item_data(buf)?,
        })
    }
}

fn full_container_name_id(container_id: u8) -> u32 {
    match container_id {
        InventoryContent::SPECIAL_ARMOR => InventoryContent::FULL_CONTAINER_ARMOR,
        InventoryContent::SPECIAL_OFFHAND => InventoryContent::FULL_CONTAINER_OFFHAND,
        InventoryContent::SPECIAL_CURSOR => InventoryContent::FULL_CONTAINER_CURSOR,
        _ => InventoryContent::FULL_CONTAINER_INVENTORY,
    }
}

/// One entry in PlayerArmorDamage (0x95).
///
/// Slot uses armor positions (helmet = 0, chestplate = 1, leggings = 2,
/// boots = 3). Damage is a little-endian 16-bit value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmorSlotDamage {
    pub slot: u8,
    pub damage: u16,
}

/// Armor slot positions for PlayerArmorDamage entries.
pub mod ArmorSlot {
    pub const HEAD: u8 = 0;
    pub const TORSO: u8 = 1;
    pub const LEGS: u8 = 2;
    pub const FEET: u8 = 3;
}

/// PlayerArmorDamage (0x95), protocol v2168.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct PlayerArmorDamage {
    pub entries: Vec<ArmorSlotDamage>,
}

impl PlayerArmorDamage {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl Writer for PlayerArmorDamage {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.entries.len() as u32)?;
        for entry in &self.entries {
            buf.write_var_i32(entry.slot as i32)?;
            buf.write_u16_le(entry.damage)?;
        }
        Ok(())
    }
}

impl Reader<PlayerArmorDamage> for PlayerArmorDamage {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let count = buf.read_var_u32()? as usize;
        if count > 5 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "too many armor damage pairs",
            ));
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(ArmorSlotDamage {
                slot: buf.read_var_i32()? as u8,
                damage: buf.read_u16_le()?,
            });
        }
        Ok(Self { entries })
    }
}

/// MobEquipment (0x1f), v975 layout.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct MobEquipment {
    pub runtime_entity_id: u64,
    pub item: ItemData,
    pub hotbar_slot: u8,
    pub inventory_slot: u8,
    pub window_id: u8,
}

impl Writer for MobEquipment {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.runtime_entity_id)?;
        write_item_data(buf, &self.item)?;
        buf.write_u8(self.inventory_slot)?;
        buf.write_u8(self.hotbar_slot)?;
        buf.write_u8(self.window_id)
    }
}

impl Reader<MobEquipment> for MobEquipment {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            runtime_entity_id: buf.read_var_u64()?,
            item: read_item_data(buf)?,
            inventory_slot: buf.read_u8()?,
            hotbar_slot: buf.read_u8()?,
            window_id: buf.read_u8()?,
        })
    }
}

fn bounded_count(value: u32) -> Result<usize, Error> {
    let value = value as usize;
    if value > 4096 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "too many inventory items",
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};

    #[test]
    fn empty_content_round_trip() {
        let packet = InventoryContent::new(0, vec![ItemData::default(); 36]);
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let bytes: Vec<u8> = writer.into();
        // 1(container)+1(count)+36x18 (`ItemData.AIR` value)
        // +1(fullContainerName)+1(dynamic optional)+18(empty storage item)
        assert_eq!(bytes.len(), 670);
        let mut reader = ByteReader::from(&bytes[..]);
        let decoded = InventoryContent::read(&mut reader).unwrap();
        assert_eq!(decoded.container_id, 0);
        assert_eq!(decoded.items.len(), 36);
        assert_eq!(decoded.full_container_name_id, 28);
        assert!(decoded.items.iter().all(ItemData::is_empty));
    }

    #[test]
    fn pnx_container_lengths_match() {
        let armor = InventoryContent::new(
            InventoryContent::SPECIAL_ARMOR,
            vec![ItemData::default(); 4],
        );
        let offhand =
            InventoryContent::new(InventoryContent::SPECIAL_OFFHAND, vec![ItemData::default()]);
        let mut writer = ByteWriter::new();
        armor.write(&mut writer).unwrap();
        assert_eq!(writer.as_slice().len(), 94);
        let mut writer = ByteWriter::new();
        offhand.write(&mut writer).unwrap();
        assert_eq!(writer.as_slice().len(), 40);
    }

    #[test]
    fn inventory_slot_matches_pnx_layout() {
        let packet = InventorySlot {
            full_container_name_id: None,
            full_container_dynamic_id: None,
            container_id: InventoryContent::SPECIAL_INVENTORY,
            slot: 0,
            item: ItemData::default(),
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        // Packet body length is 24 bytes; the packet id (0x32) is added by
        // MinecraftPackets when sent on the wire.  The air descriptor
        // (`ItemData.AIR`) ends with the one-byte user-data length followed
        // by ten bytes of defaults.
        assert_eq!(writer.as_slice().len(), 24);
        assert_eq!(&writer.as_slice()[..7], &[0, 0, 1, 28, 0, 0, 0]);
    }

    #[test]
    fn player_armor_damage_matches_official_layout() {
        // Empty list encodes as a single zero count byte.
        let packet = PlayerArmorDamage::empty();
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), &[0]);

        let packet = PlayerArmorDamage {
            entries: vec![
                ArmorSlotDamage {
                    slot: ArmorSlot::HEAD,
                    damage: 12,
                },
                ArmorSlotDamage {
                    slot: ArmorSlot::LEGS,
                    damage: 300,
                },
            ],
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(
            writer.as_slice(),
            &[2, 0, 12, 0, 4, (300u16 & 0xff) as u8, (300u16 >> 8) as u8]
        );
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = PlayerArmorDamage::read(&mut reader).unwrap();
        assert_eq!(decoded.entries.len(), 2);
        assert_eq!(decoded.entries[0].slot, ArmorSlot::HEAD);
        assert_eq!(decoded.entries[0].damage, 12);
        assert_eq!(decoded.entries[1].slot, ArmorSlot::LEGS);
        assert_eq!(decoded.entries[1].damage, 300);
    }

    #[test]
    fn offhand_mob_equipment_matches_pnx_container_tail() {
        let packet = MobEquipment {
            runtime_entity_id: 22,
            item: ItemData::default(),
            inventory_slot: 1,
            hotbar_slot: 1,
            window_id: InventoryContent::SPECIAL_OFFHAND,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let bytes = writer.as_slice();
        assert_eq!(bytes.len(), 22);
        assert_eq!(&bytes[..2], &[0x16, 0x00]);
        assert_eq!(&bytes[bytes.len() - 3..], &[1, 1, 0x77]);
    }

    #[test]
    fn player_inventory_container_open_matches_bedrock_layout() {
        let packet = ContainerOpen::player_inventory(1, 64, -2, 42);
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let bytes = writer.as_slice();

        assert_eq!(bytes[0], 0);
        assert_eq!(bytes[1], 0xff);
        // Bedrock BlockPosition: x/y/z are all zigzag varint32:
        // x=1->2, y=64->128->[0x80,0x01], z=-2->3.
        assert_eq!(&bytes[2..6], &[2, 0x80, 0x01, 3]);
        assert_eq!(&bytes[6..], &[0x54]);
    }

    #[test]
    fn workbench_container_open_matches_pnx_layout() {
        // CraftingTableInventory.onOpen: window 1 + WORKBENCH(1) +
        // BlockPosition + target entity -1. Y uses zigzag like UpdateBlock.
        let packet = ContainerOpen {
            window_id: 1,
            container_type: 1,
            x: 10,
            y: 64,
            z: -3,
            entity_id: -1,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(
            writer.as_slice(),
            &[1, 1, 20, 0x80, 0x01, 5, 0x01],
            "window=1 type=WORKBENCH x=10→20 y=64→[0x80,0x01] z=-3→5 entity=-1→1"
        );
    }
}
