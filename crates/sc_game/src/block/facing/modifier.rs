//! 方块状态修改器（BlockStateModifier）。
//!
//! 封装 BlockStateRegistry 与当前状态的查询、修改和回写样板。
//!
//! Block state modifier (BlockStateModifier).
//!
//! Wraps the query, mutate and write-back boilerplate against the
//! [`BlockStateRegistry`] for a single block's current state.

use sc_block::registry::BlockStateRegistry;
use sc_block::state::{BlockPropertyValue, BlockStateId};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;

/// 状态修改器：管理当前方块正在修改的属性与稠密状态。
///
/// State modifier: tracks the properties and dense state a block is being changed to.
pub struct BlockStateModifier<'a> {
    registry: &'a BlockStateRegistry,
    /// 方块标识符（如 `"minecraft:oak_log"`）。
    ///
    /// Block identifier (e.g. `"minecraft:oak_log"`).
    pub block_name: String,
    /// 当前在注册表中的稠密状态 id。
    ///
    /// Current dense state id in the registry.
    pub state_id: BlockStateId,
    /// 当前状态的 NBT 属性字典快照。
    ///
    /// NBT property-dictionary snapshot of the current state.
    pub states: Option<CompoundNbt>,
    base_hash: u32,
}

impl<'a> BlockStateModifier<'a> {
    /// 由注册表和基础状态哈希构建修改器。
    ///
    /// Builds a modifier from the registry and a base state hash; returns `None` when
    /// the hash is unknown to the registry.
    pub fn new(registry: &'a BlockStateRegistry, base_hash: u32) -> Option<Self> {
        let state_id = registry.by_runtime_id(sc_world::chunk::BlockRuntimeId(base_hash))?;
        let entry = registry.state(state_id)?;
        Some(Self {
            registry,
            block_name: entry.name.to_string(),
            state_id,
            states: entry.states.clone(),
            base_hash,
        })
    }

    /// 查询该方块当前状态是否具有指定属性键名。
    ///
    /// Whether the block's current state has the given property key.
    #[inline]
    pub fn has_property(&self, name: &str) -> bool {
        self.states.as_ref().is_some_and(|s| s.contains_key(name))
    }

    /// 设置一个属性值并切换内部 `state_id`。
    ///
    /// Sets one property value and switches the internal `state_id`; returns whether
    /// the target state exists.
    pub fn set_property(&mut self, name: &str, value: BlockPropertyValue) -> bool {
        if let Some(next_id) = self.registry.with_property(self.state_id, name, value) {
            self.state_id = next_id;
            return true;
        }
        false
    }

    /// 按调色板原有的 NBT 数据类型写入数值（Int 写入 Int，Byte 写入 Bool）。
    ///
    /// Writes a numeric value using the palette's existing NBT type (Int stays Int,
    /// Byte becomes Bool); returns `false` for other/absent types.
    pub fn set_typed_numeric(&mut self, name: &str, int_val: i32) -> bool {
        let current = self.states.as_ref().and_then(|s| s.get(name));
        let prop_val = match current {
            Some(NbtValue::Int(_)) => BlockPropertyValue::Int(int_val),
            Some(NbtValue::Byte(_)) => BlockPropertyValue::Bool(int_val != 0),
            _ => return false,
        };
        self.set_property(name, prop_val)
    }

    /// 若方块包含 `upside_down_bit`，根据计算结果写入倒置位（重新读取最新存储类型避免类型错位）。
    ///
    /// Writes the `upside_down_bit` when the block has it, re-reading the current
    /// stored type first to avoid a type mismatch.
    pub fn apply_upside_down(&mut self, is_upside_down: bool) {
        if self.has_property("upside_down_bit") {
            let int_val = if is_upside_down { 1 } else { 0 };
            let fresh_states = self
                .registry
                .state(self.state_id)
                .and_then(|entry| entry.states.clone());
            let current = fresh_states.as_ref().and_then(|s| s.get("upside_down_bit"));
            let prop_val = match current {
                Some(NbtValue::Byte(_)) => BlockPropertyValue::Bool(int_val != 0),
                Some(NbtValue::Int(_)) => BlockPropertyValue::Int(int_val),
                _ => BlockPropertyValue::Bool(int_val != 0),
            };
            self.set_property("upside_down_bit", prop_val);
        }
    }

    /// 提交当前修改，返回最终运行时哈希；若解析失败则回退至 base_hash。
    ///
    /// Commits the current changes and returns the final runtime hash, falling back to
    /// `base_hash` when resolution fails.
    pub fn commit(self) -> u32 {
        self.registry
            .runtime_id(self.state_id)
            .map(|id| id.0)
            .unwrap_or(self.base_hash)
    }
}
