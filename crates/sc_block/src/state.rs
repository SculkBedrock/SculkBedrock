//! Dense block-state model and capability bitmask.
//!
//! [`BlockStateId`] is the **dense ordinal index** for logic hot paths (`Vec<T>` tables indexed by it),
//! running alongside the storage/network [`sc_world::chunk::BlockRuntimeId`] (FNV1a-32 hash).
//! Hashes cover storage and network, dense ids cover logic.
//! [`BlockComponentFlags`] is the dense capability-bitmask table indexed by [`BlockStateId`].

use std::sync::{Arc, RwLock};

use sc_ecs::resource::Resource;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;

/// Dense block-state index: logic hot-path subscript (attribute tables address by it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockStateId(pub u32);

/// Attribute lookup value for a single block state.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockPropertyValue {
    Bool(bool),
    Int(i32),
    Enum(Arc<str>),
}

pub(crate) fn property_to_nbt(value: &BlockPropertyValue) -> NbtValue {
    match value {
        BlockPropertyValue::Bool(b) => NbtValue::Byte(*b as i8),
        BlockPropertyValue::Int(i) => NbtValue::Int(*i),
        BlockPropertyValue::Enum(s) => NbtValue::String(s.to_string()),
    }
}

/// A registered block state: hash plus identifier plus raw states NBT plus palette network id.
#[derive(Clone, Debug)]
pub struct BlockState {
    /// FNV1a-32 state hash (equals the [`sc_world::chunk::BlockRuntimeId`] value).
    pub hash: u32,
    /// Block identifier (e.g. `minecraft:oak_log`).
    pub name: Arc<str>,
    /// Raw states NBT (unknown fields preserved, re-serialization round-trips losslessly).
    pub states: Option<CompoundNbt>,
    /// Protocol network runtime id from the palette (sequential palette order).
    pub network_id: u32,
}

/// Capability bitmask constants (core definition; never names concrete blocks).
///
/// `IS_AIR` / `IS_UNKNOWN` are protocol sentinels (the sole layering exception); the rest are
/// data-driven bits populated once `blocks/properties.json` lands, defaulting to 0.
pub mod flags {
    // Protocol sentinels.
    pub const IS_AIR: u64 = 1 << 0;
    pub const IS_UNKNOWN: u64 = 1 << 1;
    /// Whether this state carries non-empty states (use it to decide if attribute parsing is needed).
    pub const HAS_STATES: u64 = 1 << 2;
    // Data-driven flags (populated once properties.json lands).
    pub const SOLID: u64 = 1 << 3;
    pub const TRANSPARENT: u64 = 1 << 4;
    pub const REPLACEABLE: u64 = 1 << 5;
    pub const LIQUID: u64 = 1 << 6;
    pub const RANDOM_TICK: u64 = 1 << 7;
    pub const NEEDS_SUPPORT: u64 = 1 << 8;
    pub const CAN_CONTAIN_LIQUID: u64 = 1 << 9;
    pub const UNBREAKABLE: u64 = 1 << 10;
    pub const NO_COLLISION: u64 = 1 << 11;
}

/// Dense capability-bitmask table indexed by [`BlockStateId`].
///
/// Built by [`crate::registry::BlockStateRegistry`] after palette load (SCLoad), read-only afterwards.
/// `has(state, bit)` is an O(1) array probe backing logic hot paths such as random-tick
/// filtering and redstone signals. Plugin side tables (e.g. redstone conductivity) use
/// `BlockColumn<T>`; this table only carries core-level capability bits.
#[derive(Resource, Default)]
pub struct BlockComponentFlags {
    table: RwLock<Vec<u64>>,
}

impl Clone for BlockComponentFlags {
    fn clone(&self) -> Self {
        Self {
            table: RwLock::new(self.table.read().expect("lock poisoned").clone()),
        }
    }
}

impl BlockComponentFlags {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds from the registry state table (once at SCLoad).
    pub fn build(&mut self, registry: &crate::registry::BlockStateRegistry) {
        let air = registry.air_state_id();
        let unknown = registry.unknown_state_id();
        let snapshot = registry.states_snapshot();
        let mut table = vec![0u64; snapshot.len()];
        if let Some(air) = air {
            if let Some(bits) = table.get_mut(air.0 as usize) {
                *bits |= flags::IS_AIR;
            }
        }
        if let Some(unknown) = unknown {
            if let Some(bits) = table.get_mut(unknown.0 as usize) {
                *bits |= flags::IS_UNKNOWN;
            }
        }
        for (index, state) in snapshot.iter().enumerate() {
            if state
                .states
                .as_ref()
                .is_some_and(|compound| !compound.is_empty())
            {
                table[index] |= flags::HAS_STATES;
            }
        }
        *self
            .table
            .write()
            .expect("BlockComponentFlags lock poisoned") = table;
    }

    /// Builds from a `.block.json` snapshot (once at SCLoad, read-only afterwards).
    ///
    /// Capability bits come only from explicit file declarations (see the no-guessing
    /// `BlockJsonSnapshot::flag_bits` mapping); solidity/transparency have no data source and stay unset.
    pub fn build_from_snapshot(&mut self, snapshot: &crate::block_json::BlockJsonSnapshot) {
        let mut table = vec![0u64; snapshot.state_count()];
        for (index, bits) in table.iter_mut().enumerate() {
            *bits = snapshot.flag_bits(index as u32);
        }
        *self
            .table
            .write()
            .expect("BlockComponentFlags lock poisoned") = table;
    }

    /// O(1) bitmask lookup by dense id.
    pub fn has(&self, state: BlockStateId, bit: u64) -> bool {
        self.table
            .read()
            .ok()
            .and_then(|table| table.get(state.0 as usize).copied())
            .is_some_and(|bits| bits & bit != 0)
    }

    /// Sets one state's capability bits (used to fill per-block data once `blocks/properties.json` lands).
    pub fn set(&self, state: BlockStateId, bit: u64) {
        if let Ok(mut table) = self.table.write() {
            if let Some(bits) = table.get_mut(state.0 as usize) {
                *bits |= bit;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.table.read().map(|table| table.len()).unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// states NBT to canonical signature (keys in dict order; bool as `true`/`false`, int as decimal,
/// string verbatim). Pairs with the O(1) signature lookup in
/// [`crate::registry::BlockStateRegistry::state_of`] / `with_property`.
pub(crate) fn state_signature(states: &Option<CompoundNbt>) -> Box<str> {
    let Some(states) = states else {
        return Box::from("");
    };
    if states.is_empty() {
        return Box::from("");
    }
    let mut pairs: Vec<(String, String)> = states
        .iter()
        .map(|(key, value)| (key.clone(), value_token(value)))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::new();
    for (index, (key, value)) in pairs.iter().enumerate() {
        if index > 0 {
            out.push(';');
        }
        out.push_str(key);
        out.push('=');
        out.push_str(value);
    }
    Box::from(out)
}

fn value_token(value: &NbtValue) -> String {
    match value {
        NbtValue::String(s) => s.clone(),
        NbtValue::Int(i) => i.to_string(),
        NbtValue::Long(l) => l.to_string(),
        NbtValue::Byte(b) => {
            if *b != 0 {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        other => format!("{other:?}"),
    }
}
