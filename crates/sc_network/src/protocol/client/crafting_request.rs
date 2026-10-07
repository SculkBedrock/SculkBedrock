//! `ItemStackRequest` (0x93 = 147) boundary type (2168 profile).
//!
//! Supported subset (all shapes cross-checked against bedrock-protocol-docs
//! action values, the 1.26.40 legacy-container note and an independent
//! Bedrock implementation for the classic pre-cereal layout):
//! - `Take` (0) / `Place` (1): amount + source slot + destination slot.
//!   Parsed so mixed arrange-then-craft requests stay aligned; the moves
//!   themselves are **not** applied — the server crafts authoritatively
//!   from the inventory multiset and resyncs (client grid prediction is
//!   never trusted).
//! - `Swap` (2): source slot + destination slot (same treatment as moves).
//! - `CraftRecipe` (10) / `CraftRecipeAuto` (11): recipe network id +
//!   requested craft count → craft intents.
//! - Any other action type: the payload shape is not implemented, so the
//!   whole packet is rejected with a bounded error (never guessed, never
//!   panics). Such requests are logged and skipped; no craft executes.
//!
//! Malformed buffers return `Err`, never panic.

use std::io::{Error, ErrorKind};

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;

pub const ITEM_STACK_REQUEST_PACKET_ID: u16 = 147;

pub mod ItemStackRequestActionType {
    pub const TAKE: u8 = 0;
    pub const PLACE: u8 = 1;
    pub const SWAP: u8 = 2;
    pub const DROP: u8 = 3;
    pub const DESTROY: u8 = 4;
    pub const CONSUME: u8 = 5;
    pub const CREATE: u8 = 6;
    // Action TYPE IDs: 7 LAB_COMBINE, 8 BEACON_PAYMENT, 9 MINE_BLOCK.
    pub const LAB_TABLE_COMBINE: u8 = 7;
    pub const BEACON_PAYMENT: u8 = 8;
    pub const MINE_BLOCK: u8 = 9;
    pub const CRAFT_RECIPE: u8 = 10;
    pub const CRAFT_RECIPE_AUTO: u8 = 11;
    pub const CRAFT_CREATIVE: u8 = 12;
    pub const CRAFT_RECIPE_OPTIONAL: u8 = 13;
    /// Index 13 carries CraftReservedAction on protocol 2225 and later.
    pub const CRAFT_RESERVED_ACTION: u8 = 13;
    pub const CRAFT_REPAIR: u8 = 14;
    pub const CRAFT_LOOM: u8 = 15;
    pub const CRAFT_NON_IMPLEMENTED: u8 = 16;
    pub const CRAFT_RESULTS: u8 = 17;
}

/// Container reference inside a slot (opaque to crafting; only consumed
/// for stream alignment — the server never trusts client slot routing).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestSlotRef {
    pub container_enum: u8,
    pub dynamic_slot: Option<u32>,
    pub slot: u8,
    pub net_id: i32,
}

fn read_slot_ref(buf: &mut ByteReader) -> Result<RequestSlotRef, Error> {
    let container_enum = buf.read_u8()?;
    let dynamic_slot = if buf.read_bool()? {
        Some(buf.read_u32_le()?)
    } else {
        None
    };
    let slot = buf.read_u8()?;
    let net_id = buf.read_i32_le()?;
    Ok(RequestSlotRef {
        container_enum,
        dynamic_slot,
        slot,
        net_id,
    })
}

fn write_slot_ref(buf: &mut ByteWriter, slot: &RequestSlotRef) -> Result<(), Error> {
    buf.write_u8(slot.container_enum)?;
    match slot.dynamic_slot {
        Some(dynamic) => {
            buf.write_bool(true)?;
            buf.write_u32_le(dynamic)?;
        }
        None => buf.write_bool(false)?,
    }
    buf.write_u8(slot.slot)?;
    buf.write_i32_le(slot.net_id)?;
    Ok(())
}

/// One parsed request action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemStackRequestAction {
    CraftResults {
        times: u8,
    },
    Consume {
        amount: u8,
        source: RequestSlotRef,
    },
    Create {
        result_slot: u8,
    },
    /// Client-side move (arrange). Parsed for alignment only; the game
    /// crafts from the authoritative multiset instead.
    Move {
        action_type: u8,
        amount: Option<u8>,
        source: RequestSlotRef,
        destination: RequestSlotRef,
    },
    CraftRecipe {
        recipe_network_id: u32,
        times: u8,
    },
    CraftRecipeAuto {
        recipe_network_id: u32,
        times: u8,
    },
    /// Any other action type. The payload shape is not implemented, so the
    /// request cannot be aligned safely and the packet is rejected.
    Unknown {
        action_type: u8,
    },
}

/// One client request: id + bounded action list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemStackRequestEntry {
    pub request_id: i32,
    pub actions: Vec<ItemStackRequestAction>,
}

/// `ItemStackRequest` packet (client → server).
#[derive(Clone, Debug, Eq, PartialEq, MinecraftPacket)]
pub struct ItemStackRequest {
    pub requests: Vec<ItemStackRequestEntry>,
}

impl ItemStackRequest {
    pub const MAX_REQUESTS: usize = 64;
    pub const MAX_ACTIONS: usize = 64;

    /// Craft actions only (recipe network id, times). Used by the crafting
    /// translator; client-side moves are skipped (the server crafts from
    /// the authoritative multiset instead of replaying client slot moves).
    pub fn craft_actions(&self) -> Vec<(i32, u32, u8)> {
        let mut out = Vec::new();
        for request in &self.requests {
            for action in &request.actions {
                match action {
                    ItemStackRequestAction::CraftRecipe {
                        recipe_network_id,
                        times,
                    }
                    | ItemStackRequestAction::CraftRecipeAuto {
                        recipe_network_id,
                        times,
                    } => out.push((request.request_id, *recipe_network_id, *times)),
                    ItemStackRequestAction::Move { .. }
                    | ItemStackRequestAction::Consume { .. }
                    | ItemStackRequestAction::Create { .. }
                    | ItemStackRequestAction::CraftResults { .. }
                    | ItemStackRequestAction::Unknown { .. } => {}
                }
            }
        }
        out
    }

    /// Whether any request carries a craft action.
    pub fn has_craft(&self) -> bool {
        self.requests.iter().any(|request| {
            request.actions.iter().any(|action| {
                matches!(
                    action,
                    ItemStackRequestAction::CraftRecipe { .. }
                        | ItemStackRequestAction::CraftRecipeAuto { .. }
                )
            })
        })
    }
}

fn read_move(
    buf: &mut ByteReader,
    action_type: u8,
    with_amount: bool,
) -> Result<ItemStackRequestAction, Error> {
    let amount = if with_amount {
        Some(buf.read_u8()?)
    } else {
        None
    };
    let source = read_slot_ref(buf)?;
    let destination = read_slot_ref(buf)?;
    Ok(ItemStackRequestAction::Move {
        action_type,
        amount,
        source,
        destination,
    })
}

fn read_action(buf: &mut ByteReader) -> Result<ItemStackRequestAction, Error> {
    let action_type = u8::try_from(buf.read_var_u32()?)
        .map_err(|_| Error::new(ErrorKind::InvalidData, "invalid action type"))?;
    let _legacy_type = buf.read_u8()?;
    match action_type {
        ItemStackRequestActionType::TAKE | ItemStackRequestActionType::PLACE => {
            read_move(buf, action_type, true)
        }
        ItemStackRequestActionType::SWAP => read_move(buf, action_type, false),
        ItemStackRequestActionType::CONSUME => Ok(ItemStackRequestAction::Consume {
            amount: buf.read_u8()?,
            source: read_slot_ref(buf)?,
        }),
        ItemStackRequestActionType::CREATE => Ok(ItemStackRequestAction::Create {
            result_slot: buf.read_u8()?,
        }),
        // Unsupported actions decode their payload, then classify as
        // Unknown (stream stays aligned; the caller answers ERROR so the
        // client rolls its prediction back instead of stalling).
        ItemStackRequestActionType::DROP => {
            buf.read_u8()?; // count
            read_slot_ref(buf)?;
            buf.read_bool()?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::DESTROY => {
            buf.read_u8()?; // count
            read_slot_ref(buf)?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::LAB_TABLE_COMBINE => {
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::BEACON_PAYMENT => {
            buf.read_var_i32()?;
            buf.read_var_i32()?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::MINE_BLOCK => {
            buf.read_var_i32()?;
            buf.read_var_i32()?;
            buf.read_i32_le()?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_CREATIVE => {
            buf.read_var_u32()?;
            buf.read_u8()?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_RECIPE_OPTIONAL => {
            if crate::protocol::version::protocol_at_least(
                crate::protocol::version::PROTOCOL_VERSION_1_26_60,
            ) {
                // CraftReservedAction on protocol 2225 and later.
                buf.read_string()?;
                buf.read_u8()?;
            } else {
                buf.read_var_u32()?;
                buf.read_i32_le()?;
            }
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_REPAIR => {
            buf.read_i32_le()?; // recipe net id (intLE)
            buf.read_u8()?;
            buf.read_var_i32()?; // repair cost
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_LOOM => {
            buf.read_string()?;
            buf.read_u8()?;
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_NON_IMPLEMENTED => {
            Ok(ItemStackRequestAction::Unknown { action_type })
        }
        ItemStackRequestActionType::CRAFT_RESULTS => {
            let count = buf.read_var_u32()?;
            if count > 9 {
                return Err(Error::new(ErrorKind::InvalidData, "too many craft results"));
            }
            for _ in 0..count {
                read_request_ingredient(buf)?;
                buf.read_var_u32()?; // block runtime id
                let bytes = buf.read_var_u32()? as usize;
                if bytes > 65536 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "craft userdata too large",
                    ));
                }
                buf.read_bytes(bytes)?;
            }
            Ok(ItemStackRequestAction::CraftResults {
                times: buf.read_u8()?,
            })
        }
        ItemStackRequestActionType::CRAFT_RECIPE => {
            let recipe_network_id = buf.read_var_u32()?;
            let times = buf.read_u8()?;
            Ok(ItemStackRequestAction::CraftRecipe {
                recipe_network_id,
                times,
            })
        }
        ItemStackRequestActionType::CRAFT_RECIPE_AUTO => {
            let recipe_network_id = buf.read_var_u32()?;
            let times = buf.read_u8()?;
            let count = buf.read_var_u32()?;
            if count > 9 {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "too many auto craft ingredients",
                ));
            }
            for _ in 0..count {
                read_request_ingredient(buf)?;
            }
            Ok(ItemStackRequestAction::CraftRecipeAuto {
                recipe_network_id,
                times,
            })
        }
        other => {
            // Payload shape not implemented: the request cannot be aligned
            // safely, so the whole packet is rejected (caller logs + skips).
            // This is never silent and never panics.
            Err(Error::new(
                ErrorKind::Unsupported,
                format!("unsupported item stack action {other}"),
            ))
        }
    }
}

impl Reader<ItemStackRequest> for ItemStackRequest {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let count = buf.read_var_u32()? as usize;
        if count > Self::MAX_REQUESTS {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "too many item stack requests",
            ));
        }
        let mut requests = Vec::with_capacity(count);
        for _ in 0..count {
            requests.push(ItemStackRequestEntry::read_entry(buf)?);
        }
        Ok(Self { requests })
    }
}

impl ItemStackRequestEntry {
    pub fn read_entry(buf: &mut ByteReader) -> Result<Self, Error> {
        let request_id = buf.read_var_i32()?;
        let action_count = buf.read_var_u32()? as usize;
        if action_count > ItemStackRequest::MAX_ACTIONS {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "too many item stack request actions",
            ));
        }
        let mut actions = Vec::with_capacity(action_count);
        for _ in 0..action_count {
            actions.push(read_action(buf)?);
            // Every known action (including DROP/DESTROY/BEACON/MINE/CREATIVE/
            // OPTIONAL/REPAIR/LOOM normalized to Unknown) consumes its
            // standard-length payload, so alignment holds for following
            // actions. Only an unknown TYPE errors out (packet skipped with
            // a warning, connection kept).
        }
        let filters = buf.read_var_u32()?;
        if filters > 16 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "too many filter strings",
            ));
        }
        for _ in 0..filters {
            let text = buf.read_string()?;
            if text.len() > 1024 {
                return Err(Error::new(ErrorKind::InvalidData, "filter string too long"));
            }
        }
        let _origin = buf.read_i32_le()?;
        Ok(Self {
            request_id,
            actions,
        })
    }
}

fn read_request_ingredient(buf: &mut ByteReader) -> Result<(), Error> {
    let kind = buf.read_var_u32()?;
    let _legacy = buf.read_u8()?;
    match kind {
        0 => {}
        1 => {
            buf.read_string()?;
            buf.read_var_i32()?;
        }
        2 => {
            buf.read_string()?;
            buf.read_i16_le()?;
        }
        3 => {
            buf.read_string()?;
        }
        _ => {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "unsupported auto craft descriptor",
            ))
        }
    }
    buf.read_i16_le()?;
    Ok(())
}

fn write_action_type(buf: &mut ByteWriter, kind: u8) -> Result<(), Error> {
    // Legacy type mapping: TYPE 0..6 keep legacy values, TYPE 7..17 map to
    // legacy 9..19 (legacy 7/8 were retired container moves). Writer is only
    // used for test round-trips.
    buf.write_var_u32(kind as u32)?;
    buf.write_u8(if kind >= 7 { kind + 2 } else { kind })
}

impl Writer for ItemStackRequest {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.requests.len() as u32)?;
        for request in &self.requests {
            buf.write_var_i32(request.request_id)?;
            buf.write_var_u32(request.actions.len() as u32)?;
            for action in &request.actions {
                match action {
                    ItemStackRequestAction::CraftResults { times } => {
                        write_action_type(buf, 17)?;
                        buf.write_var_u32(0)?;
                        buf.write_u8(*times)?;
                    }
                    ItemStackRequestAction::Consume { amount, source } => {
                        write_action_type(buf, ItemStackRequestActionType::CONSUME)?;
                        buf.write_u8(*amount)?;
                        write_slot_ref(buf, source)?;
                    }
                    ItemStackRequestAction::Create { result_slot } => {
                        write_action_type(buf, ItemStackRequestActionType::CREATE)?;
                        buf.write_u8(*result_slot)?;
                    }
                    ItemStackRequestAction::Move {
                        action_type,
                        amount,
                        source,
                        destination,
                    } => {
                        write_action_type(buf, *action_type)?;
                        if *action_type == ItemStackRequestActionType::TAKE
                            || *action_type == ItemStackRequestActionType::PLACE
                        {
                            buf.write_u8(amount.unwrap_or(0))?;
                        }
                        write_slot_ref(buf, source)?;
                        write_slot_ref(buf, destination)?;
                    }
                    ItemStackRequestAction::CraftRecipe {
                        recipe_network_id,
                        times,
                    } => {
                        write_action_type(buf, ItemStackRequestActionType::CRAFT_RECIPE)?;
                        buf.write_var_u32(*recipe_network_id)?;
                        buf.write_u8(*times)?;
                    }
                    ItemStackRequestAction::CraftRecipeAuto {
                        recipe_network_id,
                        times,
                    } => {
                        write_action_type(buf, ItemStackRequestActionType::CRAFT_RECIPE_AUTO)?;
                        buf.write_var_u32(*recipe_network_id)?;
                        buf.write_u8(*times)?;
                        buf.write_var_u32(0)?; // automatic input selection, no descriptor hints
                    }
                    ItemStackRequestAction::Unknown { action_type } => {
                        if *action_type != 16 {
                            return Err(Error::new(
                                ErrorKind::Unsupported,
                                "unknown action cannot be encoded",
                            ));
                        }
                        write_action_type(buf, 16)?;
                    }
                }
            }
            buf.write_var_u32(0)?; // stringsToFilter
            buf.write_i32_le(-1)?; // UNKNOWN text origin
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};
    use sc_binary::{ByteReader, ByteWriter};

    #[test]
    fn vanilla_take_bytes_decode_to_move() {
        // Hand-built wire bytes (not via this Writer): single request -1,
        // Take{1, (29/slot10/net0) -> (59/slot0/net0)}, 25 bytes total.
        // request(-1)=01 actions(1)=01 type(00)+legacy(00) count(01)
        // src(1D 00 0A 00000000) dst(3B 00 00 00000000) filters(00) origin(FFFFFFFF).
        let bytes: [u8; 25] = [
            1, 1, 1, 0, 0, 1, 0x1d, 0, 10, 0, 0, 0, 0, 0x3b, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff,
            0xff,
        ];
        let mut reader = ByteReader::from(&bytes[..]);
        let request = ItemStackRequest::read(&mut reader).unwrap();
        assert!(reader.as_slice().is_empty());
        assert_eq!(request.requests.len(), 1);
        assert_eq!(request.requests[0].request_id, -1);
        assert!(matches!(
            &request.requests[0].actions[..],
            [ItemStackRequestAction::Move {
                action_type: 0,
                amount: Some(1),
                source,
                destination,
            }] if source.container_enum == 29
                && source.slot == 10
                && source.net_id == 0
                && destination.container_enum == 59
                && destination.slot == 0
        ));
        assert!(!request.has_craft());
    }

    #[test]
    fn codec_2168_craft_consume_golden_and_every_truncated_prefix() {
        // One signed request (-1), two actions: new type + legacy type,
        // recipe id=2, count=2, then consume from hotbar (container 28).
        let bytes = [
            1, 1, 2, 10, 12, 2, 2, 5, 5, 2, 28, 0, 3, 0, 0, 0, 0, 0, 255, 255, 255, 255,
        ];
        let mut reader = ByteReader::from(&bytes[..]);
        let request = ItemStackRequest::read(&mut reader).unwrap();
        assert_eq!(request.craft_actions(), vec![(-1, 2, 2)]);
        assert!(
            matches!(&request.requests[0].actions[1], ItemStackRequestAction::Consume { source, amount: 2 } if source.slot == 3 && source.net_id == 0)
        );
        for end in 0..bytes.len() {
            assert!(
                ItemStackRequest::read(&mut ByteReader::from(&bytes[..end])).is_err(),
                "prefix {end}"
            );
        }
        let mut encoded = ByteWriter::new();
        request.write(&mut encoded).unwrap();
        assert_eq!(encoded.as_slice(), bytes);
    }

    #[test]
    fn auto_craft_descriptors_and_filter_tail_keep_multiple_requests_aligned() {
        let mut bytes = ByteWriter::new();
        bytes.write_var_u32(2).unwrap();
        bytes.write_var_i32(-4).unwrap();
        bytes.write_var_u32(1).unwrap();
        write_action_type(&mut bytes, 11).unwrap();
        bytes.write_var_u32(2).unwrap();
        bytes.write_u8(3).unwrap();
        bytes.write_var_u32(1).unwrap();
        bytes.write_var_u32(1).unwrap();
        bytes.write_u8(1).unwrap();
        bytes.write_string("minecraft:oak_log").unwrap();
        bytes.write_var_i32(32767).unwrap();
        bytes.write_i16_le(1).unwrap();
        bytes.write_var_u32(1).unwrap();
        bytes.write_string("filter").unwrap();
        bytes.write_i32_le(-1).unwrap();
        bytes.write_var_i32(-5).unwrap();
        bytes.write_var_u32(1).unwrap();
        write_action_type(&mut bytes, 10).unwrap();
        bytes.write_var_u32(3).unwrap();
        bytes.write_u8(1).unwrap();
        bytes.write_var_u32(0).unwrap();
        bytes.write_i32_le(-1).unwrap();
        let parsed = ItemStackRequest::read(&mut ByteReader::from(bytes.as_slice())).unwrap();
        assert_eq!(parsed.craft_actions(), vec![(-4, 2, 3), (-5, 3, 1)]);
    }

    #[test]
    fn craft_recipe_round_trip() {
        let packet = ItemStackRequest {
            requests: vec![ItemStackRequestEntry {
                request_id: 5,
                actions: vec![
                    ItemStackRequestAction::CraftRecipe {
                        recipe_network_id: 9,
                        times: 2,
                    },
                    ItemStackRequestAction::CraftRecipeAuto {
                        recipe_network_id: 10,
                        times: 1,
                    },
                ],
            }],
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ItemStackRequest::read(&mut reader).unwrap();
        assert_eq!(decoded, packet);
        assert_eq!(decoded.craft_actions().len(), 2);
    }

    #[test]
    fn malformed_request_returns_error_without_panic() {
        // Truncated varint.
        let mut reader = ByteReader::from(&[0xFF, 0xFF, 0xFF][..]);
        assert!(ItemStackRequest::read(&mut reader).is_err());
        // Declared count exceeds the bound.
        let mut writer = ByteWriter::new();
        writer.write_var_u32(10_000).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        assert!(ItemStackRequest::read(&mut reader).is_err());
        // Empty buffer.
        let mut reader = ByteReader::from(&[][..]);
        assert!(ItemStackRequest::read(&mut reader).is_err());
    }

    #[test]
    fn unknown_action_does_not_panic_and_yields_no_craft() {
        let mut writer = ByteWriter::new();
        writer.write_var_u32(1).unwrap();
        writer.write_var_i32(1).unwrap();
        writer.write_var_u32(1).unwrap();
        writer.write_u8(3).unwrap(); // Drop (unsupported payload shape here)
        let mut reader = ByteReader::from(writer.as_slice());
        assert!(ItemStackRequest::read(&mut reader).is_err());
    }

    #[test]
    fn mixed_take_place_then_craft_stays_aligned() {
        // Real clients arrange (Take/Place) then execute (CraftRecipe) in
        // one request. Moves are parsed for alignment; only the craft
        // yields an intent.
        let slot = |container: u8, slot: u8| RequestSlotRef {
            container_enum: container,
            dynamic_slot: None,
            slot,
            net_id: 0,
        };
        let packet = ItemStackRequest {
            requests: vec![ItemStackRequestEntry {
                request_id: 7,
                actions: vec![
                    ItemStackRequestAction::Move {
                        action_type: ItemStackRequestActionType::TAKE,
                        amount: Some(2),
                        source: slot(0, 5),
                        destination: slot(1, 0),
                    },
                    ItemStackRequestAction::Move {
                        action_type: ItemStackRequestActionType::PLACE,
                        amount: Some(2),
                        source: slot(1, 0),
                        destination: slot(0, 6),
                    },
                    ItemStackRequestAction::CraftRecipeAuto {
                        recipe_network_id: 12,
                        times: 1,
                    },
                ],
            }],
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ItemStackRequest::read(&mut reader).unwrap();
        assert_eq!(decoded, packet);
        assert!(decoded.has_craft());
        assert_eq!(decoded.craft_actions(), vec![(7, 12, 1)]);
    }
}
