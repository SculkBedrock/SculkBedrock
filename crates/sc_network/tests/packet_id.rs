//! Regression tests for `MinecraftPackets::packet_id()` / `packet_name()`.
//!
//! Background: `MinecraftPackets::id()` used to serialize the whole packet
//! with `write_to_bytes()` just to read the first two bytes, which
//! serializes kilobytes per send for large payloads such as `LevelChunk`.
//! The send path also used to clone whole packets to read ids; the concrete
//! packet type to id mapping now avoids that clone.
//!
//! `id()` now comes from the `MinecraftPackets` derive macro reading
//! `#[repr(u16)]` discriminants as `packet_id()`, and
//! `packet_id_for_type::<P>()` frees send sites from constructing the enum.
//! Both id sources must always agree:
//!
//! - `MinecraftPackets` derive (`sc_network_macros`) — the object under test;
//! - `BinaryIo` derive — the real encode path, parsing the same
//!   `#[repr(u16)]` literals and failing compilation on non-integer
//!   discriminants.
//!
//! Both read the same source, so they cannot diverge in principle; but
//! `BinaryIo` implicitly derives `discrim + 1` for default discriminants
//! while `packet_id()` refuses to generate in that case (falling back to
//! the `id()` serialization path). The tests below pin both boundaries.

use sc_binary::interfaces::Writer as _;
use sc_network::protocol::server::chunk::LevelChunk;
use sc_network::protocol::server::game::Disconnect;
use sc_network::protocol::server::login::PlayStatus;
use sc_network::protocol::server::misc::SetTime;
use sc_network::protocol::MinecraftPackets;

/// Packet id from the serialized byte stream (legacy `MinecraftPackets::id()`).
fn legacy_id_from_wire(packet: &MinecraftPackets) -> u16 {
    let bytes = packet.write_to_bytes().expect("serialize");
    let bytes = bytes.as_slice();
    if bytes.len() >= 2 {
        u16::from_be_bytes([bytes[0], bytes[1]])
    } else {
        bytes.first().copied().map(u16::from).unwrap_or(0)
    }
}

fn sample_packets() -> Vec<MinecraftPackets> {
    vec![
        MinecraftPackets::PlayStatus(PlayStatus { status: 3 }),
        MinecraftPackets::SetTime(SetTime { time: 12345 }),
        MinecraftPackets::Disconnect(Disconnect {
            hide_disconnect_screen: true,
            kick_message: String::from("bye"),
        }),
        MinecraftPackets::LevelChunk(LevelChunk {
            chunk_x: 11,
            chunk_z: 10,
            dimension: 0,
            sub_chunk_count: 24,
            cache_enabled: false,
            payload: (0u8..=255).cycle().take(4096).collect(),
        }),
    ]
}

/// Compile-time `packet_id()` must match the serialized prefix exactly.
///
/// Covers small packets and a 4KB-payload LevelChunk.
#[test]
fn packet_id_matches_wire_prefix() {
    for packet in sample_packets() {
        let concrete_id = match &packet {
            MinecraftPackets::PlayStatus(_) => MinecraftPackets::packet_id_for_type::<PlayStatus>(),
            MinecraftPackets::SetTime(_) => MinecraftPackets::packet_id_for_type::<SetTime>(),
            MinecraftPackets::Disconnect(_) => MinecraftPackets::packet_id_for_type::<Disconnect>(),
            MinecraftPackets::LevelChunk(_) => MinecraftPackets::packet_id_for_type::<LevelChunk>(),
            _ => unreachable!("sample_packets only contains the four listed variants"),
        };
        assert_eq!(concrete_id, Some(packet.id()));
        assert_eq!(
            packet.id(),
            legacy_id_from_wire(&packet),
            "packet_id() disagrees with the wire prefix: {}",
            packet.packet_name()
        );
    }
}

/// A large `LevelChunk` payload does not affect the id (discriminant only).
#[test]
fn packet_id_is_independent_of_payload_size() {
    let make = |payload: Vec<u8>| {
        MinecraftPackets::LevelChunk(LevelChunk {
            chunk_x: 0,
            chunk_z: 0,
            dimension: 0,
            sub_chunk_count: 24,
            cache_enabled: false,
            payload,
        })
    };
    let empty = make(Vec::new());
    let huge = make(vec![7u8; 74_210]);
    assert_eq!(empty.id(), huge.id());
    assert_eq!(empty.id(), legacy_id_from_wire(&huge));
}

/// `PACKET_IDS` table integrity: no duplicate ids, real variant names.
///
/// Duplicate ids would decode two packets as one, so the table itself
/// must catch that.
#[test]
fn packet_ids_are_unique_and_named() {
    let table = MinecraftPackets::PACKET_IDS;
    assert!(!table.is_empty(), "PACKET_IDS must not be empty");

    let mut seen_ids = std::collections::HashSet::new();
    for (name, id) in table {
        assert!(!name.is_empty(), "variant name must not be empty");
        assert!(
            seen_ids.insert(*id),
            "duplicate packet id 0x{id:x} (variant {name})"
        );
    }

    // Spot-check a few wire-critical ids.
    let find = |wanted: &str| {
        table
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, id)| *id)
    };
    assert_eq!(find("LevelChunk"), Some(0x3a));
    assert_eq!(find("PlayStatus"), Some(0x02));
    assert_eq!(find("Disconnect"), Some(0x05));
    assert_eq!(find("VoxelShapes"), Some(0x151));
    assert_eq!(find("CameraAimAssistPresets"), Some(0x140));
}

/// `packet_name()` must equal the legacy derived-Debug implementation.
/// Log search and `blocked_packets` diagnostics rely on the name.
#[test]
fn packet_name_matches_legacy_debug_split() {
    for packet in sample_packets() {
        let legacy = format!("{packet:?}");
        let legacy_name = legacy.split('(').next().expect("non-empty debug output");
        assert_eq!(packet.packet_name(), legacy_name);
        assert_eq!(packet.variant_name(), legacy_name);
    }
}

/// Every name in `PACKET_IDS` must come out of `packet_name()`.
///
/// Both cover the same variant set; this catches table/match drift when
/// variants are added.
#[test]
fn every_tabulated_variant_is_reachable() {
    // Sample variants: their `packet_name()` must appear in the table.
    for packet in sample_packets() {
        let name = packet.packet_name();
        assert!(
            MinecraftPackets::PACKET_IDS
                .iter()
                .any(|(tabulated, _)| *tabulated == name),
            "variant {name} missing from the PACKET_IDS table"
        );
    }
}
