//! 方块状态引导字典（状态哈希 → 名称+状态原始 NBT）。
//!
//! FNV1a-32 哈希不可逆，LevelDB 调色板解析是唯一同时持有哈希与原始
//! `name+states` 的地方（`leveldb/format.rs::read_palette_entry`）。
//! 在那里注册即可让服务器在无数据文件的情况下回答某个哈希对应哪个方块，
//! 并保证磁盘上出现的任何状态（旧存档、自定义方块）无损往返。
//! 版本包调色板到来前该字典是回退源；到来之后仍保留用于交叉校验与
//! 超集回退。
//!
//! 性能：注册按调色板条目发生（每存储数个到数十个，不是每方块）；
//! 命中路径为一次读锁 + u32 查找，零分配；条目仅在首次见到某哈希时
//! 构建（双检锁下插入）。
//!
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

/// 一个已见过的方块状态：标识符 + 原始状态 NBT。
/// 状态保留未知字段，保证重新序列化无损往返。
///
/// One seen block state: identifier + raw states NBT.
/// States keep unknown fields so re-serialization round-trips losslessly.
#[derive(Clone, Debug)]
pub struct BlockStateEntry {
    /// 方块标识符，如 `minecraft:stone`。
    ///
    /// Block identifier, e.g. `minecraft:stone`.
    pub name: String,
    /// 原始状态 NBT；无状态方块为 `None`。
    ///
    /// Raw states NBT; `None` for stateless blocks.
    pub states: Option<CompoundNbt>,
}

impl fmt::Display for BlockStateEntry {
    /// 格式化为 `name{key=value, ...}`，状态按键名排序。
    ///
    /// Formats as `name{key=value, ...}` with states sorted by key.
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

/// 调色板状态值与查询串的匹配：字符串逐字比较，整数按十进制比较；
/// 字节接受布尔写法（`true`/`false`）与数字。
///
/// Match palette state values against query strings: strings compare verbatim, integers compare as decimal;
/// bytes accept boolean spellings (`true`/`false`) and numbers.
///
/// - `value`: 调色板中的 NBT 值（缺失视为不匹配）
/// - `query`: 查询串
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

/// `minecraft:air` 的哈希网络 id（进程内缓存，只算一次）。
///
/// Hash network id of `minecraft:air` (cached per process, computed once).
pub fn air_runtime_id() -> u32 {
    *AIR_RUNTIME_ID.get_or_init(|| block_state_hash("minecraft:air", None))
}

/// 从全局字典解析方块标识符的规范默认哈希（与版本包状态定义一致），
/// 找不到则回退到 `block_state_hash(name, None)` 并注册进
/// [`BlockStateDictionary::global`]，保证 LevelDB 持久化不会因
/// 「哈希不在引导字典里」而失败。
///
/// Resolves a block identifier's canonical default hash from the global dictionary
/// (matching version pack state definitions), or falls back to `block_state_hash(name, None)`
/// and registers it in `BlockStateDictionary::global()` so LevelDB persistence never fails
/// with "hash not in bootstrap dictionary".
///
/// - `name`: 方块标识符
pub fn resolve_or_register_default(name: &str) -> u32 {
    let dict = BlockStateDictionary::global();
    if let Some(hash) = dict.first_hash_of(name) {
        return hash;
    }
    let alias = match name {
        "minecraft:dead_bush" => Some("minecraft:deadbush"),
        "minecraft:snow_block" => Some("minecraft:snow"),
        "minecraft:terracotta" => Some("minecraft:hardened_clay"),
        _ => None,
    };
    if let Some(alias) = alias {
        if let Some(hash) = dict.first_hash_of(alias) {
            return hash;
        }
    }
    let hash = block_state_hash(name, None);
    dict.record_with(hash, || BlockStateEntry {
        name: name.to_string(),
        states: None,
    });
    hash
}

/// Converts an internal state hash to the version-pack runtime id used by
/// 把内部状态哈希转换为旧版/非哈希包路径使用的版本包 runtime id。
///
/// 协议 2168 的哈希方块包不调用本函数：它们直接携带 FNV1a 状态哈希。
/// 两套 id 空间对仍需调色板映射的物品/旧版包保持有意分离。
///
/// Converts an internal state hash to the version-pack runtime id used by
/// legacy/non-hashed packet paths.
///
/// Protocol 2168 hashed block packets do not call this function: they carry
/// the FNV1a state hash itself. The two id spaces remain deliberately
/// separate for item/legacy packets that still require the palette mapping.
///
/// 哈希未注册时返回 `None`。
///
/// Returns `None` when the hash is not registered.
pub fn network_runtime_id(hash: u32) -> Option<u32> {
    BlockStateDictionary::global().network_id(hash)
}

/// 全局方块状态字典：哈希 → 条目、哈希 → 网络 id、标识符 → 首次哈希。
///
/// Global block-state dictionary: hash to entry, hash to network id and
/// identifier to first-seen hash.
pub struct BlockStateDictionary {
    /// 状态哈希 → 条目。
    ///
    /// State hash to entry.
    entries: RwLock<HashMap<u32, Arc<BlockStateEntry>, RandomState>>,
    /// 状态哈希 → 版本包网络 id。
    ///
    /// State hash to version-pack network id.
    network_ids: RwLock<HashMap<u32, u32, RandomState>>,
    /// 标识符 → 首次见到的哈希。默认值落地前，按名查找（如 /setblock）
    /// 用该方块首次见到的状态作为默认。
    ///
    /// identifier to first-seen hash. Before defaults land, name-based lookups (e.g. /setblock)
    /// use the first-seen state of a block as its default.
    by_name: RwLock<HashMap<String, u32, RandomState>>,
}

impl BlockStateDictionary {
    /// 构造只预注册内部哨兵条目的字典。网络 runtime id 必须来自
    /// 版本包调色板，因为连空气状态 id 都是随版本而变的数据。
    ///
    /// Builds a dictionary pre-registering only internal sentinel entries.
    /// Network runtime ids must come from the version palette, because even
    /// the air-state id is version-dependent data.
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

    /// 进程级全局字典（懒初始化，只注册一次）。
    ///
    /// Process-wide global dictionary (lazily initialized, seeded once).
    pub fn global() -> &'static BlockStateDictionary {
        GLOBAL_DICTIONARY.get_or_init(Self::new_seeded)
    }

    /// 记录一个哈希。命中路径：一次读锁 + 查找，不调用 `make_entry`，
    /// 零分配；未命中：写锁下双检插入（并发首见者先到先得），
    /// 并维护「标识符 → 首次哈希」索引。
    ///
    /// Record one hash. Hit path: one read lock + lookup, no `make_entry` call, zero allocation.
    /// Miss: double-checked insert under the write lock (concurrent first sights keep the first writer),
    /// and maintain the identifier-to-first-hash index.
    ///
    /// - `hash`: 状态哈希
    /// - `make_entry`: 条目构造闭包（仅未命中时调用）
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

    /// 按哈希取条目；未注册返回 `None`。
    ///
    /// Looks up an entry by hash; returns `None` when unregistered.
    pub fn get(&self, hash: u32) -> Option<Arc<BlockStateEntry>> {
        self.entries.read().ok()?.get(&hash).cloned()
    }

    /// 从已加载的版本包调色板取协议 runtime id；未注册返回 `None`。
    ///
    /// Returns the protocol runtime id from the loaded version-pack palette;
    /// returns `None` when unregistered.
    pub fn network_id(&self, hash: u32) -> Option<u32> {
        self.network_ids.read().ok()?.get(&hash).copied()
    }

    /// 记录哈希及其版本包网络 id（网络 id 只记首个，不覆盖）。
    ///
    /// Records a hash together with its version-pack network id (the first
    /// network id wins).
    ///
    /// - `hash`: 状态哈希
    /// - `network_id`: 版本包网络 id
    /// - `make_entry`: 条目构造闭包（仅哈希未命中时调用）
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

    /// 标识符 → 该方块首次见到的状态哈希（引导默认值）；未注册返回 `None`。
    ///
    /// identifier to the first-seen state hash of the block (the bootstrap default);
    /// returns `None` when unregistered.
    pub fn first_hash_of(&self, name: &str) -> Option<u32> {
        self.by_name.read().ok()?.get(name).copied()
    }

    /// 标识符 + 一组状态键值对 → 状态哈希。
    ///
    /// 消费方：世界生成（多状态方块，如带 `pillar_axis` 的三轴原木、
    /// 带 `vine_direction_bits` 的藤蔓）。单次全表扫描，适合构建期的
    /// 一次性解析（热路径应缓存结果）。
    ///
    /// Consumer: world generation (multi-state blocks such as three-axis logs with `pillar_axis`
    /// and vines with `vine_direction_bits`). Single full-table scan, suited for one-time
    /// resolution at build time (hot paths should cache the result).
    ///
    /// - `name`: 方块标识符
    /// - `state_key` / `state_value`: 单个状态键值对
    pub fn find_hash_by_state(
        &self,
        name: &str,
        state_key: &str,
        state_value: &str,
    ) -> Option<u32> {
        self.find_hash_by_states(name, &[(state_key, state_value)])
    }

    /// 标识符 + 多个状态键值对（全部须匹配）→ 状态哈希。
    ///
    /// identifier plus multiple state key-value pairs (all must match) to state hash.
    ///
    /// 值匹配覆盖调色板 NBT 的三种存储：字符串（`pillar_axis="x"`）、
    /// 整数（`vine_direction_bits=8`、`age=2`）与字节布尔
    /// （`tip=true`）。并列时取最小哈希以保证确定性。
    ///
    /// Value matching covers the three palette NBT storages: strings (`pillar_axis="x"`),
    /// integers (`vine_direction_bits=8`, `age=2`), and byte bools
    /// (`tip=true`). Ties take the smallest hash for determinism.
    ///
    /// - `name`: 方块标识符
    /// - `pairs`: 状态键值对列表
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

    /// 某标识符已注册的全部状态（调试/测试用；顺序不保证）。
    ///
    /// All registered states of an identifier (debug/test use; order undefined).
    ///
    /// 返回 `(哈希, 条目)` 列表，按哈希升序。
    ///
    /// Returns `(hash, entry)` pairs sorted by hash ascending.
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

    /// 已注册条目数。
    ///
    /// Number of registered entries.
    pub fn len(&self) -> usize {
        self.entries
            .read()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }

    /// 是否为空（无任何条目）。
    ///
    /// Whether the dictionary is empty (no entries).
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
