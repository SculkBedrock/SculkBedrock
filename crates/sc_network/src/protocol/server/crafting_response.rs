//! `ItemStackResponse` (0x94).
//!
//! Layout: single-entry array; per entry `u8 result` (SUCCESS=0, ERROR=1),
//! zigzag varint request id, unconditional `bool(true)`, then the containers
//! presence bool followed by the container array when present.
//! This packet always carries a single entry (one `CraftResponse` per
//! request): `slots` grouped by container are `containers`, and
//! `success=false` encodes ERROR (no containers). Container and slot
//! encoding: `FullContainerName` is `u8 containerEnum +
//! Optional(dynamicId)` (absent dynamic id is a single `bool(false)` byte);
//! slot is `u8 requestedSlot + u8 slot + u8 count + varint netId + string
//! customName + varint durabilityCorrection`.
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::{Error, ErrorKind};

/// One server-issued `ItemStackResponse` entry.
///
/// `MinecraftPackets::ItemStackResponse` is always a single entry at the
/// packet-array level, so the struct carries all slot updates for one
/// request directly, with no extra entries layer.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ItemStackResponse {
    pub slots: Vec<sc_game::craft_inventory::CraftSlotUpdate>,
    pub request_id: i32,
    pub success: bool,
}

impl Writer for ItemStackResponse {
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

/// `helper::writeItemStackResponseContainer`:
/// `FullContainerName` + slot array.
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

/// Slot entry layout: `u8 slot + u8 hotbarSlot + u8 count + bool + optional
/// varint netId + string customName + optional filteredName + varint
/// durabilityCorrection`.
///
/// Notes:
/// - `slot` and `hotbarSlot` carry the same network slot value twice;
/// - `writeBoolean(true)` is unconditional; the netId varint follows only
///   when the id is positive, otherwise a second `false`;
/// - `customName` is always one (here empty) string; absent
///   `filteredCustomName` writes only `false`; durabilityCorrection carries
///   the item damage.
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
                },
                CraftSlotUpdate {
                    container: C::Grid,
                    slot: 30,
                    stack: sc_item::ItemStack::empty(),
                    net_id: 0,
                    dynamic: None,
                },
                CraftSlotUpdate {
                    container: C::Cursor,
                    slot: 0,
                    stack: sc_item::ItemStack::empty(),
                    net_id: 0,
                    dynamic: None,
                },
                CraftSlotUpdate {
                    container: C::Output,
                    slot: 50,
                    stack: sc_item::ItemStack::empty(),
                    net_id: 0,
                    dynamic: None,
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
}
