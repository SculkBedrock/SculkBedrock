use std::io::{Error, ErrorKind};
use std::sync::atomic::{AtomicU16, Ordering};

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

/// Empty Bedrock items always encode as runtime 0 / block 0 /
/// usingNetId=false (`ItemData.AIR`), never from the version-pack palette:
/// empty slots send the `ItemData.AIR` constant directly, while
/// usingNetId=true applies only to non-empty items (replaced at the call
/// site). A palette air id such as -158 is an internal id, not a wire
/// empty value.
static SHIELD_ITEM_RUNTIME_ID: AtomicU16 = AtomicU16::new(u16::MAX);

/// Configures the shield item runtime id from the active version pack.
///
/// Clients derive `ShieldItemID` from the ItemRegistry packet and switch the
/// instance-item extra data to `ItemExtraDataWithBlockingTick` (an extra
/// little-endian i64) for that item, so the encoder must append the blocking
/// tick field whenever the serialized item is a shield.
pub fn configure_shield_item_runtime(runtime_id: u16) {
    SHIELD_ITEM_RUNTIME_ID.store(runtime_id, Ordering::Relaxed);
}

fn configured_shield_item_runtime() -> u16 {
    SHIELD_ITEM_RUNTIME_ID.load(Ordering::Relaxed)
}

pub mod TransactionType {
    pub const NORMAL: u32 = 0;
    pub const MISMATCH: u32 = 1;
    pub const ITEM_USE: u32 = 2;
    pub const ITEM_USE_ON_ACTOR: u32 = 3;
    pub const ITEM_RELEASE: u32 = 4;
}

pub mod ItemUseAction {
    pub const CLICK_BLOCK: i32 = 0;
    pub const CLICK_AIR: i32 = 1;
    pub const BREAK: i32 = 2;
}

/// Hand used for an item interaction (protocol 2225 and later).
pub mod TransactionHand {
    pub const MAINHAND: u8 = 0;
    pub const OFFHAND: u8 = 1;
}

pub mod ItemUseOnActorAction {
    pub const INTERACT: i32 = 0;
    pub const ATTACK: i32 = 1;
}

pub mod ItemReleaseAction {
    pub const RELEASE: i32 = 0;
    pub const CONSUME: i32 = 1;
}

#[derive(Clone, Debug, Default)]
pub struct ItemData {
    pub runtime_id: u16,
    pub count: u16,
    pub damage: u32,
    pub has_net_id: bool,
    pub net_id: i32,
    pub block_runtime_id: u32,
    /// Serialized item user-data payload.  `None` means the canonical empty
    /// user-data section (empty NBT marker + empty CanPlaceOn/CanDestroy
    /// arrays), not an omitted protocol field.
    pub user_data: Option<Vec<u8>>,
}

impl ItemData {
    pub fn is_empty(&self) -> bool {
        self.runtime_id == 0 || self.count == 0
    }
}

#[derive(Clone, Debug)]
pub struct InventoryActionData {
    pub source_type: u32,
    pub container_id: i32,
    pub flag: u32,
    pub slot: u32,
    pub from_item: ItemData,
    pub to_item: ItemData,
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct InventoryTransaction {
    pub legacy_request_id: i32,
    pub transaction_type: u32,
    /// Kept for existing game handlers. Decoders require this section.
    pub has_value: bool,
    pub actions: Vec<InventoryActionData>,

    // TYPE_USE_ITEM
    pub action_type: i32,
    pub trigger_type: u8,
    pub block_x: i32,
    pub block_y: i32,
    pub block_z: i32,
    pub face: u8,
    pub slot: i32,
    /// Used hand (mainhand/offhand). Present on the wire for protocol 2225
    /// and later, zero otherwise.
    pub hand: u8,
    pub item: ItemData,
    pub from_x: f32,
    pub from_y: f32,
    pub from_z: f32,
    pub click_x: f32,
    pub click_y: f32,
    pub click_z: f32,
    pub target_block_id: u32,
    pub prediction: u8,
    pub cooldown: u8,

    // TYPE_USE_ITEM_ON_ENTITY
    pub entity_runtime_id: u64,
    pub entity_action_type: i32,
    /// Used hand (mainhand/offhand). Present on the wire for protocol 2225
    /// and later, zero otherwise.
    pub entity_hand: u8,
    pub entity_player_x: f32,
    pub entity_player_y: f32,
    pub entity_player_z: f32,
    pub entity_click_x: f32,
    pub entity_click_y: f32,
    pub entity_click_z: f32,

    // TYPE_RELEASE_ITEM
    pub release_action_type: i32,
    pub head_x: f32,
    pub head_y: f32,
    pub head_z: f32,
    /// Used hand (mainhand/offhand). Present on the wire for protocol 2225
    /// and later, zero otherwise.
    pub release_hand: u8,
}

fn read_optional_i8(buf: &mut ByteReader) -> Result<i32, Error> {
    if !buf.read_bool()? {
        return Ok(0);
    }
    if !buf.read_bool()? {
        return Ok(0);
    }
    Ok(buf.read_i8()? as i32)
}

fn read_optional_var_u32(buf: &mut ByteReader) -> Result<u32, Error> {
    if !buf.read_bool()? {
        return Ok(0);
    }
    if !buf.read_bool()? {
        return Ok(0);
    }
    buf.read_var_u32()
}

fn read_source(buf: &mut ByteReader) -> Result<(u32, i32, u32), Error> {
    let source_type = buf.read_var_u32()?;
    let container_id = read_optional_i8(buf)?;
    let flag = read_optional_var_u32(buf)?;
    Ok((source_type, container_id, flag))
}

/// Reads Bedrock's NetworkItemStackDescriptor.
pub(crate) fn read_item_data(buf: &mut ByteReader) -> Result<ItemData, Error> {
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
            "item user data is too large",
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

fn read_action_count(buf: &mut ByteReader) -> Result<usize, Error> {
    let count = buf.read_var_u32()? as usize;
    if count > 4096 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "too many inventory actions",
        ));
    }
    Ok(count)
}

impl Reader<InventoryTransaction> for InventoryTransaction {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let legacy_request_id = buf.read_var_i32()?;
        if buf.read_bool()? && legacy_request_id < -1 && (legacy_request_id & 1) == 0 {
            let count = buf.read_var_u32()? as usize;
            if count > 4096 {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "too many legacy inventory actions",
                ));
            }
            for _ in 0..count {
                let _container_id = buf.read_u8()?;
                let payload = buf.read_sized_slice()?;
                if payload.len() > 1 << 20 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "legacy action is too large",
                    ));
                }
            }
        }

        if !buf.read_bool()? {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "missing inventory transaction type",
            ));
        }
        let transaction_type = buf.read_var_u32()?;
        if !buf.read_bool()? {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "missing inventory action data",
            ));
        }

        let action_count = read_action_count(buf)?;
        let mut packet = Self {
            legacy_request_id,
            transaction_type,
            has_value: true,
            actions: Vec::with_capacity(action_count),
            action_type: 0,
            trigger_type: 0,
            block_x: 0,
            block_y: 0,
            block_z: 0,
            face: 0,
            slot: 0,
            hand: 0,
            item: ItemData::default(),
            from_x: 0.0,
            from_y: 0.0,
            from_z: 0.0,
            click_x: 0.0,
            click_y: 0.0,
            click_z: 0.0,
            target_block_id: 0,
            prediction: 0,
            cooldown: 0,
            entity_runtime_id: 0,
            entity_action_type: 0,
            entity_hand: 0,
            entity_player_x: 0.0,
            entity_player_y: 0.0,
            entity_player_z: 0.0,
            entity_click_x: 0.0,
            entity_click_y: 0.0,
            entity_click_z: 0.0,
            release_action_type: 0,
            head_x: 0.0,
            head_y: 0.0,
            head_z: 0.0,
            release_hand: 0,
        };

        for _ in 0..action_count {
            let (source_type, container_id, flag) = read_source(buf)?;
            let slot = buf.read_var_u32()?;
            let from_item = read_item_data(buf)?;
            let to_item = read_item_data(buf)?;
            packet.actions.push(InventoryActionData {
                source_type,
                container_id,
                flag,
                slot,
                from_item,
                to_item,
            });
        }

        match transaction_type {
            TransactionType::NORMAL | TransactionType::MISMATCH => {}
            TransactionType::ITEM_USE => {
                packet.action_type = buf.read_var_i32()?;
                packet.trigger_type = buf.read_u8()?;
                packet.block_x = buf.read_var_i32()?;
                packet.block_y = buf.read_var_i32()?;
                packet.block_z = buf.read_var_i32()?;
                packet.face = buf.read_u8()?;
                packet.slot = buf.read_var_i32()?;
                if crate::protocol::version::protocol_at_least(
                    crate::protocol::version::PROTOCOL_VERSION_1_26_50,
                ) {
                    packet.hand = buf.read_u8()?;
                }
                packet.item = read_item_data(buf)?;
                packet.from_x = buf.read_f32_le()?;
                packet.from_y = buf.read_f32_le()?;
                packet.from_z = buf.read_f32_le()?;
                packet.click_x = buf.read_f32_le()?;
                packet.click_y = buf.read_f32_le()?;
                packet.click_z = buf.read_f32_le()?;
                packet.target_block_id = buf.read_var_u32()?;
                packet.prediction = buf.read_u8()?;
                packet.cooldown = buf.read_u8()?;
            }
            TransactionType::ITEM_USE_ON_ACTOR => {
                packet.entity_runtime_id = buf.read_var_u64()?;
                packet.entity_action_type = buf.read_var_i32()?;
                packet.slot = buf.read_var_i32()?;
                if crate::protocol::version::protocol_at_least(
                    crate::protocol::version::PROTOCOL_VERSION_1_26_60,
                ) {
                    packet.entity_hand = buf.read_u8()?;
                }
                packet.item = read_item_data(buf)?;
                packet.entity_player_x = buf.read_f32_le()?;
                packet.entity_player_y = buf.read_f32_le()?;
                packet.entity_player_z = buf.read_f32_le()?;
                packet.entity_click_x = buf.read_f32_le()?;
                packet.entity_click_y = buf.read_f32_le()?;
                packet.entity_click_z = buf.read_f32_le()?;
            }
            TransactionType::ITEM_RELEASE => {
                packet.release_action_type = buf.read_var_i32()?;
                packet.slot = buf.read_var_i32()?;
                packet.item = read_item_data(buf)?;
                packet.head_x = buf.read_f32_le()?;
                packet.head_y = buf.read_f32_le()?;
                packet.head_z = buf.read_f32_le()?;
                if crate::protocol::version::protocol_at_least(
                    crate::protocol::version::PROTOCOL_VERSION_1_26_60,
                ) {
                    packet.release_hand = buf.read_u8()?;
                }
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "unknown inventory transaction type",
                ));
            }
        }
        Ok(packet)
    }
}

impl Writer for InventoryTransaction {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.legacy_request_id)?;
        buf.write_bool(false)?;
        buf.write_bool(true)?;
        buf.write_var_u32(self.transaction_type)?;
        // The action-data optional must be present for decoders.
        buf.write_bool(true)?;
        buf.write_var_u32(self.actions.len() as u32)?;
        for action in &self.actions {
            buf.write_var_u32(action.source_type)?;
            if action.container_id == 0 {
                buf.write_bool(false)?;
            } else {
                buf.write_bool(true)?;
                buf.write_bool(true)?;
                buf.write_i8(action.container_id as i8)?;
            }
            if action.flag == 0 {
                buf.write_bool(false)?;
            } else {
                buf.write_bool(true)?;
                buf.write_bool(true)?;
                buf.write_var_u32(action.flag)?;
            }
            buf.write_var_u32(action.slot)?;
            write_item_data(buf, &action.from_item)?;
            write_item_data(buf, &action.to_item)?;
        }

        match self.transaction_type {
            TransactionType::NORMAL | TransactionType::MISMATCH => {}
            TransactionType::ITEM_USE => {
                buf.write_var_i32(self.action_type)?;
                buf.write_u8(self.trigger_type)?;
                buf.write_var_i32(self.block_x)?;
                buf.write_var_i32(self.block_y)?;
                buf.write_var_i32(self.block_z)?;
                buf.write_u8(self.face)?;
                buf.write_var_i32(self.slot)?;
                if crate::protocol::version::protocol_at_least(
                    crate::protocol::version::PROTOCOL_VERSION_1_26_50,
                ) {
                    buf.write_u8(self.hand)?;
                }
                write_item_data(buf, &self.item)?;
                buf.write_f32_le(self.from_x)?;
                buf.write_f32_le(self.from_y)?;
                buf.write_f32_le(self.from_z)?;
                buf.write_f32_le(self.click_x)?;
                buf.write_f32_le(self.click_y)?;
                buf.write_f32_le(self.click_z)?;
                buf.write_var_u32(self.target_block_id)?;
                buf.write_u8(self.prediction)?;
                buf.write_u8(self.cooldown)?;
            }
            TransactionType::ITEM_USE_ON_ACTOR => {
                buf.write_var_u64(self.entity_runtime_id)?;
                buf.write_var_i32(self.entity_action_type)?;
                buf.write_var_i32(self.slot)?;
                if crate::protocol::version::protocol_at_least(
                    crate::protocol::version::PROTOCOL_VERSION_1_26_60,
                ) {
                    buf.write_u8(self.entity_hand)?;
                }
                write_item_data(buf, &self.item)?;
                buf.write_f32_le(self.entity_player_x)?;
                buf.write_f32_le(self.entity_player_y)?;
                buf.write_f32_le(self.entity_player_z)?;
                buf.write_f32_le(self.entity_click_x)?;
                buf.write_f32_le(self.entity_click_y)?;
                buf.write_f32_le(self.entity_click_z)?;
            }
            TransactionType::ITEM_RELEASE => {
                buf.write_var_i32(self.release_action_type)?;
                buf.write_var_i32(self.slot)?;
                write_item_data(buf, &self.item)?;
                buf.write_f32_le(self.head_x)?;
                buf.write_f32_le(self.head_y)?;
                buf.write_f32_le(self.head_z)?;
                if crate::protocol::version::protocol_at_least(
                    crate::protocol::version::PROTOCOL_VERSION_1_26_60,
                ) {
                    buf.write_u8(self.release_hand)?;
                }
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "unknown inventory transaction type",
                ))
            }
        }
        Ok(())
    }
}

/// Writes the ordinary NetworkItemStackDescriptor used by inventory packets,
/// transactions and entity equipment.
pub(crate) fn write_item_data(buf: &mut ByteWriter, item: &ItemData) -> Result<(), Error> {
    write_network_item_stack_descriptor(buf, item)
}

/// Writes a Bedrock network item stack descriptor.
///
/// Empty slots send the `ItemData.AIR` constant directly (runtime 0 /
/// count 0 / damage 0 / usingNetId=false / block 0 / default userData).
/// usingNetId=true applies only to non-empty items. Block runtime ids are
/// hashed block-state ids and therefore must not be remapped through the
/// legacy block palette (non-empty items pass through; empty slots write 0).
pub(crate) fn write_network_item_stack_descriptor(
    buf: &mut ByteWriter,
    item: &ItemData,
) -> Result<(), Error> {
    let is_empty = item.is_empty();
    // Empty slots are the `ItemData.AIR` literal (18 bytes); never fill
    // palette air runtime or net id / block hash.
    let runtime_id = if is_empty { 0 } else { item.runtime_id };
    let count = if is_empty { 0 } else { item.count };
    let damage = if is_empty { 0 } else { item.damage };
    let block_runtime_id = if is_empty { 0 } else { item.block_runtime_id };

    buf.write_u16_le(runtime_id)?;
    buf.write_u16_le(count)?;
    buf.write_var_u32(damage)?;
    if is_empty {
        buf.write_bool(false)?;
    } else {
        // Non-empty items always set usingNetId; echo the tracked id
        // (0 when the server has no net id allocator yet).
        buf.write_bool(true)?;
        buf.write_var_i32(item.net_id)?;
    }
    buf.write_var_u32(block_runtime_id)?;
    write_item_user_data(buf, item.user_data.as_deref())?;
    Ok(())
}

/// Writes the user-data field used by item descriptors.
///
/// The default is generated from its semantic fields rather than copied from
/// a packet dump: `ByteWriter::write_slice` emits the required length prefix,
/// then writes the little-endian empty NBT marker followed by zero-length
/// CanPlaceOn and CanDestroy arrays.
fn write_item_user_data(buf: &mut ByteWriter, payload: Option<&[u8]>) -> Result<(), Error> {
    if let Some(payload) = payload {
        return buf.write_slice(payload);
    }

    let mut defaults = ByteWriter::new();
    defaults.write_i16_le(0)?;
    defaults.write_i32_le(0)?;
    defaults.write_i32_le(0)?;
    buf.write_slice(defaults.as_slice())
}

/// Writes the instance-item descriptor used by CreativeContent.
pub(crate) fn write_instance_item(buf: &mut ByteWriter, item: &ItemData) -> Result<(), Error> {
    let is_empty = item.is_empty();
    // Empty items always encode 0/0 (`ItemData.AIR`).
    let runtime_id = if is_empty { 0 } else { item.runtime_id };
    buf.write_var_i32(runtime_id as i16 as i32)?;
    buf.write_u16_le(if is_empty { 0 } else { item.count })?;
    buf.write_var_u32(if is_empty { 0 } else { item.damage })?;
    // blockRuntimeId is zigzag varint here; only the
    // NetworkItemStackDescriptor family uses uvarint.
    buf.write_var_i32(if is_empty {
        0
    } else {
        item.block_runtime_id as i32
    })?;
    // ItemExtraData: default when NBT is absent.
    let mut extra: Vec<u8> = match item.user_data.as_deref() {
        Some(payload) => payload.to_vec(),
        None => {
            let mut defaults = ByteWriter::new();
            defaults.write_i16_le(0)?;
            defaults.write_i32_le(0)?;
            defaults.write_i32_le(0)?;
            defaults.into()
        }
    };
    // Shields carry an extra 8-byte blocking tick inside the encapsulated
    // length prefix.
    if !is_empty && runtime_id == configured_shield_item_runtime() {
        extra.extend_from_slice(&0i64.to_le_bytes());
    }
    buf.write_slice(&extra)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};

    /// has_net_id=true layout: `bool(1) + varint id` (no variant prefix).
    #[test]
    fn net_id_descriptor_round_trip() {
        let item = ItemData {
            runtime_id: 5,
            count: 3,
            damage: 0,
            has_net_id: true,
            net_id: 7,
            block_runtime_id: 9,
            user_data: None,
        };
        let mut writer = ByteWriter::new();
        write_item_data(&mut writer, &item).unwrap();
        let bytes = writer.as_slice();
        // u16le(5) u16le(3) varU(0) bool(1) varI(7→14) varU(9) ...
        assert_eq!(&bytes[..8], &[5, 0, 3, 0, 0, 1, 14, 9]);
        let mut reader = ByteReader::from(bytes);
        let decoded = read_item_data(&mut reader).unwrap();
        assert!(decoded.has_net_id);
        assert_eq!(decoded.net_id, 7);
        assert_eq!(decoded.runtime_id, 5);
        assert_eq!(decoded.count, 3);
    }

    #[test]
    /// Empty slots are the `ItemData.AIR` literal: runtime 0 / count 0 /
    /// damage 0 / usingNetId=false / block 0 / default userData, 18 bytes
    /// total, with no netIdVariant prefix.
    #[test]
    fn empty_descriptor_matches_pnx_item_data_air() {
        let item = ItemData::default();
        let mut writer = ByteWriter::new();
        write_item_data(&mut writer, &item).unwrap();
        let bytes = writer.as_slice();
        // u16le(0) u16le(0) varU(0) bool(0) varU(0) varU(10)+10B default userData.
        assert_eq!(bytes.len(), 18);
        assert_eq!(&bytes[..8], &[0, 0, 0, 0, 0, 0, 0, 10]);
        let mut reader = ByteReader::from(bytes);
        let decoded = read_item_data(&mut reader).unwrap();
        assert!(decoded.is_empty());
        assert!(!decoded.has_net_id);
        assert_eq!(decoded.net_id, 0);
        assert_eq!(decoded.runtime_id, 0);
        assert_eq!(decoded.block_runtime_id, 0);
    }

    fn minimal_use_transaction() -> InventoryTransaction {
        InventoryTransaction {
            legacy_request_id: 0,
            transaction_type: TransactionType::ITEM_USE,
            has_value: true,
            actions: Vec::new(),
            action_type: ItemUseAction::CLICK_BLOCK,
            trigger_type: 0,
            block_x: 1,
            block_y: 2,
            block_z: 3,
            face: 1,
            slot: 0,
            hand: TransactionHand::OFFHAND,
            item: ItemData::default(),
            from_x: 0.0,
            from_y: 0.0,
            from_z: 0.0,
            click_x: 0.0,
            click_y: 0.0,
            click_z: 0.0,
            target_block_id: 0,
            prediction: 0,
            cooldown: 0,
            entity_runtime_id: 0,
            entity_action_type: 0,
            entity_hand: 0,
            entity_player_x: 0.0,
            entity_player_y: 0.0,
            entity_player_z: 0.0,
            entity_click_x: 0.0,
            entity_click_y: 0.0,
            entity_click_z: 0.0,
            release_action_type: 0,
            head_x: 0.0,
            head_y: 0.0,
            head_z: 0.0,
            release_hand: 0,
        }
    }

    /// Hand byte is present for protocol 2193 and later (ITEM_USE) or 2225
    /// and later (other variants), between slot and item. Older encodings
    /// round-trip with hand zeroed.
    #[test]
    fn use_hand_round_trips_only_on_newer_protocols() {
        use crate::protocol::version::{
            with_protocol_version, PROTOCOL_VERSION_1_26_40, PROTOCOL_VERSION_1_26_50,
        };
        let old_bytes = with_protocol_version(PROTOCOL_VERSION_1_26_40, || {
            let mut writer = ByteWriter::new();
            minimal_use_transaction().write(&mut writer).unwrap();
            writer.as_slice().to_vec()
        });
        let new_bytes = with_protocol_version(PROTOCOL_VERSION_1_26_50, || {
            let mut writer = ByteWriter::new();
            minimal_use_transaction().write(&mut writer).unwrap();
            writer.as_slice().to_vec()
        });
        assert_eq!(new_bytes.len(), old_bytes.len() + 1);
        let old_decoded = with_protocol_version(PROTOCOL_VERSION_1_26_40, || {
            let mut reader = ByteReader::from(old_bytes.as_slice());
            InventoryTransaction::read(&mut reader).unwrap()
        });
        assert_eq!(old_decoded.hand, 0);
        assert_eq!(old_decoded.block_x, 1);
        let new_decoded = with_protocol_version(PROTOCOL_VERSION_1_26_50, || {
            let mut reader = ByteReader::from(new_bytes.as_slice());
            InventoryTransaction::read(&mut reader).unwrap()
        });
        assert_eq!(new_decoded.hand, TransactionHand::OFFHAND);
        assert_eq!(new_decoded.block_x, 1);
    }
}
