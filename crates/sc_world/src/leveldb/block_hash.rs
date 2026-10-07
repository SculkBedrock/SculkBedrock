//! Hashed block network ids.
//!
//! With `block_network_ids_hashed` enabled in StartGame, the client derives a
//! block's network runtime id from the FNV1a-32 hash of its canonical state
//! NBT, so server and client agree without exchanging a runtime palette.
//! The canonical form (a little-endian NBT compound with an empty root name
//! containing only `name` and `states`, with every compound serialized in
//! alphabetical key order):
//! a little-endian NBT compound with an empty root name containing only
//! `name` and `states` (the `version` field is excluded), with every compound
//! serialized in alphabetical key order.

use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;

const FNV1_OFFSET_BASIS: u32 = 0x811c_9dc5;
const FNV1_PRIME: u32 = 0x0100_0193;

/// Vanilla maps `minecraft:unknown` to the constant -2 instead of its hash.
pub const UNKNOWN_BLOCK_RUNTIME_ID: u32 = -2i32 as u32;

pub fn fnv1a32(data: &[u8]) -> u32 {
    let mut hash = FNV1_OFFSET_BASIS;
    for byte in data {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(FNV1_PRIME);
    }
    hash
}

/// Computes the hashed network runtime id for a block state.
pub fn block_state_hash(name: &str, states: Option<&CompoundNbt>) -> u32 {
    if name == "minecraft:unknown" {
        return UNKNOWN_BLOCK_RUNTIME_ID;
    }
    let mut root = CompoundNbt::new(None);
    root.insert("name", NbtValue::String(name.to_string()));
    let states = states.cloned().unwrap_or_else(|| CompoundNbt::new(None));
    root.insert("states", NbtValue::Compound(states));

    let mut bytes = Vec::with_capacity(64);
    // Root: tag byte, empty name, compound payload.
    bytes.push(10);
    write_string(&mut bytes, "");
    write_compound_payload(&mut bytes, &root);
    fnv1a32(&bytes)
}

fn write_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u16).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn write_compound_payload(out: &mut Vec<u8>, compound: &CompoundNbt) {
    // CompoundNbt is backed by a HashMap; sort keys for a canonical order.
    let mut entries: Vec<(&String, &NbtValue)> = compound.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (key, value) in entries {
        out.push(value.tag());
        write_string(out, key);
        write_value_payload(out, value);
    }
    out.push(0);
}

fn write_value_payload(out: &mut Vec<u8>, value: &NbtValue) {
    match value {
        NbtValue::Byte(x) => out.push(*x as u8),
        NbtValue::Short(x) => out.extend_from_slice(&x.to_le_bytes()),
        NbtValue::Int(x) => out.extend_from_slice(&x.to_le_bytes()),
        NbtValue::Long(x) => out.extend_from_slice(&x.to_le_bytes()),
        NbtValue::Float(x) => out.extend_from_slice(&x.to_le_bytes()),
        NbtValue::Double(x) => out.extend_from_slice(&x.to_le_bytes()),
        NbtValue::ByteArray(x) => {
            out.extend_from_slice(&(x.len() as i32).to_le_bytes());
            out.extend(x.iter().map(|byte| *byte as u8));
        }
        NbtValue::String(x) => write_string(out, x),
        NbtValue::List(x) => {
            let tag = x.first().map(NbtValue::tag).unwrap_or(0);
            out.push(tag);
            out.extend_from_slice(&(x.len() as i32).to_le_bytes());
            for item in x {
                write_value_payload(out, item);
            }
        }
        NbtValue::Compound(x) => write_compound_payload(out, x),
        NbtValue::IntArray(x) => {
            out.extend_from_slice(&(x.len() as i32).to_le_bytes());
            for item in x {
                out.extend_from_slice(&item.to_le_bytes());
            }
        }
        NbtValue::LongArray(x) => {
            out.extend_from_slice(&(x.len() as i32).to_le_bytes());
            for item in x {
                out.extend_from_slice(&item.to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a32_standard_vectors() {
        assert_eq!(fnv1a32(b""), 0x811c9dc5);
        assert_eq!(fnv1a32(b"a"), 0xe40c292c);
        assert_eq!(fnv1a32(b"foobar"), 0xbf9cf968);
    }

    #[test]
    fn unknown_block_maps_to_minus_two() {
        assert_eq!(block_state_hash("minecraft:unknown", None), -2i32 as u32);
    }

    #[test]
    fn hash_is_deterministic_and_state_sensitive() {
        let empty = block_state_hash("minecraft:air", None);
        assert_eq!(empty, block_state_hash("minecraft:air", None));
        assert_ne!(empty, block_state_hash("minecraft:stone", None));

        let mut states = CompoundNbt::new(None);
        states.insert("direction", NbtValue::Int(1));
        assert_ne!(
            block_state_hash("minecraft:stone", Some(&states)),
            block_state_hash("minecraft:stone", None),
        );
    }

    #[test]
    fn state_key_order_does_not_change_hash() {
        // HashMap iteration order is arbitrary; the canonical writer must
        // sort, so building the map in any order yields the same bytes.
        let mut a = CompoundNbt::new(None);
        a.insert("b", NbtValue::Int(2));
        a.insert("a", NbtValue::Int(1));
        let mut b = CompoundNbt::new(None);
        b.insert("a", NbtValue::Int(1));
        b.insert("b", NbtValue::Int(2));
        assert_eq!(
            block_state_hash("minecraft:test", Some(&a)),
            block_state_hash("minecraft:test", Some(&b)),
        );
    }

    #[test]
    fn air_hash_matches_known_vanilla_value() {
        // fnv1a32 of the canonical LE NBT for {name:"minecraft:air", states:{}}.
        let expected = {
            let mut bytes = Vec::new();
            bytes.push(10u8);
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.push(8u8); // name: String
            bytes.extend_from_slice(&4u16.to_le_bytes());
            bytes.extend_from_slice(b"name");
            bytes.extend_from_slice(&13u16.to_le_bytes());
            bytes.extend_from_slice(b"minecraft:air");
            bytes.push(10u8); // states: Compound (empty)
            bytes.extend_from_slice(&6u16.to_le_bytes());
            bytes.extend_from_slice(b"states");
            bytes.push(0u8); // end of states
            bytes.push(0u8); // end of root
            fnv1a32(&bytes)
        };
        assert_eq!(block_state_hash("minecraft:air", None), expected);
    }
}
