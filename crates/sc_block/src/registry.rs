//! 方块状态注册表。
//!
//! Block-state registry.
//!
//! 双 ID 模式：调色板加载后，每个状态获得稠密序号下标 [`BlockStateId`]
//! （逻辑热路径表以它为下标），同时 FNV1a-32 哈希 [`BlockRuntimeId`] 继续覆盖存储与网络。
//! 全局引导字典（`sc_world::block_dictionary`）作为超集保留：
//! 旧存档或自定义方块（调色板之外的哈希）仍能无损反查。
//!
//! Dual-ID mode: after palette load each state gets a dense ordinal index
//! [`BlockStateId`] (logic hot-path tables index by it) while the FNV1a-32 hash
//! [`BlockRuntimeId`] keeps covering storage and network. The global bootstrap dictionary
//! (`sc_world::block_dictionary`) is kept as a superset:
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

/// 方块状态注册表：稠密状态表 + 哈希/network id 双向映射 + 签名索引。
///
/// Block state registry: the dense state table plus the two-way hash/network-id
/// maps and the signature index.
#[derive(Resource, Default)]
pub struct BlockStateRegistry {
    /// 以 [`BlockStateId`] 为下标的稠密状态表（调色板顺序，加载后冻结）。
    ///
    /// Dense state table indexed by [`BlockStateId`] (palette order, frozen after load).
    states: RwLock<Vec<BlockState>>,
    /// 状态哈希到稠密 id。
    ///
    /// State hash to dense id.
    by_hash: RwLock<HashMap<u32, BlockStateId>>,
    /// 调色板协议 network id 到稠密 id（旧包/物品包路径需要调色板映射）。
    ///
    /// Palette protocol network runtime id to dense id (legacy/item packet paths need
    /// the palette map).
    by_network_id: RwLock<HashMap<u32, BlockStateId>>,
    /// 标识符到（states 签名到稠密 id），`state_of`/`with_property` 的 O(1) 基础。
    ///
    /// identifier to (states signature to dense id), the O(1) basis for
    /// `state_of`/`with_property`.
    by_name_sig: RwLock<HashMap<String, HashMap<Box<str>, BlockStateId>>>,
    /// 空气哨兵的稠密 id（加载后一次性写入）。
    ///
    /// Dense id of the air sentinel (written once at load).
    air: OnceLock<BlockStateId>,
    /// 未知方块哨兵的稠密 id（加载后一次性写入）。
    ///
    /// Dense id of the unknown-block sentinel (written once at load).
    unknown: OnceLock<BlockStateId>,
}

impl Clone for BlockStateRegistry {
    /// 克隆注册表（复制全部表内容，哨兵 `OnceLock` 不复制）。
    ///
    /// Clones the registry (copies all table contents; sentinel `OnceLock`s are not
    /// carried over).
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
    /// 创建空注册表，并顺带触发全局字典初始化。
    ///
    /// Creates an empty registry and incidentally triggers global dictionary init.
    ///
    /// 全局字典初始化会放置 air/unknown 哨兵，避免首个区块解析线程承担初始化开销。
    ///
    /// Global dictionary init places the air/unknown sentinels, so the first
    /// chunk-parse thread avoids paying init cost.
    pub fn new() -> Self {
        // Triggers global dictionary init (air/unknown sentinels placed),
        // so the first chunk-parse thread avoids paying init cost.
        let _ = BlockStateDictionary::global();
        Self::default()
    }

    /// 哈希到已知状态描述。优先查稠密表；调色板之外的哈希回退到全局引导字典
    /// （引导保证：解析过的调色板里出现过，即视为已知）。
    ///
    /// Hash to known state description. Prefers the dense table; hashes outside the
    /// palette fall back to the global bootstrap dictionary (bootstrap guarantee: once
    /// seen in a parsed palette, it is known).
    ///
    /// - `runtime_id`: 状态哈希
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

    /// 返回空气方块的哈希。
    ///
    /// Returns the air block hash.
    pub fn air(&self) -> BlockRuntimeId {
        BlockRuntimeId(air_runtime_id())
    }

    /// 判断给定哈希是否为空气。
    ///
    /// Whether the given hash is air.
    pub fn is_air(&self, runtime_id: BlockRuntimeId) -> bool {
        runtime_id.0 == air_runtime_id()
    }

    /// 已注册状态数。
    ///
    /// Registered state count.
    pub fn known_states(&self) -> usize {
        self.state_count()
    }

    /// 稠密状态表大小（= 最大 BlockStateId + 1）。
    ///
    /// Dense state table size (= max BlockStateId + 1).
    pub fn state_count(&self) -> usize {
        self.states.read().map(|states| states.len()).unwrap_or(0)
    }

    /// 稠密 id 到状态（克隆返回；供调试/构建期使用）。
    ///
    /// Dense id to state (cloned; for debug/build-time use).
    ///
    /// - `state_id`: 目标稠密 id
    pub fn state(&self, state_id: BlockStateId) -> Option<BlockState> {
        self.states.read().ok()?.get(state_id.0 as usize).cloned()
    }

    /// 稠密状态表快照（启动构建期一次性获取，如能力掩码构建）。
    ///
    /// Dense state table snapshot (one-shot at startup build time, e.g. capability-mask
    /// build).
    pub fn states_snapshot(&self) -> Vec<BlockState> {
        self.states
            .read()
            .map(|states| states.clone())
            .unwrap_or_default()
    }

    /// 哈希到稠密 id。
    ///
    /// Hash to dense id.
    ///
    /// - `runtime_id`: 状态哈希
    pub fn by_runtime_id(&self, runtime_id: BlockRuntimeId) -> Option<BlockStateId> {
        self.by_hash.read().ok()?.get(&runtime_id.0).copied()
    }

    /// 稠密 id 到哈希（网络/存储 id）。
    ///
    /// Dense id to hash (network/storage id).
    ///
    /// - `state_id`: 目标稠密 id
    pub fn runtime_id(&self, state_id: BlockStateId) -> Option<BlockRuntimeId> {
        self.state(state_id).map(|state| BlockRuntimeId(state.hash))
    }

    /// 调色板协议 network id 到稠密 id。
    ///
    /// Palette protocol network runtime id to dense id.
    ///
    /// - `network_id`: 调色板顺序编号
    pub fn by_network_runtime_id(&self, network_id: u32) -> Option<BlockStateId> {
        self.by_network_id.read().ok()?.get(&network_id).copied()
    }

    /// 空气哨兵的稠密 id（未加载时为 `None`）。
    ///
    /// Dense id of the air sentinel (`None` when not loaded).
    pub fn air_state_id(&self) -> Option<BlockStateId> {
        self.air.get().copied()
    }

    /// 未知方块哨兵的稠密 id（未加载时为 `None`）。
    ///
    /// Dense id of the unknown-block sentinel (`None` when not loaded).
    pub fn unknown_state_id(&self) -> Option<BlockStateId> {
        self.unknown.get().copied()
    }

    /// 标识符 + 完整 states 键值到稠密 id（O(1) 签名查找）。
    ///
    /// identifier plus complete states key-values to dense id (O(1) signature lookup).
    ///
    /// 使用方：世界生成等多状态方块构造（三轴原木、藤蔓方向等）；
    /// 构造时解析一次，运行热路径上缓存结果。
    ///
    /// Consumers: multi-state block construction such as worldgen (three-axis logs,
    /// vine directions); resolve once at construction, cache the result on runtime hot paths.
    ///
    /// - `identifier`: 方块标识符（如 `minecraft:oak_log`）
    /// - `props`: 属性名到属性值的切片
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

    /// 直接通过方块标识符 + 原始 states NBT 查找稠密状态 ID（O(1) 签名查找）。
    ///
    /// 适用于状态变种切换（如点燃/熄灭、激活/关闭、开启/关闭），直接继承源方块的所有 states/组件属性。
    ///
    /// Directly look up dense state ID by block identifier plus raw states NBT (O(1) signature lookup).
    /// Used for state variant transitions (lit/unlit, active/inactive), directly inheriting all source states.
    pub fn state_by_raw_states(
        &self,
        identifier: &str,
        states: &Option<sc_nbt::compound::CompoundNbt>,
    ) -> Option<BlockStateId> {
        let sig = state_signature(states);
        self.by_name_sig
            .read()
            .ok()?
            .get(identifier)?
            .get(&sig)
            .copied()
    }

    /// 把状态的单个属性改为目标值，返回目标稠密 id（O(1) 签名查找）。
    ///
    /// Changes one property of a state to the target dense id (O(1) signature lookup).
    ///
    /// 可在混合基数索引运算之前使用：不做"类型连续"假设，直接以 states 签名为键，
    /// 任意原版多状态方块均可命中。
    ///
    /// Usable before mixed-radix index math: makes no "types are contiguous" assumption
    /// and keys directly on the states signature, so any vanilla multi-state block hits.
    ///
    /// - `state_id`: 源状态稠密 id
    /// - `key`: 属性名
    /// - `value`: 目标属性值
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

    /// 从版本包调色板字节注册全部方块状态、稠密 id 与双向映射。
    ///
    /// Registers all block states plus dense ids and the two-way maps from
    /// version-pack palette bytes.
    ///
    /// 输入为 JavaLocalNbt（转换后的 `runtime_block_states.dat`），根结构为
    /// `{ blocks: List<{name, states, version, protocol_runtime_id}> }`。
    /// `protocol_runtime_id` 是当前协议调色板的顺序 runtime id。
    ///
    /// Input is JavaLocalNbt (converted `runtime_block_states.dat`), with a root
    /// `{ blocks: List<{name, states, version, protocol_runtime_id}> }`.
    /// `protocol_runtime_id` is the current protocol palette's sequential runtime id.
    ///
    /// 出错时返回描述性错误（network id 重复且哈希不一致、缺少 air 等）。
    ///
    /// Errors: duplicate network id with mismatched hash, missing air, etc.
    ///
    /// - `bytes`: 调色板 NBT 字节
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

            // Still registers into the global dictionary as the superset (old saves/custom blocks round-trip).
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

    /// 从 `.block.json` 快照构建一个全新注册表（供原子发布）。
    ///
    /// Builds a fresh registry from `.block.json` snapshots (for atomic publish).
    ///
    /// 调用方用返回值整体替换旧资源（`*registry = ...`），而不是修补旧表；
    /// 失败的 bundle 不会到达这里（不存在部分发布）。
    /// 稠密下标遵循快照顺序（类型标识符、各类型规范键序的 states），与旧调色板顺序不同；
    /// 持久化/网络只使用哈希与顺序 id，绝不使用稠密下标。
    ///
    /// Callers replace the old resource wholesale with the return value (`*registry = ...`)
    /// instead of patching the old table; a failed bundle never reaches here (no partial
    /// publish). Dense indices follow snapshot order (type identifiers, states in canonical
    /// per-type key order), which differs from the legacy palette order; persistence/network
    /// use only hashes and sequential ids, never dense indices.
    ///
    /// - `snapshot`: `.block.json` 编译产物快照
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
        // Snapshot states sync into the global dictionary superset, same as the legacy load path.
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

    /// Builds a synthetic palette (air, unknown, stone and three log axes).
    fn synthetic_palette() -> (BlockStateRegistry, usize) {
        use sc_binary::ByteWriter;
        use sc_nbt::compound::CompoundNbt;
        use sc_nbt::local::JavaLocalNbt;
        use sc_nbt::writer::NbtWriter;
        use sc_nbt::NbtValue;

        fn entry(name: &str, states: &[(&str, NbtValue)], network_id: i32) -> NbtValue {
            let mut state_compound = CompoundNbt::new(None);
            for (key, value) in states {
                state_compound.insert(*key, value.clone());
            }
            let mut entry = CompoundNbt::new(None);
            entry.insert("name", NbtValue::String(name.to_string()));
            entry.insert("states", NbtValue::Compound(state_compound));
            entry.insert("protocol_runtime_id", NbtValue::Int(network_id));
            NbtValue::Compound(entry)
        }

        let blocks = vec![
            entry("minecraft:air", &[], 0),
            entry("minecraft:unknown", &[], 1),
            entry("minecraft:stone", &[], 2),
            entry(
                "minecraft:oak_log",
                &[("pillar_axis", NbtValue::String("x".to_string()))],
                3,
            ),
            entry(
                "minecraft:oak_log",
                &[("pillar_axis", NbtValue::String("y".to_string()))],
                4,
            ),
            entry(
                "minecraft:oak_log",
                &[("pillar_axis", NbtValue::String("z".to_string()))],
                5,
            ),
        ];
        let mut root = CompoundNbt::new(Some(String::new()));
        root.insert("blocks", NbtValue::List(blocks));
        let mut writer = ByteWriter::new();
        NbtWriter::from_writer(&mut writer)
            .write::<JavaLocalNbt>(&NbtValue::Compound(root))
            .expect("palette 编码");
        let registry = BlockStateRegistry::new();
        let count = registry
            .load_palette_from_bytes(writer.as_slice())
            .expect("palette 解析");
        (registry, count)
    }

    #[test]
    fn palette_load_builds_dense_dual_id_mapping() {
        let (registry, count) = synthetic_palette();
        assert_eq!(count, 6, "palette 状态数应与合成条目一致: {count}");
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
        let (registry, _) = synthetic_palette();
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

        // state_by_raw_states: directly inherits raw states compound
        let log_y_state = registry.state(log_y).expect("log_y state");
        let matched_y = registry
            .state_by_raw_states("minecraft:oak_log", &log_y_state.states)
            .expect("raw states match");
        assert_eq!(matched_y, log_y);
    }

    #[test]
    fn component_flags_builds_dense_table_with_sentinels() {
        let (registry, count) = synthetic_palette();
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

}
