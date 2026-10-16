//! Block snapshot types and hot-path read-only queries.
//!
//! Moved verbatim from `block_json.rs` (P4 split); re-exported from [`super`]
//! so existing paths keep compiling.

use super::*;
use std::collections::HashMap;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use sc_packloader::block::PropValue;
use crate::mining_drops::{DropCompiled, MiningCompiled};
use crate::workstation::{ProcessDecl, WorkstationDecl};

// ---------------------------------------------------------------------------
// Immutable registry snapshot (the sole atomically published product).
// ---------------------------------------------------------------------------

/// Compiled view of a block type (sorted by identifier in the snapshot).
#[derive(Clone, Debug)]
pub struct SnapType {
    pub identifier: Box<str>,
    pub file: Box<str>,
    pub type_id: u32,
    pub properties: Vec<sc_packloader::block::PropertyDef>,
    /// Dense index of the default state (explicitly declared via `sc:default_state`).
    pub default_state: u32,
    pub state_ids: Vec<u32>,
}

/// Compiled view of a single valid state (sorted by (identifier, typed key) in the snapshot;
/// the index is the new dense `BlockStateId`).
///
/// Static capabilities are not inlined in this struct; they compile into the snapshot's fixed dense
/// columns (below), addressed by index.
#[derive(Clone, Debug)]
pub struct SnapState {
    pub index: u32,
    pub type_id: u32,
    /// Canonical state key (`name=repr` comma-joined; `default` when property-less).
    pub key: Box<str>,
    pub hash: u32,
    /// Verbatim `sc:protocol_runtime_ids`, never recomputed by sort order.
    pub network_id: u32,
    /// Typed canonical state identity (sorted length-prefixed encoding).
    pub typed_key: Box<str>,
    /// Canonical state property assignments (ascending by name; equivalent to states NBT).
    pub props: Vec<(String, PropValue)>,
}

/// Immutable snapshot atomically published after the private builder passes all checks.
///
/// Callers share an `Arc` read-only reference without per-Region deep copies; hot paths only read
/// this snapshot and the dense tables derived from it.
#[derive(Clone, Debug, Default)]
pub struct BlockJsonSnapshot {
    pub schema_version: u32,
    pub network_id_mode: String,
    /// Content fingerprint (packloader computes it over sorted path+bytes+mode): any file change
    /// alters it, suitable for the generator descriptor.
    pub fingerprint: u64,
    pub types: Vec<SnapType>,
    pub states: Vec<SnapState>,
    pub by_hash: HashMap<u32, u32>,
    pub by_network_id: HashMap<u32, u32>,
    pub by_typed: HashMap<(Box<str>, Box<str>), u32>,
    pub defaults: HashMap<Box<str>, u32>,
    // ---- Fixed dense capability columns (fixed component set plus plugin verbatim columns; all indexed
    // by state, same length as `states`; hot paths only do array indexing) ----
    /// `minecraft:collision_box` boxes (each `[ox,oy,oz,sx,sy,sz]` in 16ths;
    /// `enabled:false` means empty list = no collision; None = undeclared).
    pub collision: Vec<Option<Vec<[f32; 6]>>>,
    /// `minecraft:destructible_by_mining` seconds (None = undeclared).
    pub mining_seconds: Vec<Option<f32>>,
    /// `sc:unbreakable` (mutually exclusive with destructible_by_mining; checked at parse time per layer plus post-merge).
    pub unbreakable: Vec<bool>,
    /// `minecraft:light_emission`(0..15).
    pub light_emission: Vec<Option<u8>>,
    /// `minecraft:light_dampening`(0..15).
    pub light_dampening: Vec<Option<u8>>,
    /// `minecraft:loot` table path (full table lives in a separate data domain, referenced here).
    pub loot: Vec<Option<Box<str>>>,
    /// `sc:replaceable`.
    pub replaceable: Vec<Option<bool>>,
    /// `sc:liquid` kind.
    pub liquid: Vec<Option<Box<str>>>,
    /// `sc:random_tick`.
    pub random_tick: Vec<Option<bool>>,
    /// `sc:needs_support` condition.
    pub needs_support: Vec<Option<Box<str>>>,
    /// `sc:can_contain_liquid`.
    pub can_contain_liquid: Vec<Option<bool>>,
    /// Compiled `sc:mining` profile (`None` means no mining for this state, legacy path applies;
    /// densely addressed by `BlockStateId`, same length as `states`).
    pub mining: Vec<Option<MiningCompiled>>,
    /// Compiled `sc:drops` profile (`None` means no drops, nothing guessed;
    /// `Some(enabled=false)` means explicitly drop-free; same length as `states`).
    pub drops: Vec<Option<DropCompiled>>,
    /// Compiled `sc:workstation` declaration (`None` means no workstation
    /// window; same length as `states`).
    pub workstation: Vec<Option<WorkstationDecl>>,
    /// Compiled `sc:process` declaration (`None` means no duration process;
    /// same length as `states`).
    pub process: Vec<Option<ProcessDecl>>,
    /// Non-fixed component verbatim values (name to value; schema validated by plugin registration, cold path).
    pub plugin_components: Vec<Vec<(Box<str>, serde_json::Value)>>,
}


// ---------------------------------------------------------------------------
// Snapshot queries (hot-path read-only, no JSON).
// ---------------------------------------------------------------------------

impl BlockJsonSnapshot {
    pub fn state_count(&self) -> usize {
        self.states.len()
    }

    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    pub fn state(&self, index: u32) -> Option<&SnapState> {
        self.states.get(index as usize)
    }

    pub fn block_type(&self, type_id: u32) -> Option<&SnapType> {
        self.types.get(type_id as usize)
    }

    pub fn type_by_identifier(&self, identifier: &str) -> Option<&SnapType> {
        self.types
            .iter()
            .find(|t| t.identifier.as_ref() == identifier)
    }

    /// FNV state hash of the block default (for worldgen/item mapping; never guessed: missing means None).
    pub fn default_hash(&self, identifier: &str) -> Option<u32> {
        let idx = self.defaults.get(identifier)?;
        self.states.get(*idx as usize).map(|s| s.hash)
    }

    pub fn default_state_idx(&self, identifier: &str) -> Option<u32> {
        self.defaults.get(identifier).copied()
    }

    pub fn state_idx_by_hash(&self, hash: u32) -> Option<u32> {
        self.by_hash.get(&hash).copied()
    }

    pub fn state_idx_by_network_id(&self, network_id: u32) -> Option<u32> {
        self.by_network_id.get(&network_id).copied()
    }

    /// identifier plus complete property assignments to dense index (typed key, byte/int never confused).
    pub fn typed_state_of(&self, identifier: &str, props: &[(String, PropValue)]) -> Option<u32> {
        let key = typed_state_key(identifier, props);
        self.by_typed.get(&(Box::from(identifier), key)).copied()
    }

    /// identifier plus lenient string property assignments to state hash
    /// (minimum hash wins, matching the global dictionary scan).
    ///
    /// String matching mirrors the dictionary: exact string equality, int
    /// parse, byte `true`/`false`/int parse. Worldgen table builders prefer
    /// this over the global dictionary: dlopened plugin copies may hold an
    /// empty dictionary while the snapshot is always at hand there.
    pub fn find_state_hash(&self, identifier: &str, pairs: &[(&str, &str)]) -> Option<u32> {
        let ty = self.type_by_identifier(identifier)?;
        let mut best: Option<u32> = None;
        for &idx in &ty.state_ids {
            let Some(st) = self.states.get(idx as usize) else {
                continue;
            };
            if !snapshot_props_match(&st.props, pairs) {
                continue;
            }
            if best.is_none_or(|b| st.hash < b) {
                best = Some(st.hash);
            }
        }
        best
    }

    /// All state hashes of one identifier (empty when unknown).
    pub fn state_hashes_of(&self, identifier: &str) -> Vec<u32> {
        let Some(ty) = self.type_by_identifier(identifier) else {
            return Vec::new();
        };
        ty.state_ids
            .iter()
            .filter_map(|&idx| self.states.get(idx as usize).map(|st| st.hash))
            .collect()
    }

    /// Changes one property of a state to the target index (O(1) typed lookup).
    pub fn with_property_typed(&self, index: u32, name: &str, value: PropValue) -> Option<u32> {
        let state = self.state(index)?;
        let type_view = self.block_type(state.type_id)?;
        let identifier = type_view.identifier.clone();
        let mut props = state.props.clone();
        let mut replaced = false;
        for (k, v) in props.iter_mut() {
            if k == name {
                *v = value.clone();
                replaced = true;
            }
        }
        if !replaced {
            return None;
        }
        self.typed_state_of(&identifier, &props)
    }

    /// Per-state capability bits (for `BlockComponentFlags` builds).
    ///
    /// The map compiles explicit declarations without guessing: transparency/solidity have no data source
    /// and stay unset; other bits are set only when their component is explicitly declared.
    pub fn flag_bits(&self, index: u32) -> u64 {
        use crate::state::flags;
        let Some(state) = self.state(index) else {
            return 0;
        };
        let Some(type_view) = self.block_type(state.type_id) else {
            return 0;
        };
        let idx = index as usize;
        let mut bits = 0u64;
        if type_view.identifier.as_ref() == "minecraft:air" {
            bits |= flags::IS_AIR;
        }
        if type_view.identifier.as_ref() == "minecraft:unknown" {
            bits |= flags::IS_UNKNOWN;
        }
        if !state.props.is_empty() {
            bits |= flags::HAS_STATES;
        }
        if self.replaceable.get(idx).copied().flatten() == Some(true) {
            bits |= flags::REPLACEABLE;
        }
        if self.liquid.get(idx).and_then(Option::as_ref).is_some() {
            bits |= flags::LIQUID;
        }
        if self.random_tick.get(idx).copied().flatten() == Some(true) {
            bits |= flags::RANDOM_TICK;
        }
        if self
            .needs_support
            .get(idx)
            .and_then(Option::as_ref)
            .is_some()
        {
            bits |= flags::NEEDS_SUPPORT;
        }
        if self.can_contain_liquid.get(idx).copied().flatten() == Some(true) {
            bits |= flags::CAN_CONTAIN_LIQUID;
        }
        if self.unbreakable.get(idx).copied().unwrap_or(false) {
            bits |= flags::UNBREAKABLE;
        }
        if self
            .collision
            .get(idx)
            .is_some_and(|c| c.as_ref().is_some_and(Vec::is_empty))
        {
            bits |= flags::NO_COLLISION;
        }
        bits
    }

    /// Typed column accessors (fixed dense columns; array indexing on hot and cold paths, no deserialization).
    pub fn collision_of(&self, index: u32) -> Option<&Vec<[f32; 6]>> {
        self.collision.get(index as usize)?.as_ref()
    }

    /// Mine seconds (`minecraft:destructible_by_mining`; None = undeclared).
    pub fn mining_seconds_of(&self, index: u32) -> Option<f32> {
        self.mining_seconds.get(index as usize).copied().flatten()
    }

    pub fn is_unbreakable(&self, index: u32) -> bool {
        self.unbreakable
            .get(index as usize)
            .copied()
            .unwrap_or(false)
    }

    pub fn light_emission_of(&self, index: u32) -> Option<u8> {
        self.light_emission.get(index as usize).copied().flatten()
    }

    pub fn light_dampening_of(&self, index: u32) -> Option<u8> {
        self.light_dampening.get(index as usize).copied().flatten()
    }

    pub fn loot_of(&self, index: u32) -> Option<&str> {
        self.loot.get(index as usize)?.as_deref()
    }

    pub fn replaceable_of(&self, index: u32) -> Option<bool> {
        self.replaceable.get(index as usize).copied().flatten()
    }

    pub fn liquid_of(&self, index: u32) -> Option<&str> {
        self.liquid.get(index as usize)?.as_deref()
    }

    pub fn random_tick_of(&self, index: u32) -> Option<bool> {
        self.random_tick.get(index as usize).copied().flatten()
    }

    pub fn needs_support_of(&self, index: u32) -> Option<&str> {
        self.needs_support.get(index as usize)?.as_deref()
    }

    pub fn can_contain_liquid_of(&self, index: u32) -> Option<bool> {
        self.can_contain_liquid
            .get(index as usize)
            .copied()
            .flatten()
    }

    /// Compiled `sc:mining` profile (hot-path read-only; `None` means no mining, legacy path applies).
    pub fn mining_of(&self, index: u32) -> Option<&MiningCompiled> {
        self.mining.get(index as usize)?.as_ref()
    }

    /// Compiled `sc:drops` profile (hot-path read-only; `None` means no drops, nothing guessed).
    pub fn drops_of(&self, index: u32) -> Option<&DropCompiled> {
        self.drops.get(index as usize)?.as_ref()
    }

    /// Compiled `sc:workstation` declaration (hot-path read-only).
    pub fn workstation_of(&self, index: u32) -> Option<&WorkstationDecl> {
        self.workstation.get(index as usize)?.as_ref()
    }

    /// Compiled `sc:process` declaration (hot-path read-only).
    pub fn process_of(&self, index: u32) -> Option<&ProcessDecl> {
        self.process.get(index as usize)?.as_ref()
    }

    /// Plugin component verbatim values (cold path; schema validated by plugin registration).
    pub fn plugin_component(&self, index: u32, name: &str) -> Option<&serde_json::Value> {
        self.plugin_components
            .get(index as usize)?
            .iter()
            .find(|(k, _)| k.as_ref() == name)
            .map(|(_, v)| v)
    }

    /// Derives legacy `block_palette.nbt` bytes from the snapshot (compat-period derived adapter).
    ///
    /// For callers that still consume palette bytes (independent dictionary copies of dynamic plugins,
    /// legacy test paths); contents match the snapshot. Disk version/LevelDB key/wire formats are unaffected.
    pub fn encode_legacy_palette_bytes(&self) -> Result<Vec<u8>, String> {
        use sc_binary::ByteWriter;
        use sc_nbt::local::JavaLocalNbt;
        use sc_nbt::writer::NbtWriter;
        let mut blocks = Vec::with_capacity(self.states.len());
        for state in self.states.iter() {
            let type_view = self.block_type(state.type_id).ok_or_else(|| {
                format!("快照损坏：状态 {} 无类型 {}", state.index, state.type_id)
            })?;
            let mut entry = CompoundNbt::new(None);
            entry.insert("name", NbtValue::String(type_view.identifier.to_string()));
            entry.insert("states", NbtValue::Compound(states_nbt_of(&state.props)));
            if state.network_id > i32::MAX as u32 {
                return Err(format!(
                    "protocol_runtime_id {} 超出 i32（状态 {:?}）",
                    state.network_id, state.key
                ));
            }
            entry.insert(
                "protocol_runtime_id",
                NbtValue::Int(state.network_id as i32),
            );
            blocks.push(NbtValue::Compound(entry));
        }
        let mut root = CompoundNbt::new(Some(String::new()));
        root.insert("blocks", NbtValue::List(blocks));
        let mut writer = ByteWriter::new();
        NbtWriter::from_writer(&mut writer)
            .write::<JavaLocalNbt>(&NbtValue::Compound(root))
            .map_err(|e| format!("派生 palette 编码失败: {e}"))?;
        Ok(writer.as_slice().to_vec())
    }
}