//! 方块快照类型与热路径只读查询。
//!
//! Block snapshot types and hot-path read-only queries.
//!
//! 由 [`super`] 再导出，既有引用路径保持不变。
//!
//! Re-exported from [`super`] so existing paths keep compiling.

use super::*;
use std::collections::HashMap;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use sc_packloader::block::PropValue;
use crate::mining_drops::{DropCompiled, MiningCompiled};
use crate::workstation::{ChestDecl, CraftingTableDecl, FurnaceDecl};

// ---------------------------------------------------------------------------
// Immutable registry snapshot (the sole atomically published product).
// ---------------------------------------------------------------------------

/// 一个方块类型的编译视图（快照中按标识符排序）。
///
/// Compiled view of a block type (sorted by identifier in the snapshot).
#[derive(Clone, Debug)]
pub struct SnapType {
    pub identifier: Box<str>,
    pub file: Box<str>,
    pub type_id: u32,
    pub properties: Vec<sc_packloader::block::PropertyDef>,
    /// 默认状态的稠密下标（每个属性的首个声明值；规范下标 0）。
    ///
    /// Dense index of the default state (first declared value of every property; canonical index 0).
    pub default_state: u32,
    pub state_ids: Vec<u32>,
}

/// 单个有效状态的编译视图（快照中按 (标识符, 类型化键) 排序；
/// 下标即新的稠密 `BlockStateId`）。
///
/// Compiled view of a single valid state (sorted by (identifier, typed key) in the snapshot;
/// the index is the new dense `BlockStateId`).
///
/// 静态能力不内联在本结构里；它们编译进快照的固定稠密列（见下），按下标寻址。
///
/// Static capabilities are not inlined in this struct; they compile into the snapshot's fixed dense
/// columns (below), addressed by index.
#[derive(Clone, Debug)]
pub struct SnapState {
    pub index: u32,
    pub type_id: u32,
    /// 规范状态键（`name=repr` 逗号连接；无属性时为 `default`）。
    ///
    /// Canonical state key (`name=repr` comma-joined; `default` when property-less).
    pub key: Box<str>,
    pub hash: u32,
    /// 原样的 `sc:protocol_runtime_ids`，绝不按排序重算。
    ///
    /// Verbatim `sc:protocol_runtime_ids`, never recomputed by sort order.
    pub network_id: u32,
    /// 类型化规范状态标识（排序的长度前缀编码）。
    ///
    /// Typed canonical state identity (sorted length-prefixed encoding).
    pub typed_key: Box<str>,
    /// 规范状态属性赋值（按名升序；等价于 states NBT）。
    ///
    /// Canonical state property assignments (ascending by name; equivalent to states NBT).
    pub props: Vec<(String, PropValue)>,
}

/// 私有构建器通过全部检查后原子发布的不可变快照。
///
/// Immutable snapshot atomically published after the private builder passes all checks.
///
/// 调用方共享 `Arc` 只读引用，无需按 Region 深拷贝；热路径只读
/// 本快照及由它派生的稠密表。
///
/// Callers share an `Arc` read-only reference without per-Region deep copies; hot paths only read
/// this snapshot and the dense tables derived from it.
#[derive(Clone, Debug, Default)]
pub struct BlockJsonSnapshot {
    pub schema_version: u32,
    pub network_id_mode: String,
    /// 内容指纹（packloader 按排序后的路径+字节+模式计算）：任何文件变化
    /// 都会改变它，适合用作生成器描述符。
    ///
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
    /// `minecraft:collision_box` 碰撞盒（每个 `[ox,oy,oz,sx,sy,sz]`，以 1/16 为单位；
    /// `enabled:false` 表示空列表 = 无碰撞；None = 未声明）。
    ///
    /// `minecraft:collision_box` boxes (each `[ox,oy,oz,sx,sy,sz]` in 16ths;
    /// `enabled:false` means empty list = no collision; None = undeclared).
    pub collision: Vec<Option<Vec<[f32; 6]>>>,
    /// `minecraft:destructible_by_mining` 秒数（None = 未声明）。
    ///
    /// `minecraft:destructible_by_mining` seconds (None = undeclared).
    pub mining_seconds: Vec<Option<f32>>,
    /// `sc:unbreakable`（与 destructible_by_mining 互斥；解析期逐层 + 合并后检查）。
    ///
    /// `sc:unbreakable` (mutually exclusive with destructible_by_mining; checked at parse time per layer plus post-merge).
    pub unbreakable: Vec<bool>,
    /// `minecraft:light_emission`（0..15）。
    ///
    /// `minecraft:light_emission`(0..15).
    pub light_emission: Vec<Option<u8>>,
    /// `minecraft:light_dampening`（0..15）。
    ///
    /// `minecraft:light_dampening`(0..15).
    pub light_dampening: Vec<Option<u8>>,
    /// `minecraft:loot` 表路径（完整表在独立数据域，此处仅引用）。
    ///
    /// `minecraft:loot` table path (full table lives in a separate data domain, referenced here).
    pub loot: Vec<Option<Box<str>>>,
    /// `sc:replaceable`。
    ///
    /// `sc:replaceable`.
    pub replaceable: Vec<Option<bool>>,
    /// `sc:liquid` 类型。
    ///
    /// `sc:liquid` kind.
    pub liquid: Vec<Option<Box<str>>>,
    /// `sc:random_tick`。
    ///
    /// `sc:random_tick`.
    pub random_tick: Vec<Option<bool>>,
    /// `sc:needs_support` 条件。
    ///
    /// `sc:needs_support` condition.
    pub needs_support: Vec<Option<Box<str>>>,
    /// `sc:can_contain_liquid`。
    ///
    /// `sc:can_contain_liquid`.
    pub can_contain_liquid: Vec<Option<bool>>,
    /// 编译后的 `sc:mining` profile（`None` 表示本状态无挖掘配置，走旧路径；
    /// 以 `BlockStateId` 稠密寻址，与 `states` 等长）。
    ///
    /// Compiled `sc:mining` profile (`None` means no mining for this state, legacy path applies;
    /// densely addressed by `BlockStateId`, same length as `states`).
    pub mining: Vec<Option<MiningCompiled>>,
    /// 从引用战利品表编译的掉落 profile（`None` 表示无掉落，不做猜测；
    /// `Some(enabled=false)` 表示显式无掉落；与 `states` 等长）。
    ///
    /// Compiled drops profile from the referenced loot table (`None` means no drops, nothing guessed;
    /// `Some(enabled=false)` means explicitly drop-free; same length as `states`).
    pub drops: Vec<Option<DropCompiled>>,
    /// 编译后的 `sc:chest` 声明（`None` 表示无箱子容器；与 `states` 等长）。
    ///
    /// Compiled `sc:chest` declaration (`None` means no chest container; same length as `states`).
    pub chests: Vec<Option<ChestDecl>>,
    /// 编译后的 `sc:furnace` 声明（`None` 表示无熔炉处理；与 `states` 等长）。
    ///
    /// Compiled `sc:furnace` declaration (`None` means no furnace processor; same length as `states`).
    pub furnaces: Vec<Option<FurnaceDecl>>,
    /// 编译后的 `sc:crafting_table` 声明（`None` 表示无合成台；与 `states` 等长）。
    ///
    /// Compiled `sc:crafting_table` declaration (`None` means no crafting table; same length as `states`).
    pub crafting_tables: Vec<Option<CraftingTableDecl>>,
    /// 非固定组件的原样值（名到值；schema 由插件注册校验，冷路径）。
    ///
    /// Non-fixed component verbatim values (name to value; schema validated by plugin registration, cold path).
    pub plugin_components: Vec<Vec<(Box<str>, serde_json::Value)>>,
}


// ---------------------------------------------------------------------------
// Snapshot queries (hot-path read-only, no JSON).
// ---------------------------------------------------------------------------

impl BlockJsonSnapshot {
    /// 状态数量。
    ///
    /// Number of states.
    pub fn state_count(&self) -> usize {
        self.states.len()
    }

    /// 类型数量。
    ///
    /// Number of types.
    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    /// 按下标取状态。
    ///
    /// Gets a state by index.
    ///
    /// - `index`: 状态稠密下标
    pub fn state(&self, index: u32) -> Option<&SnapState> {
        self.states.get(index as usize)
    }

    /// 按类型 id 取类型。
    ///
    /// Gets a type by type id.
    ///
    /// - `type_id`: 类型 id
    pub fn block_type(&self, type_id: u32) -> Option<&SnapType> {
        self.types.get(type_id as usize)
    }

    /// 按标识符取类型。
    ///
    /// Gets a type by identifier.
    ///
    /// - `identifier`: 方块标识符
    pub fn type_by_identifier(&self, identifier: &str) -> Option<&SnapType> {
        self.types
            .iter()
            .find(|t| t.identifier.as_ref() == identifier)
    }

    /// 方块默认状态的 FNV 状态哈希（供世界生成/物品映射；绝不猜测：缺失即 None）。
    ///
    /// FNV state hash of the block default (for worldgen/item mapping; never guessed: missing means None).
    ///
    /// - `identifier`: 方块标识符
    pub fn default_hash(&self, identifier: &str) -> Option<u32> {
        let idx = self.defaults.get(identifier)?;
        self.states.get(*idx as usize).map(|s| s.hash)
    }

    /// 方块默认状态的稠密下标。
    ///
    /// Dense index of the block default state.
    ///
    /// - `identifier`: 方块标识符
    pub fn default_state_idx(&self, identifier: &str) -> Option<u32> {
        self.defaults.get(identifier).copied()
    }

    /// 状态哈希到稠密下标。
    ///
    /// State hash to dense index.
    ///
    /// - `hash`: FNV 状态哈希
    pub fn state_idx_by_hash(&self, hash: u32) -> Option<u32> {
        self.by_hash.get(&hash).copied()
    }

    /// 协议 network id 到稠密下标。
    ///
    /// Protocol network id to dense index.
    ///
    /// - `network_id`: 调色板顺序编号
    pub fn state_idx_by_network_id(&self, network_id: u32) -> Option<u32> {
        self.by_network_id.get(&network_id).copied()
    }

    /// 标识符 + 完整属性赋值到稠密下标（类型化键，byte/int 不混淆）。
    ///
    /// identifier plus complete property assignments to dense index (typed key, byte/int never confused).
    ///
    /// - `identifier`: 方块标识符
    /// - `props`: 属性名到属性值列表
    pub fn typed_state_of(&self, identifier: &str, props: &[(String, PropValue)]) -> Option<u32> {
        let key = typed_state_key(identifier, props);
        self.by_typed.get(&(Box::from(identifier), key)).copied()
    }

    /// 标识符 + 宽松字符串属性赋值到状态哈希
    /// （取最小哈希，与全局字典扫描一致）。
    ///
    /// identifier plus lenient string property assignments to state hash
    /// (minimum hash wins, matching the global dictionary scan).
    ///
    /// 字符串匹配镜像字典规则：字符串精确相等、整数解析、byte 的
    /// `true`/`false`/整数解析。世界生成查表器优先用它而非全局字典：
    /// dlopen 的插件副本可能持有空字典，而快照总是随手可得。
    ///
    /// String matching mirrors the dictionary: exact string equality, int
    /// parse, byte `true`/`false`/int parse. Worldgen table builders prefer
    /// this over the global dictionary: dlopened plugin copies may hold an
    /// empty dictionary while the snapshot is always at hand there.
    ///
    /// - `identifier`: 方块标识符
    /// - `pairs`: 属性键值对（字符串形式）
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

    /// 一个标识符的全部状态哈希（未知时为空）。
    ///
    /// All state hashes of one identifier (empty when unknown).
    ///
    /// - `identifier`: 方块标识符
    pub fn state_hashes_of(&self, identifier: &str) -> Vec<u32> {
        let Some(ty) = self.type_by_identifier(identifier) else {
            return Vec::new();
        };
        ty.state_ids
            .iter()
            .filter_map(|&idx| self.states.get(idx as usize).map(|st| st.hash))
            .collect()
    }

    /// 把状态的单个属性改为目标值并返回目标下标（O(1) 类型化查找）。
    ///
    /// Changes one property of a state to the target index (O(1) typed lookup).
    ///
    /// - `index`: 源状态下标
    /// - `name`: 属性名
    /// - `value`: 目标属性值
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

    /// 逐状态能力位（供 `BlockComponentFlags` 构建）。
    ///
    /// Per-state capability bits (for `BlockComponentFlags` builds).
    ///
    /// 映射只编译显式声明、不做猜测：透明度/硬度没有数据来源，保持不置位；
    /// 其余位仅在其组件被显式声明时置位。
    ///
    /// The map compiles explicit declarations without guessing: transparency/solidity have no data source
    /// and stay unset; other bits are set only when their component is explicitly declared.
    ///
    /// - `index`: 状态稠密下标
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

    /// 类型化列访问器（固定稠密列；热/冷路径均只做数组索引，无反序列化）。
    ///
    /// Typed column accessors (fixed dense columns; array indexing on hot and cold paths, no deserialization).
    ///
    /// - `index`: 状态稠密下标
    pub fn collision_of(&self, index: u32) -> Option<&Vec<[f32; 6]>> {
        self.collision.get(index as usize)?.as_ref()
    }

    /// 挖掘秒数（`minecraft:destructible_by_mining`；None = 未声明）。
    ///
    /// Mine seconds (`minecraft:destructible_by_mining`; None = undeclared).
    ///
    /// - `index`: 状态稠密下标
    pub fn mining_seconds_of(&self, index: u32) -> Option<f32> {
        self.mining_seconds.get(index as usize).copied().flatten()
    }

    /// 是否不可破坏。
    ///
    /// Whether unbreakable.
    ///
    /// - `index`: 状态稠密下标
    pub fn is_unbreakable(&self, index: u32) -> bool {
        self.unbreakable
            .get(index as usize)
            .copied()
            .unwrap_or(false)
    }

    /// 发光等级。
    ///
    /// Light emission level.
    ///
    /// - `index`: 状态稠密下标
    pub fn light_emission_of(&self, index: u32) -> Option<u8> {
        self.light_emission.get(index as usize).copied().flatten()
    }

    /// 遮光等级。
    ///
    /// Light dampening level.
    ///
    /// - `index`: 状态稠密下标
    pub fn light_dampening_of(&self, index: u32) -> Option<u8> {
        self.light_dampening.get(index as usize).copied().flatten()
    }

    /// 战利品表路径。
    ///
    /// Loot table path.
    ///
    /// - `index`: 状态稠密下标
    pub fn loot_of(&self, index: u32) -> Option<&str> {
        self.loot.get(index as usize)?.as_deref()
    }

    /// 是否可替换。
    ///
    /// Whether replaceable.
    ///
    /// - `index`: 状态稠密下标
    pub fn replaceable_of(&self, index: u32) -> Option<bool> {
        self.replaceable.get(index as usize).copied().flatten()
    }

    /// 液体类型。
    ///
    /// Liquid kind.
    ///
    /// - `index`: 状态稠密下标
    pub fn liquid_of(&self, index: u32) -> Option<&str> {
        self.liquid.get(index as usize)?.as_deref()
    }

    /// 是否随机刻。
    ///
    /// Whether randomly ticked.
    ///
    /// - `index`: 状态稠密下标
    pub fn random_tick_of(&self, index: u32) -> Option<bool> {
        self.random_tick.get(index as usize).copied().flatten()
    }

    /// 支撑需求条件。
    ///
    /// Support requirement condition.
    ///
    /// - `index`: 状态稠密下标
    pub fn needs_support_of(&self, index: u32) -> Option<&str> {
        self.needs_support.get(index as usize)?.as_deref()
    }

    /// 是否可盛液体。
    ///
    /// Whether it can contain liquid.
    ///
    /// - `index`: 状态稠密下标
    pub fn can_contain_liquid_of(&self, index: u32) -> Option<bool> {
        self.can_contain_liquid
            .get(index as usize)
            .copied()
            .flatten()
    }

    /// 编译后的 `sc:mining` profile（热路径只读；`None` 表示无挖掘，走旧路径）。
    ///
    /// Compiled `sc:mining` profile (hot-path read-only; `None` means no mining, legacy path applies).
    ///
    /// - `index`: 状态稠密下标
    pub fn mining_of(&self, index: u32) -> Option<&MiningCompiled> {
        self.mining.get(index as usize)?.as_ref()
    }

    /// 从引用战利品表编译的掉落 profile（热路径只读；`None` 表示无掉落，不做猜测）。
    ///
    /// Compiled drops profile from the referenced loot table (hot-path read-only; `None` means no drops, nothing guessed).
    ///
    /// - `index`: 状态稠密下标
    pub fn drops_of(&self, index: u32) -> Option<&DropCompiled> {
        self.drops.get(index as usize)?.as_ref()
    }

    /// 编译后的 `sc:chest` 声明（热路径只读）。
    ///
    /// Compiled `sc:chest` declaration (hot-path read-only).
    ///
    /// - `index`: 状态稠密下标
    pub fn chest_of(&self, index: u32) -> Option<&ChestDecl> {
        self.chests.get(index as usize)?.as_ref()
    }

    /// 编译后的 `sc:furnace` 声明（热路径只读）。
    ///
    /// Compiled `sc:furnace` declaration (hot-path read-only).
    ///
    /// - `index`: 状态稠密下标
    pub fn furnace_of(&self, index: u32) -> Option<&FurnaceDecl> {
        self.furnaces.get(index as usize)?.as_ref()
    }

    /// 编译后的 `sc:crafting_table` 声明（热路径只读）。
    ///
    /// Compiled `sc:crafting_table` declaration (hot-path read-only).
    ///
    /// - `index`: 状态稠密下标
    pub fn crafting_table_of(&self, index: u32) -> Option<&CraftingTableDecl> {
        self.crafting_tables.get(index as usize)?.as_ref()
    }

    /// 该方块状态是否带交互容器或工作站。
    ///
    /// Whether this block state has an interactive container or workstation.
    ///
    /// - `index`: 状态稠密下标
    pub fn is_interactive(&self, index: u32) -> bool {
        self.chest_of(index).is_some()
            || self.furnace_of(index).is_some()
            || self.crafting_table_of(index).is_some()
    }

    /// 插件组件原样值（冷路径；schema 由插件注册校验）。
    ///
    /// Plugin component verbatim values (cold path; schema validated by plugin registration).
    ///
    /// - `index`: 状态稠密下标
    /// - `name`: 组件名
    pub fn plugin_component(&self, index: u32, name: &str) -> Option<&serde_json::Value> {
        self.plugin_components
            .get(index as usize)?
            .iter()
            .find(|(k, _)| k.as_ref() == name)
            .map(|(_, v)| v)
    }

    /// 从快照派生旧版 `block_palette.nbt` 字节（供仍消费调色板字节的调用方使用的适配器）。
    ///
    /// Derives legacy `block_palette.nbt` bytes from the snapshot (an adapter for callers
    /// that still consume palette bytes).
    ///
    /// 供仍消费调色板字节的调用方使用（动态插件的独立字典副本、旧测试路径）；
    /// 内容与快照一致。磁盘版本/LevelDB 键/线上格式不受影响。
    ///
    /// Independent dictionary copies of dlopened plugins and legacy test paths still read
    /// palette bytes; contents match the snapshot. Disk version/LevelDB key/wire formats are unaffected.
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