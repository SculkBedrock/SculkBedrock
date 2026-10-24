//! 稠密方块状态模型与能力位掩码。
//!
//! Dense block-state model and capability bitmask.
//!
//! [`BlockStateId`] 是逻辑热路径的**稠密序号下标**（属性表等 `Vec<T>` 以它为下标），
//! 与存储/网络侧使用的 [`sc_world::chunk::BlockRuntimeId`]（FNV1a-32 哈希）并行：
//! 哈希覆盖存储与网络，稠密 id 覆盖逻辑。
//! [`BlockComponentFlags`] 是以 [`BlockStateId`] 为下标的能力位掩码表。
//!
//! [`BlockStateId`] is the **dense ordinal index** for logic hot paths (`Vec<T>` tables indexed
//! by it), running alongside the storage/network [`sc_world::chunk::BlockRuntimeId`]
//! (FNV1a-32 hash). Hashes cover storage and network, dense ids cover logic.
//! [`BlockComponentFlags`] is the dense capability-bitmask table indexed by [`BlockStateId`].

use std::sync::{Arc, RwLock};

use sc_ecs::resource::Resource;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;

/// 稠密方块状态索引：逻辑热路径下标（属性表按它寻址）。
///
/// Dense block-state index: logic hot-path subscript (attribute tables address by it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockStateId(pub u32);

/// 单个方块状态的属性查找值。
///
/// Attribute lookup value for a single block state.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockPropertyValue {
    Bool(bool),
    Int(i32),
    Enum(Arc<str>),
}

/// 把属性值转换为 NBT 值（布尔转 Byte，整数转 Int/Long，枚举转 String）。
///
/// Converts a property value into an NBT value (bool to Byte, int to Int/Long,
/// enum to String).
///
/// - `value`: 待转换的属性值
pub(crate) fn property_to_nbt(value: &BlockPropertyValue) -> NbtValue {
    match value {
        BlockPropertyValue::Bool(b) => NbtValue::Byte(*b as i8),
        BlockPropertyValue::Int(i) => NbtValue::Int(*i),
        BlockPropertyValue::Enum(s) => NbtValue::String(s.to_string()),
    }
}

/// 一个已注册的方块状态：哈希 + 标识符 + 原始 states NBT + 调色板 network id。
///
/// A registered block state: hash plus identifier plus raw states NBT plus palette
/// network id.
#[derive(Clone, Debug)]
pub struct BlockState {
    /// FNV1a-32 状态哈希（值等于 [`sc_world::chunk::BlockRuntimeId`]）。
    ///
    /// FNV1a-32 state hash (equals the [`sc_world::chunk::BlockRuntimeId`] value).
    pub hash: u32,
    /// 方块标识符（如 `minecraft:oak_log`）。
    ///
    /// Block identifier (e.g. `minecraft:oak_log`).
    pub name: Arc<str>,
    /// 原始 states NBT（未知字段原样保留，重新序列化可无损往返）。
    ///
    /// Raw states NBT (unknown fields preserved, re-serialization round-trips losslessly).
    pub states: Option<CompoundNbt>,
    /// 调色板中的协议 network id（调色板顺序编号）。
    ///
    /// Protocol network runtime id from the palette (sequential palette order).
    pub network_id: u32,
}

/// 能力位掩码常量（核心定义，绝不指名具体方块）。
///
/// Capability bitmask constants (core definition; never names concrete blocks).
///
/// `IS_AIR` / `IS_UNKNOWN` 是协议哨兵（唯一的分层例外）；其余是数据驱动的位，
/// 默认为 0，由外部方块数据填充。
///
/// `IS_AIR` / `IS_UNKNOWN` are protocol sentinels (the sole layering exception); the
/// rest are data-driven bits, defaulting to 0 and filled from external block data.
pub mod flags {
    // Protocol sentinels.
    pub const IS_AIR: u64 = 1 << 0;
    pub const IS_UNKNOWN: u64 = 1 << 1;
    /// 该状态是否带有非空 states（据此决定是否需要解析属性）。
    ///
    /// Whether this state carries non-empty states (use it to decide if attribute parsing is needed).
    pub const HAS_STATES: u64 = 1 << 2;
    // Data-driven flags, default to 0.
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

/// 以 [`BlockStateId`] 为下标的稠密能力位掩码表。
///
/// Dense capability-bitmask table indexed by [`BlockStateId`].
///
/// 由 [`crate::registry::BlockStateRegistry`] 在调色板加载后构建，之后只读。
/// `has(state, bit)` 是 O(1) 数组探测，支撑随机刻过滤、红石信号等逻辑热路径。
/// 插件侧表格（如红石导电性）使用 `BlockColumn<T>`；本表只承载核心层能力位。
///
/// Built by [`crate::registry::BlockStateRegistry`] after palette load, read-only afterwards.
/// `has(state, bit)` is an O(1) array probe backing logic hot paths such as random-tick
/// filtering and redstone signals. Plugin side tables (e.g. redstone conductivity) use
/// `BlockColumn<T>`; this table only carries core-level capability bits.
#[derive(Resource, Default)]
pub struct BlockComponentFlags {
    /// 位掩码表本体，`RwLock` 保护以允许构建期写入、运行期并发读。
    ///
    /// The bitmask table itself, guarded by `RwLock` for build-time writes and
    /// concurrent reads at runtime.
    table: RwLock<Vec<u64>>,
}

impl Clone for BlockComponentFlags {
    /// 克隆能力位表（逐项复制底层 `Vec`，锁状态不复制）。
    ///
    /// Clones the capability bit table (copies the underlying `Vec` element-wise,
    /// lock state is not carried over).
    fn clone(&self) -> Self {
        Self {
            table: RwLock::new(self.table.read().expect("lock poisoned").clone()),
        }
    }
}

impl BlockComponentFlags {
    /// 创建空的能力位表。
    ///
    /// Creates an empty capability bit table.
    pub fn new() -> Self {
        Self::default()
    }

    /// 从注册表状态表构建（调色板加载时执行一次）。
    ///
    /// Builds from the registry state table (once at palette load).
    ///
    /// - `registry`: 方块状态注册表，提供空气/未知哨兵与全量状态快照
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

    /// 从 `.block.json` 快照构建（调色板加载时执行一次，之后只读）。
    ///
    /// Builds from a `.block.json` snapshot (once at palette load, read-only afterwards).
    ///
    /// 能力位只来自文件中的显式声明（见 `BlockJsonSnapshot::flag_bits` 的不猜映射）；
    /// 硬度/透明度没有数据来源，保持不置位。
    ///
    /// Capability bits come only from explicit file declarations (see the no-guessing
    /// `BlockJsonSnapshot::flag_bits` mapping); solidity/transparency have no data source
    /// and stay unset.
    ///
    /// - `snapshot`: `.block.json` 编译产物快照
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

    /// 按稠密 id 做 O(1) 位查询。
    ///
    /// O(1) bitmask lookup by dense id.
    ///
    /// - `state`: 目标状态的稠密 id
    /// - `bit`: 待查询的能力位（[`flags`] 中的常量）
    pub fn has(&self, state: BlockStateId, bit: u64) -> bool {
        self.table
            .read()
            .ok()
            .and_then(|table| table.get(state.0 as usize).copied())
            .is_some_and(|bits| bits & bit != 0)
    }

    /// 置位单个状态的能力位（供逐方块的数据驱动填充使用）。
    ///
    /// Sets one state's capability bits (used to fill per-block data-driven bits).
    ///
    /// - `state`: 目标状态的稠密 id
    /// - `bit`: 待置位的能力位（[`flags`] 中的常量）
    pub fn set(&self, state: BlockStateId, bit: u64) {
        if let Ok(mut table) = self.table.write() {
            if let Some(bits) = table.get_mut(state.0 as usize) {
                *bits |= bit;
            }
        }
    }

    /// 返回表中的状态数量。
    ///
    /// Returns the number of states in the table.
    pub fn len(&self) -> usize {
        self.table.read().map(|table| table.len()).unwrap_or(0)
    }
    /// 表是否为空。
    ///
    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 把 states NBT 归一化为规范签名（键按字典序；布尔作 `true`/`false`，整数作十进制，
/// 字符串原样）。与 [`crate::registry::BlockStateRegistry::state_of`] / `with_property`
/// 的 O(1) 签名查找配套使用。
///
/// Normalizes a states NBT into a canonical signature (keys in dict order; bool as
/// `true`/`false`, int as decimal, string verbatim). Pairs with the O(1) signature
/// lookup in [`crate::registry::BlockStateRegistry::state_of`] / `with_property`.
///
/// - `states`: 原始 states NBT，`None` 或空表归一为空签名
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

/// 把单个 NBT 值转换为签名词元（布尔作 `true`/`false`，其余按调试格式）。
///
/// Converts a single NBT value into a signature token (bool as `true`/`false`,
/// others via their debug formatting).
///
/// - `value`: 待转换的 NBT 值
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
