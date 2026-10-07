//! Block-state registry.
//!
//! Dual-ID mode: after palette load each state gets a dense ordinal index
//! [`BlockStateId`] (logic hot-path tables index by it) while the FNV1a-32 hash
//! [`BlockRuntimeId`] keeps covering storage and network. The global bootstrap dictionary
//! (`sc_world::block_dictionary`) stays for cross-checks and superset fallback:
//! states from old saves or custom blocks (hashes outside the palette) still reverse-lookup losslessly.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use sc_binary::ByteReader;
use sc_ecs::resource::Resource;
use sc_nbt::local::JavaLocalNbt;
use sc_nbt::reader::NbtReader;
use sc_nbt::NbtValue;
use sc_world::block_dictionary::{air_runtime_id, BlockStateDictionary, BlockStateEntry};
use sc_world::chunk::BlockRuntimeId;
use sc_world::leveldb::block_hash::{block_state_hash, UNKNOWN_BLOCK_RUNTIME_ID};

use crate::state::{
    property_to_nbt, state_signature, BlockPropertyValue, BlockState, BlockStateId,
};

#[derive(Resource, Default)]
pub struct BlockStateRegistry {
    /// Dense state table indexed by [`BlockStateId`] (palette order, frozen after SCLoad).
    states: RwLock<Vec<BlockState>>,
    /// State hash to dense id.
    by_hash: RwLock<HashMap<u32, BlockStateId>>,
    /// Palette protocol network runtime id to dense id (legacy/item packet paths need the palette map).
    by_network_id: RwLock<HashMap<u32, BlockStateId>>,
    /// identifier to (states signature to dense id), the O(1) basis for `state_of`/`with_property`.
    by_name_sig: RwLock<HashMap<String, HashMap<Box<str>, BlockStateId>>>,
    air: OnceLock<BlockStateId>,
    unknown: OnceLock<BlockStateId>,
}

impl Clone for BlockStateRegistry {
    fn clone(&self) -> Self {
        Self {
            states: RwLock::new(self.states.read().expect("lock poisoned").clone()),
            by_hash: RwLock::new(self.by_hash.read().expect("lock poisoned").clone()),
            by_network_id: RwLock::new(self.by_network_id.read().expect("lock poisoned").clone()),
            by_name_sig: RwLock::new(self.by_name_sig.read().expect("lock poisoned").clone()),
            air: OnceLock::new(),
            unknown: OnceLock::new(),
        }
    }
}

impl BlockStateRegistry {
    pub fn new() -> Self {
        // Triggers global dictionary init (air/unknown sentinels placed),
        // so the first chunk-parse thread avoids paying init cost.
        let _ = BlockStateDictionary::global();
        Self::default()
    }

    /// Hash to known state description. Prefers the dense table; hashes outside the palette
    /// fall back to the global bootstrap dictionary (bootstrap guarantee: once seen in a parsed palette, it is known).
    pub fn describe(&self, runtime_id: BlockRuntimeId) -> Option<Arc<BlockStateEntry>> {
        if let Some(state_id) = self.by_runtime_id(runtime_id) {
            if let Some(state) = self.state(state_id) {
                return Some(Arc::new(BlockStateEntry {
                    name: state.name.to_string(),
                    states: state.states.clone(),
                }));
            }
        }
        BlockStateDictionary::global().get(runtime_id.0)
    }

    pub fn air(&self) -> BlockRuntimeId {
        BlockRuntimeId(air_runtime_id())
    }

    pub fn is_air(&self, runtime_id: BlockRuntimeId) -> bool {
        runtime_id.0 == air_runtime_id()
    }

    /// Registered state count (for debug/acceptance observability).
    pub fn known_states(&self) -> usize {
        self.state_count()
    }

    /// Dense state table size (= max BlockStateId + 1).
    pub fn state_count(&self) -> usize {
        self.states.read().map(|states| states.len()).unwrap_or(0)
    }

    /// Dense id to state (cloned; for debug/build-time use).
    pub fn state(&self, state_id: BlockStateId) -> Option<BlockState> {
        self.states.read().ok()?.get(state_id.0 as usize).cloned()
    }

    /// Dense state table snapshot (one-shot at startup build time, e.g. capability-mask build).
    pub fn states_snapshot(&self) -> Vec<BlockState> {
        self.states
            .read()
            .map(|states| states.clone())
            .unwrap_or_default()
    }

    /// Hash to dense id.
    pub fn by_runtime_id(&self, runtime_id: BlockRuntimeId) -> Option<BlockStateId> {
        self.by_hash.read().ok()?.get(&runtime_id.0).copied()
    }

    /// Dense id to hash (network/storage id).
    pub fn runtime_id(&self, state_id: BlockStateId) -> Option<BlockRuntimeId> {
        self.state(state_id).map(|state| BlockRuntimeId(state.hash))
    }

    /// Palette protocol network runtime id to dense id.
    pub fn by_network_runtime_id(&self, network_id: u32) -> Option<BlockStateId> {
        self.by_network_id.read().ok()?.get(&network_id).copied()
    }

    pub fn air_state_id(&self) -> Option<BlockStateId> {
        self.air.get().copied()
    }

    pub fn unknown_state_id(&self) -> Option<BlockStateId> {
        self.unknown.get().copied()
    }

    /// identifier plus complete states key-values to dense id (O(1) signature lookup).
    ///
    /// Consumers: multi-state block construction such as worldgen (three-axis logs, vine directions);
    /// resolve once at construction, cache the result on runtime hot paths.
    pub fn state_of(
        &self,
        identifier: &str,
        props: &[(&str, BlockPropertyValue)],
    ) -> Option<BlockStateId> {
        let mut states = sc_nbt::compound::CompoundNbt::new(None);
        for (key, value) in props {
            states.insert(*key, property_to_nbt(value));
        }
        let sig = state_signature(&Some(states));
        self.by_name_sig
            .read()
            .ok()?
            .get(identifier)?
            .get(&sig)
            .copied()
    }

    /// Changes one property of a state to the target dense id (O(1) signature lookup).
    ///
    /// Usable before mixed-radix index math: makes no "types are contiguous" assumption and keys
    /// directly on the states signature, so any vanilla multi-state block hits.
    pub fn with_property(
        &self,
        state_id: BlockStateId,
        key: &str,
        value: BlockPropertyValue,
    ) -> Option<BlockStateId> {
        let current = self.state(state_id)?;
        let mut states = current
            .states
            .clone()
            .unwrap_or_else(|| sc_nbt::compound::CompoundNbt::new(None));
        states.insert(key, property_to_nbt(&value));
        let sig = state_signature(&Some(states));
        self.by_name_sig
            .read()
            .ok()?
            .get(current.name.as_ref())?
            .get(&sig)
            .copied()
    }

    /// Registers all block states plus dense ids and the two-way maps from version-pack palette bytes.
    ///
    /// Input is JavaLocalNbt (converted `runtime_block_states.dat`), with a root
    /// `{ blocks: List<{name, states, version, protocol_runtime_id}> }`.
    /// `protocol_runtime_id` is the current protocol palette's sequential runtime id.
    pub fn load_palette_from_bytes(&self, bytes: &[u8]) -> Result<usize, String> {
        let mut reader = ByteReader::from(bytes.to_vec());
        let root = NbtReader::from_reader(&mut reader)
            .read::<JavaLocalNbt>()
            .map_err(|e| format!("palette NBT 读取失败: {e}"))?;
        let NbtValue::Compound(root) = &root else {
            return Err("palette 根不是 Compound".to_string());
        };
        let Some(NbtValue::List(blocks)) = root.get("blocks") else {
            return Err("palette 缺少 blocks 列表".to_string());
        };
        let dictionary = BlockStateDictionary::global();
        let mut runtime_to_hash = HashMap::with_capacity(blocks.len());
        let mut count = 0usize;
        let mut air_has_network_id = false;

        let mut states = self
            .states
            .write()
            .expect("BlockStateRegistry lock poisoned");
        let mut by_hash = self
            .by_hash
            .write()
            .expect("BlockStateRegistry lock poisoned");
        let mut by_network_id = self
            .by_network_id
            .write()
            .expect("BlockStateRegistry lock poisoned");
        let mut by_name_sig = self
            .by_name_sig
            .write()
            .expect("BlockStateRegistry lock poisoned");

        for block in blocks {
            let NbtValue::Compound(entry) = block else {
                continue;
            };
            let Some(name) = entry.get("name").and_then(NbtValue::as_string) else {
                continue;
            };
            let states_nbt = entry.get("states").and_then(NbtValue::as_compound).cloned();
            let hash = block_state_hash(name, states_nbt.as_ref());
            let network_id = entry
                .get("protocol_runtime_id")
                .or_else(|| entry.get("runtime_id"))
                .or_else(|| entry.get("runtimeId"))
                .and_then(NbtValue::as_i32)
                .ok_or_else(|| format!("方块状态缺少 protocol_runtime_id: {name}"))?;
            if network_id < 0 {
                return Err(format!(
                    "方块状态 protocol_runtime_id 必须为非负数: {name}={network_id}"
                ));
            }
            let network_id = network_id as u32;
            if let Some(previous_hash) = runtime_to_hash.insert(network_id, hash) {
                if previous_hash != hash {
                    return Err(format!(
                        "duplicate runtime id {network_id}: {previous_hash:#x} vs {hash:#x}"
                    ));
                }
            }

            // Dense id assignment: matches palette order (contiguous 0..N).
            let state_id = BlockStateId(states.len() as u32);
            by_hash.insert(hash, state_id);
            by_network_id.insert(network_id, state_id);
            let sig = state_signature(&states_nbt);
            by_name_sig
                .entry(name.clone())
                .or_default()
                .insert(sig, state_id);
            states.push(BlockState {
                hash,
                name: Arc::from(name.as_str()),
                states: states_nbt.clone(),
                network_id,
            });

            if hash == air_runtime_id() {
                let _ = self.air.set(state_id);
                air_has_network_id = true;
            }
            if hash == UNKNOWN_BLOCK_RUNTIME_ID {
                let _ = self.unknown.set(state_id);
            }

            // Still registers into the global dictionary: cross-check plus superset fallback (old saves/custom blocks round-trip).
            dictionary.record_with_network_id(hash, network_id, || BlockStateEntry {
                name: name.clone(),
                states: states_nbt.clone(),
            });
            count += 1;
        }
        if !air_has_network_id {
            return Err("palette is missing minecraft:air protocol_runtime_id".to_string());
        }
        Ok(count)
    }

    /// Builds a fresh registry from `.block.json` snapshots (for atomic publish).
    ///
    /// Callers replace the old resource wholesale with the return value (`*registry = ...`) instead of
    /// patching the old table; a failed bundle never reaches here (no partial publish).
    /// Dense indices follow snapshot order (type identifiers, states in canonical per-type key order), which differs
    /// from the legacy palette order; persistence/network use only hashes and sequential ids, never dense indices.
    pub fn from_block_snapshot(snapshot: &crate::block_json::BlockJsonSnapshot) -> Self {
        let registry = Self::default();
        {
            let mut states = registry
                .states
                .write()
                .expect("BlockStateRegistry lock poisoned");
            let mut by_hash = registry
                .by_hash
                .write()
                .expect("BlockStateRegistry lock poisoned");
            let mut by_network_id = registry
                .by_network_id
                .write()
                .expect("BlockStateRegistry lock poisoned");
            let mut by_name_sig = registry
                .by_name_sig
                .write()
                .expect("BlockStateRegistry lock poisoned");
            for snap_state in snapshot.states.iter() {
                let Some(type_view) = snapshot.block_type(snap_state.type_id) else {
                    continue;
                };
                let state_id = BlockStateId(states.len() as u32);
                debug_assert_eq!(state_id.0, snap_state.index);
                by_hash.insert(snap_state.hash, state_id);
                by_network_id.insert(snap_state.network_id, state_id);
                let states_nbt = crate::block_json::states_nbt_of(&snap_state.props);
                let sig = state_signature(&Some(states_nbt.clone()));
                by_name_sig
                    .entry(type_view.identifier.to_string())
                    .or_default()
                    .insert(sig, state_id);
                states.push(BlockState {
                    hash: snap_state.hash,
                    name: Arc::from(type_view.identifier.as_ref()),
                    states: Some(states_nbt),
                    network_id: snap_state.network_id,
                });
                if type_view.identifier.as_ref() == "minecraft:air" {
                    let _ = registry.air.set(state_id);
                }
                if type_view.identifier.as_ref() == "minecraft:unknown" {
                    let _ = registry.unknown.set(state_id);
                }
            }
        }
        // Snapshot states sync into the global dictionary (cross-check plus superset fallback), same as the legacy load path.
        crate::block_json::publish_legacy_support(snapshot);
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_describes_seeded_air() {
        let registry = BlockStateRegistry::new();
        let air = registry.air();
        assert!(registry.is_air(air));
        assert_eq!(registry.describe(air).unwrap().name, "minecraft:air");
    }

    #[test]
    fn unknown_hash_is_none() {
        let registry = BlockStateRegistry::new();
        assert!(registry.describe(BlockRuntimeId(0xDEAD_BEEF)).is_none());
    }

    /// Real fixture (returns None when `diagnostics/sc-2168/block_palette.nbt` is absent from the repo;
    /// callers skip; missing fixture is unrelated to the logic under test).
    fn load_real_palette() -> Option<(BlockStateRegistry, usize)> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../diagnostics/sc-2168/block_palette.nbt"
        );
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => {
                eprintln!("[skip] 真实 palette fixture 缺失，跳过");
                return None;
            }
        };
        let registry = BlockStateRegistry::new();
        let count = registry
            .load_palette_from_bytes(&bytes)
            .expect("palette 解析");
        Some((registry, count))
    }

    #[test]
    fn palette_load_builds_dense_dual_id_mapping() {
        let Some((registry, count)) = load_real_palette() else {
            return;
        };
        assert!(count > 1000, "palette 状态数应远大于 1000: {count}");
        assert_eq!(registry.state_count(), count);
        // air dense id placed, mappable back to the hash both ways.
        let air = registry.air_state_id().expect("air 稠密 id");
        let air_hash = registry.runtime_id(air).expect("air 哈希");
        assert!(registry.is_air(air_hash));
        // Protocol sentinel unknown also gets a dense id.
        assert!(registry.unknown_state_id().is_some());
    }

    #[test]
    fn state_of_and_with_property_are_o1_signature_lookups() {
        let Some((registry, _)) = load_real_palette() else {
            return;
        };
        // Three-axis log state: oak_log pillar_axis=y to x is an O(1) transform.
        let log_y = registry
            .state_of(
                "minecraft:oak_log",
                &[("pillar_axis", BlockPropertyValue::Enum(Arc::from("y")))],
            )
            .expect("oak_log[y] 应在 palette 中");
        let log_x = registry.with_property(
            log_y,
            "pillar_axis",
            BlockPropertyValue::Enum(Arc::from("x")),
        );
        let expected_x = registry.state_of(
            "minecraft:oak_log",
            &[("pillar_axis", BlockPropertyValue::Enum(Arc::from("x")))],
        );
        assert_eq!(log_x, expected_x);
        assert_ne!(log_x, Some(log_y));
        // Unknown property values return None (no panic).
        assert_eq!(
            registry.with_property(
                log_y,
                "pillar_axis",
                BlockPropertyValue::Enum(Arc::from("w")),
            ),
            None
        );
    }

    #[test]
    fn component_flags_builds_dense_table_with_sentinels() {
        let Some((registry, count)) = load_real_palette() else {
            return;
        };
        let mut flags = crate::state::BlockComponentFlags::new();
        flags.build(&registry);
        assert_eq!(flags.len(), count);
        let air = registry.air_state_id().unwrap();
        assert!(flags.has(air, crate::state::flags::IS_AIR));
        assert!(flags.has(air, crate::state::flags::SOLID) == false);
        // Multi-state block (log) with non-empty states sets the HAS_STATES bit.
        let log = registry
            .state_of(
                "minecraft:oak_log",
                &[("pillar_axis", BlockPropertyValue::Enum(Arc::from("y")))],
            )
            .expect("oak_log[y]");
        assert!(flags.has(log, crate::state::flags::HAS_STATES));
        // Air has no states.
        assert!(!flags.has(air, crate::state::flags::HAS_STATES));
    }

    /// Actual property-name dump for tree-generation multi-state blocks in the 1.26.40 palette
    /// (one-off survey, doubles as a regression test for TreeBlockTable extension queries).
    #[test]
    fn dump_tree_multi_state_blocks_from_palette() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../diagnostics/sc-2168/block_palette.nbt"
        );
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("[dump] palette 文件缺失，跳过");
            return;
        };
        let registry = BlockStateRegistry::new();
        let count = registry
            .load_palette_from_bytes(&bytes)
            .expect("palette 解析");
        eprintln!("[dump] 已登记 {count} 个状态");
        let dictionary = BlockStateDictionary::global();
        for name in [
            "minecraft:cocoa",
            "minecraft:vine",
            "minecraft:pale_hanging_moss",
            "minecraft:mangrove_propagule",
            "minecraft:creaking_heart",
            "minecraft:muddy_mangrove_roots",
            "minecraft:moss_carpet",
            "minecraft:water",
            "minecraft:mud",
            "minecraft:farmland",
            "minecraft:mangrove_roots",
            "minecraft:podzol",
        ] {
            let entries = dictionary.entries_for(name);
            eprintln!("[dump] {name}: {} 个状态", entries.len());
            for (hash, entry) in entries.iter().take(20) {
                eprintln!("[dump]   {hash:#010x} {entry}");
            }
        }
    }
}
