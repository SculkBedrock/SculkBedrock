//! NBT (Named Binary Tag) value model with network/local serialization.
//!
//! `NbtValue` is the value enum and `CompoundNbt` the key-value container; the `network`/`local` submodules
//! implement Bedrock network NBT (varint-prefixed) and local NBT (LE/BE variants)
//! reads/writes respectively. LevelDB palettes, block entities, and pack data all use this carrier.

use crate::compound::CompoundNbt;
use crate::writer::{NbtWriteTrait, NbtWriter};
use serde::Serialize;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};
use sc_binary::ByteWriter;
use sc_utils::game::gamemode::Gamemode;
use sc_utils::game::structs::server::Server;
use uuid::Uuid;

const MAX_CLONE_ESTIMATE_LIST_DEPTH: usize = 128;

pub mod compound;
pub mod local;
mod r#macro;
pub mod network;
pub mod reader;
#[cfg(test)]
mod test;
pub mod writer;

#[derive(Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum NbtValue {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<NbtValue>),
    Compound(CompoundNbt),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl NbtValue {
    /// Estimate heap buffers duplicated by cloning this NBT value.
    ///
    /// Compound maps use Arc-backed copy-on-write storage and therefore are
    /// shared by `Clone`; their names are still cloned. Lists and owned
    /// strings/arrays are copied. Excessive nested-list depth returns
    /// `usize::MAX`, allowing bounded callers to reject before cloning.
    pub fn estimated_clone_heap_bytes(&self) -> usize {
        self.estimated_clone_heap_bytes_at_depth(0)
    }

    fn estimated_clone_heap_bytes_at_depth(&self, depth: usize) -> usize {
        if depth > MAX_CLONE_ESTIMATE_LIST_DEPTH {
            return usize::MAX;
        }
        match self {
            NbtValue::ByteArray(values) => values.capacity(),
            NbtValue::String(value) => value.capacity(),
            NbtValue::List(values) => values
                .capacity()
                .saturating_mul(std::mem::size_of::<NbtValue>())
                .saturating_add(values.iter().fold(0usize, |total, value| {
                    total.saturating_add(value.estimated_clone_heap_bytes_at_depth(depth + 1))
                })),
            NbtValue::Compound(value) => value.name.as_ref().map_or(0, String::capacity),
            NbtValue::IntArray(values) => {
                values.capacity().saturating_mul(std::mem::size_of::<i32>())
            }
            NbtValue::LongArray(values) => {
                values.capacity().saturating_mul(std::mem::size_of::<i64>())
            }
            NbtValue::Byte(_)
            | NbtValue::Short(_)
            | NbtValue::Int(_)
            | NbtValue::Long(_)
            | NbtValue::Float(_)
            | NbtValue::Double(_) => 0,
        }
    }

    pub fn as_i8(&self) -> Option<i8> {
        match self {
            NbtValue::Byte(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i16(&self) -> Option<i16> {
        match self {
            NbtValue::Short(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            NbtValue::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            NbtValue::Long(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            NbtValue::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            NbtValue::Double(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i8_array(&self) -> Option<&Vec<i8>> {
        match self {
            NbtValue::ByteArray(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&String> {
        match self {
            NbtValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&Vec<NbtValue>> {
        match self {
            NbtValue::List(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_compound(&self) -> Option<&CompoundNbt> {
        match self {
            NbtValue::Compound(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_compound_mut(&mut self) -> Option<&mut CompoundNbt> {
        match self {
            NbtValue::Compound(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_i32_array(&self) -> Option<&Vec<i32>> {
        match self {
            NbtValue::IntArray(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_i64_array(&self) -> Option<&Vec<i64>> {
        match self {
            NbtValue::LongArray(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        let b = self.as_i8()?;
        Some(b != 0)
    }

    pub fn tag(&self) -> u8 {
        match self {
            NbtValue::Byte(_) => 1,
            NbtValue::Short(_) => 2,
            NbtValue::Int(_) => 3,
            NbtValue::Long(_) => 4,
            NbtValue::Float(_) => 5,
            NbtValue::Double(_) => 6,
            NbtValue::ByteArray(_) => 7,
            NbtValue::String(_) => 8,
            NbtValue::List(_) => 9,
            NbtValue::Compound(_) => 10,
            NbtValue::IntArray(_) => 11,
            NbtValue::LongArray(_) => 12,
        }
    }
}

pub trait SCNBTByteWriter {
    fn write_nbt<T: NbtWriteTrait>(&mut self, nbt: &NbtValue) -> io::Result<()>;
}

impl SCNBTByteWriter for ByteWriter {
    fn write_nbt<T: NbtWriteTrait>(&mut self, nbt: &NbtValue) -> io::Result<()> {
        let mut nbt_writer = NbtWriter::from_writer(self);
        nbt_writer.write::<T>(nbt)
    }
}

pub trait SCNBTServer {
    fn get_offline_player_nbt(&self, uuid: Uuid, default_gamemode: Gamemode)
        -> Option<CompoundNbt>;
}

impl SCNBTServer for Server {
    fn get_offline_player_nbt(
        &self,
        uuid: Uuid,
        default_gamemode: Gamemode,
    ) -> Option<CompoundNbt> {
        if uuid.is_nil() {
            return None;
        }
        // Look up the player database.

        // Create a default record when none is found.
        let now_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut nbt = CompoundNbt::new(None);
        nbt.insert("firstPlayed", NbtValue::Long(now_time)) // First play timestamp
            .insert("lastPlayed", NbtValue::Long(now_time)) // Last play timestamp
            .insert("Level", NbtValue::String("Overworld".to_string())) // Current world name
            .insert("Inventory", NbtValue::List(vec![])) // Inventory
            .insert("Achievements", NbtValue::Compound(CompoundNbt::new(None))) // Achievements
            .insert("EXP", NbtValue::Int(0)) // Experience points
            .insert("expLevel", NbtValue::Int(0)) // Experience level
            .insert("playerGameType", NbtValue::Int(default_gamemode.to_i32()))
            .insert(
                "Motion",
                NbtValue::List(vec![
                    NbtValue::Double(0.0),
                    NbtValue::Double(0.0),
                    NbtValue::Double(0.0),
                ]),
            )
            .insert(
                "Rotation",
                NbtValue::List(vec![NbtValue::Float(0.0), NbtValue::Float(0.0)]),
            )
            .insert("FallDistance", NbtValue::Float(0.0))
            .insert("Fire", NbtValue::Short(0))
            .insert("Air", NbtValue::Short(300))
            .insert("OnGround", NbtValue::Byte(1))
            .insert("Invulnerable", NbtValue::Byte(0));
        // Persist the player NBT.

        Some(nbt)
    }
}

#[cfg(test)]
mod clone_size_tests {
    use super::*;

    #[test]
    fn clone_estimate_counts_owned_buffers_and_shared_compound_name() {
        let bytes = vec![7i8; 96];
        let string = String::from("a moderately sized NBT string");
        let expected_min = bytes.capacity() + string.capacity();
        let value = NbtValue::List(vec![NbtValue::ByteArray(bytes), NbtValue::String(string)]);
        assert!(
            value.estimated_clone_heap_bytes() >= expected_min,
            "list estimate includes its recursively owned children"
        );

        let name = String::from("block_entity");
        let name_capacity = name.capacity();
        let mut compound = CompoundNbt::new(Some(name));
        compound.insert("payload", NbtValue::ByteArray(vec![1; 4_096]));
        assert_eq!(
            NbtValue::Compound(compound).estimated_clone_heap_bytes(),
            name_capacity,
            "a Compound clone shares its Arc-backed map"
        );
    }

    #[test]
    fn clone_estimate_rejects_excessively_nested_lists_without_unbounded_recursion() {
        let mut value = NbtValue::Int(1);
        for _ in 0..=MAX_CLONE_ESTIMATE_LIST_DEPTH {
            value = NbtValue::List(vec![value]);
        }
        assert_eq!(value.estimated_clone_heap_bytes(), usize::MAX);
    }
}
