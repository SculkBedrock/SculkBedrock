//! Block-state bootstrap dictionary (hash to name+states raw NBT).
//!
//! FNV1a-32 hashes are irreversible, and LevelDB palette parsing is the only place holding both the hash
//! and the raw `name+states` (`leveldb/format.rs::read_palette_entry`).
//! Registering there lets the server answer which block a hash is without data files,
//! and guarantees any state seen on disk (older saves, custom blocks) round-trips losslessly.
//! Before the version-pack palette arrives, this dictionary is the fallback; afterwards it stays
//! for cross-checks and superset fallback.
//!
//! Performance: registration happens per palette entry (ones to dozens per store, not per block);
//! the hit path takes one read lock + u32 lookup with zero allocation; entries are built only
//! on first sight of a hash (inserted under double-checked locking).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock, RwLock};

use ahash::RandomState;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;

use crate::leveldb::block_hash::{block_state_hash, UNKNOWN_BLOCK_RUNTIME_ID};

/// One seen block state: identifier + raw states NBT.
/// States keep unknown fields so re-serialization round-trips losslessly.
#[derive(Clone, Debug)]
pub struct BlockStateEntry {
    pub name: String,
    pub states: Option<CompoundNbt>,
}

impl fmt::Display for BlockStateEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.name)?;
        let Some(states) = &self.states else {
            return Ok(());
        };
        if states.is_empty() {
            return Ok(());
        }
        let mut entries: Vec<(&String, &NbtValue)> = states.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        write!(formatter, "{{")?;
        for (index, (key, value)) in entries.iter().enumerate() {
            if index > 0 {
                write!(formatter, ", ")?;
            }
            write!(formatter, "{key}=")?;
            match value {
                NbtValue::Byte(byte) => write!(formatter, "{}", *byte != 0)?,
                NbtValue::Int(int) => write!(formatter, "{int}")?,
                NbtValue::String(string) => write!(formatter, "{string}")?,
                other => write!(formatter, "{other:?}")?,
            }
        }
        write!(formatter, "}}")
    }
}

static GLOBAL_DICTIONARY: OnceLock<BlockStateDictionary> = OnceLock::new();
static AIR_RUNTIME_ID: OnceLock<u32> = OnceLock::new();

/// Match palette state values against query strings: strings compare verbatim, integers compare as decimal;
/// bytes accept boolean spellings (`true`/`false`) and numbers.
fn nbt_value_matches(value: Option<&NbtValue>, query: &str) -> bool {
    let Some(value) = value else {
        return false;
    };
    match value {
        NbtValue::String(s) => s == query,
        NbtValue::Int(i) => query.parse::<i32>().is_ok_and(|q| q == *i),
        NbtValue::Byte(b) => match query {
            "true" => *b != 0,
            "false" => *b == 0,
            _ => query.parse::<i32>().is_ok_and(|q| q == *b as i32),
        },
        _ => false,
    }
}

/// Hash network id of `minecraft:air` (cached per process, computed once).
pub fn air_runtime_id() -> u32 {
    *AIR_RUNTIME_ID.get_or_init(|| block_state_hash("minecraft:air", None))
}

/// Converts an internal state hash to the version-pack runtime id used by
/// legacy/non-hashed packet paths.
///
/// Protocol 2168 hashed block packets do not call this function: they carry
/// the FNV1a state hash itself. The two id spaces remain deliberately
/// separate for item/legacy packets that still require the palette mapping.
pub fn network_runtime_id(hash: u32) -> Option<u32> {
    BlockStateDictionary::global().network_id(hash)
}

pub struct BlockStateDictionary {
    entries: RwLock<HashMap<u32, Arc<BlockStateEntry>, RandomState>>,
    network_ids: RwLock<HashMap<u32, u32, RandomState>>,
    /// identifier to first-seen hash. Before defaults land, name-based lookups (e.g. /setblock)
    /// use the first-seen state of a block as its default.
    by_name: RwLock<HashMap<String, u32, RandomState>>,
}

impl BlockStateDictionary {
    fn new_seeded() -> Self {
        let dictionary = Self {
            entries: RwLock::new(HashMap::with_hasher(RandomState::new())),
            network_ids: RwLock::new(HashMap::with_hasher(RandomState::new())),
            by_name: RwLock::new(HashMap::with_hasher(RandomState::new())),
        };
        // Only pre-register internal sentinels. Network runtime ids must come from the version palette,
        // because even the air-state id is version-dependent data.
        dictionary.record_with(air_runtime_id(), || BlockStateEntry {
            name: "minecraft:air".to_string(),
            states: None,
        });
        dictionary.record_with(UNKNOWN_BLOCK_RUNTIME_ID, || BlockStateEntry {
            name: "minecraft:unknown".to_string(),
            states: None,
        });
        dictionary
    }

    pub fn global() -> &'static BlockStateDictionary {
        GLOBAL_DICTIONARY.get_or_init(Self::new_seeded)
    }

    /// Record one hash. Hit path: one read lock + lookup, no `make_entry` call, zero allocation.
    /// Miss: double-checked insert under the write lock (concurrent first sights keep the first writer),
    /// and maintain the identifier-to-first-hash index.
    pub fn record_with(&self, hash: u32, make_entry: impl FnOnce() -> BlockStateEntry) {
        if let Ok(entries) = self.entries.read() {
            if entries.contains_key(&hash) {
                return;
            }
        }
        let Ok(mut entries) = self.entries.write() else {
            return;
        };
        if entries.contains_key(&hash) {
            return;
        }
        let entry = Arc::new(make_entry());
        if let Ok(mut by_name) = self.by_name.write() {
            by_name.entry(entry.name.clone()).or_insert(hash);
        }
        entries.insert(hash, entry);
    }

    pub fn get(&self, hash: u32) -> Option<Arc<BlockStateEntry>> {
        self.entries.read().ok()?.get(&hash).cloned()
    }

    /// Returns the protocol runtime id from the loaded version-pack palette.
    pub fn network_id(&self, hash: u32) -> Option<u32> {
        self.network_ids.read().ok()?.get(&hash).copied()
    }

    pub fn record_with_network_id(
        &self,
        hash: u32,
        network_id: u32,
        make_entry: impl FnOnce() -> BlockStateEntry,
    ) {
        self.record_with(hash, make_entry);
        if let Ok(mut network_ids) = self.network_ids.write() {
            network_ids.entry(hash).or_insert(network_id);
        }
    }

    /// identifier to the first-seen state hash of the block (the bootstrap default).
    pub fn first_hash_of(&self, name: &str) -> Option<u32> {
        self.by_name.read().ok()?.get(name).copied()
    }

    /// identifier plus state key-value pairs to state hash.
    ///
    /// Consumer: world generation (multi-state blocks such as three-axis logs with `pillar_axis`
    /// and vines with `vine_direction_bits`). Single full-table scan, suited for one-time
    /// resolution at build time (hot paths should cache the result).
    pub fn find_hash_by_state(
        &self,
        name: &str,
        state_key: &str,
        state_value: &str,
    ) -> Option<u32> {
        self.find_hash_by_states(name, &[(state_key, state_value)])
    }

    /// identifier plus multiple state key-value pairs (all must match) to state hash.
    ///
    /// Value matching covers the three palette NBT storages: strings (`pillar_axis="x"`),
    /// integers (`vine_direction_bits=8`, `age=2`), and byte bools
    /// (`tip=true`). Ties take the smallest hash for determinism.
    pub fn find_hash_by_states(&self, name: &str, pairs: &[(&str, &str)]) -> Option<u32> {
        let entries = self.entries.read().ok()?;
        let mut best: Option<u32> = None;
        for (hash, entry) in entries.iter() {
            if entry.name != name {
                continue;
            }
            let Some(states) = &entry.states else {
                continue;
            };
            if pairs
                .iter()
                .all(|(key, value)| nbt_value_matches(states.get(*key), value))
            {
                if best.is_none() || *hash < best.unwrap_or(0) {
                    best = Some(*hash);
                }
            }
        }
        best
    }

    /// All registered states of an identifier (debug/test use; order undefined).
    pub fn entries_for(&self, name: &str) -> Vec<(u32, Arc<BlockStateEntry>)> {
        let Ok(entries) = self.entries.read() else {
            return Vec::new();
        };
        let mut out: Vec<(u32, Arc<BlockStateEntry>)> = entries
            .iter()
            .filter(|(_, entry)| entry.name == name)
            .map(|(hash, entry)| (*hash, entry.clone()))
            .collect();
        out.sort_by_key(|(hash, _)| *hash);
        out
    }

    pub fn len(&self) -> usize {
        self.entries
            .read()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_dictionary_is_seeded_with_air_and_unknown() {
        let dictionary = BlockStateDictionary::global();
        assert_eq!(
            dictionary.get(air_runtime_id()).unwrap().name,
            "minecraft:air"
        );
        assert_eq!(
            dictionary.get(UNKNOWN_BLOCK_RUNTIME_ID).unwrap().name,
            "minecraft:unknown",
        );
    }

    #[test]
    fn record_with_is_first_writer_wins_and_hit_path_skips_construction() {
        let dictionary = BlockStateDictionary::new_seeded();
        let hash = block_state_hash("minecraft:stone", None);
        dictionary.record_with(hash, || BlockStateEntry {
            name: "minecraft:stone".to_string(),
            states: None,
        });
        // Re-recording the same hash must not invoke the constructor closure.
        dictionary.record_with(hash, || panic!("hit path must not construct"));
        assert_eq!(dictionary.get(hash).unwrap().name, "minecraft:stone");
    }

    #[test]
    fn display_formats_name_and_sorted_states() {
        let mut states = CompoundNbt::new(None);
        states.insert("open_bit", NbtValue::Byte(1));
        states.insert("direction", NbtValue::Int(2));
        let entry = BlockStateEntry {
            name: "minecraft:oak_door".to_string(),
            states: Some(states),
        };
        assert_eq!(
            entry.to_string(),
            "minecraft:oak_door{direction=2, open_bit=true}"
        );
    }
}
