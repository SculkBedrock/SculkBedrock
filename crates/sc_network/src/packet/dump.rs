use crate::packet::batch_packet::BatchPacket;
use crate::protocol::MinecraftPackets;
use sc_binary::interfaces::Writer;
use sc_ecs::entity::EntityId;
use sc_log::t_log;
use sha2::{Digest, Sha256};
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

struct DumpState {
    dir: PathBuf,
    index: File,
    sequence: u64,
}

static STATE: OnceLock<Mutex<Option<DumpState>>> = OnceLock::new();
static ENABLED: OnceLock<bool> = OnceLock::new();
static HEX_ENABLED: OnceLock<bool> = OnceLock::new();

/// Packet capture switch. Read once per process and cached.
///
/// The switch feeds every outbound packet path, and environment variables
/// are fixed before process start, so caching is safe.
pub(crate) fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var("SC_PACKET_DUMP")
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false)
    })
}

fn hex_enabled() -> bool {
    *HEX_ENABLED.get_or_init(|| {
        std::env::var("SC_PACKET_DUMP_HEX")
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false)
    })
}

fn state() -> Option<&'static Mutex<Option<DumpState>>> {
    if !enabled() {
        return None;
    }

    Some(STATE.get_or_init(|| {
        let dir = std::env::var_os("SC_PACKET_DUMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("diagnostics/sc-2168/logs/packet-dump"));

        if let Err(error) = create_dir_all(&dir) {
            log::warn!(
                "{}",
                t_log!(
                    "console.packet.dump_create_fail",
                    dir = dir.display(),
                    error = error
                )
            );
            return Mutex::new(None);
        }

        let index_path = dir.join("sc-packets.index");
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(index_path)
        {
            Ok(index) => Mutex::new(Some(DumpState {
                dir,
                index,
                sequence: 0,
            })),
            Err(error) => {
                log::warn!(
                    "{}",
                    t_log!("console.packet.dump_index_fail", error = error)
                );
                Mutex::new(None)
            }
        }
    }))
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn full_hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push_str(&format!("{byte:02x}"));
    }
    result
}

fn sensitive(packet_name: &str) -> bool {
    packet_name.contains("Login") || packet_name.contains("Handshake")
}

pub fn record_bytes(
    direction: &str,
    stage: &str,
    entity: Option<EntityId>,
    packet_id: Option<u32>,
    packet_name: &str,
    bytes: &[u8],
    details: &str,
) {
    // Return immediately when capture is off, before any serialization
    // or string building (serializing first would waste a full encode
    // per packet).
    if !enabled() {
        return;
    }
    let Some(state) = state() else {
        return;
    };
    let Ok(mut state) = state.lock() else {
        return;
    };
    let Some(state) = state.as_mut() else {
        return;
    };

    state.sequence = state.sequence.wrapping_add(1);
    let sequence = state.sequence;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let redacted = sensitive(packet_name);
    let digest = if redacted {
        "redacted".to_string()
    } else {
        sha256(bytes)
    };
    let entity = entity
        .map(|value| format!("{value:?}"))
        .unwrap_or_else(|| "-".to_string());
    let base = format!(
        "{sequence:09}-{direction}-{stage}-{}",
        sanitize(packet_name)
    );

    let file = if redacted {
        "-".to_string()
    } else {
        let path = state.dir.join(format!("{base}.bin"));
        match File::create(&path).and_then(|mut file| file.write_all(bytes).map(|_| file)) {
            Ok(_) => {
                if hex_enabled() {
                    let hex_path = state.dir.join(format!("{base}.hex"));
                    if let Ok(mut hex_file) = File::create(hex_path) {
                        let _ = hex_file.write_all(full_hex(bytes).as_bytes());
                    }
                }
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("-")
                    .to_string()
            }
            Err(error) => {
                log::warn!(
                    "{}",
                    t_log!(
                        "console.packet.dump_write_fail",
                        path = path.display(),
                        error = error
                    )
                );
                "-".to_string()
            }
        }
    };

    let _ = writeln!(
        state.index,
        "seq={sequence} timestamp={timestamp} direction={direction} stage={stage} entity={entity} packet_id={} packet_name={} bytes={} sha256={} redacted={} file={} details={}",
        packet_id
            .map(|value| format!("0x{value:x}"))
            .unwrap_or_else(|| "-".to_string()),
        sanitize(packet_name),
        bytes.len(),
        digest,
        redacted,
        file,
        sanitize(details),
    );
    let _ = state.index.flush();
}

pub fn record_batch(direction: &str, entity: Option<EntityId>, stage: &str, batch: &BatchPacket) {
    // No serialization happens while capture is off.
    if !enabled() {
        return;
    }
    for packet in batch.packets() {
        let Ok(bytes) = packet.write_to_bytes() else {
            continue;
        };
        let packet_id = bytes
            .as_slice()
            .get(..2)
            .map(|id| u16::from_be_bytes([id[0], id[1]]) as u32);
        record_bytes(
            direction,
            stage,
            entity,
            packet_id,
            packet.packet_name(),
            bytes.as_slice(),
            "minecraft_packet",
        );
    }
}

pub fn record_wire(
    direction: &str,
    entity: Option<EntityId>,
    stage: &str,
    bytes: &[u8],
    details: &str,
) {
    if !enabled() {
        return;
    }
    let packet_name = match stage {
        "batch_plain" => "BatchPlain",
        "batch_compressed" => "BatchCompressed",
        "bedrock_wire" => "BatchWire",
        _ => "ByteBuf",
    };
    record_bytes(direction, stage, entity, None, packet_name, bytes, details);
}

/// Variant name for diagnostics: compile-time constant string, zero
/// allocation. Debug-formatting whole payloads here would serialize tens
/// of kilobytes per packet on the send path, so the name comes directly
/// from the derived `packet_name()`.
pub(crate) fn packet_variant_name(packet: &MinecraftPackets) -> &'static str {
    packet.packet_name()
}

#[cfg(test)]
mod tests {
    use super::packet_variant_name;
    use crate::protocol::server::chunk::LevelChunk;
    use crate::protocol::MinecraftPackets;

    /// `packet_name()` must match the variant name the derived Debug
    /// implementation produced: log search and blocked_packets diagnostics
    /// depend on it.
    #[test]
    fn packet_name_matches_legacy_debug_split() {
        let packets = vec![
            MinecraftPackets::LevelChunk(LevelChunk {
                chunk_x: 11,
                chunk_z: 10,
                dimension: 0,
                sub_chunk_count: 24,
                cache_enabled: false,
                payload: vec![0u8; 4096],
            }),
            MinecraftPackets::Disconnect(crate::protocol::server::game::Disconnect {
                hide_disconnect_screen: false,
                kick_message: String::from("bye"),
            }),
        ];
        for packet in &packets {
            let legacy = format!("{packet:?}");
            let legacy_name = legacy.split('(').next().unwrap();
            assert_eq!(packet_variant_name(packet), legacy_name);
        }
    }

    /// Large-payload LevelChunk: `packet_name()` must not allocate
    /// (guaranteed by `const fn` + `&'static str`).
    #[test]
    fn packet_name_is_static_str() {
        let name: &'static str = packet_variant_name(&MinecraftPackets::LevelChunk(LevelChunk {
            chunk_x: 0,
            chunk_z: 0,
            dimension: 0,
            sub_chunk_count: 24,
            cache_enabled: false,
            payload: Vec::new(),
        }));
        assert_eq!(name, "LevelChunk");
    }
}
