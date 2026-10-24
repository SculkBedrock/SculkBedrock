//! 旧调色板支持：已发布注册表、预存查询与等价性校验。
//!
//! Legacy palette support: published registry, preexisting lookup and equivalence.
//!
//! 由 [`super`] 再导出，既有引用路径保持不变。
//!
//! Re-exported from [`super`] so existing paths keep compiling.

use super::*;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use sc_ecs::resource::Resource;
use sc_packloader::block::LegacyPaletteEntry;

// ---------------------------------------------------------------------------
// Published snapshot resource (shared read-only across regions/worldgen, no deep copy).
// ---------------------------------------------------------------------------

/// 已发布的方块快照（`Arc` 只读共享；构建后释放 JSON 源）。
///
/// Published block snapshot (`Arc` read-only sharing; JSON source released after build).
#[derive(Resource, Default)]
pub struct BlockJsonRegistry {
    /// 当前快照（`None` 表示尚未发布）。
    ///
    /// Current snapshot (`None` when not yet published).
    snapshot: RwLock<Option<Arc<BlockJsonSnapshot>>>,
}

impl Clone for BlockJsonRegistry {
    /// 克隆注册表（快照 `Arc` 按值共享，锁状态不复制）。
    ///
    /// Clones the registry (the snapshot `Arc` is shared by value; lock state is not
    /// carried over).
    fn clone(&self) -> Self {
        Self {
            snapshot: RwLock::new(self.get()),
        }
    }
}

impl BlockJsonRegistry {
    /// 创建空注册表。
    ///
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// 发布快照（仅在全部检查通过后调用；失败路径绝不调用）。
    ///
    /// Publishes the snapshot (call only after all checks pass; never on failure paths).
    ///
    /// - `snapshot`: 待发布的快照
    pub fn publish(&self, snapshot: Arc<BlockJsonSnapshot>) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = Some(snapshot);
        }
    }

    /// 取当前快照。
    ///
    /// Gets the current snapshot.
    pub fn get(&self) -> Option<Arc<BlockJsonSnapshot>> {
        self.snapshot.read().ok()?.clone()
    }

    /// 是否已发布快照。
    ///
    /// Whether a snapshot has been published.
    pub fn is_present(&self) -> bool {
        self.snapshot.read().ok().is_some_and(|g| g.is_some())
    }
}

// ---------------------------------------------------------------------------
// Global dictionary coordination (probing plus post-success registration).
// ---------------------------------------------------------------------------

/// 生产期既有状态探测：查全局引导字典（首见者胜，此处绝不写入）。
///
/// Production existing-state probe: the global bootstrap dictionary (first-seen wins, never written here).
///
/// - `hash`: 状态哈希
pub fn global_preexisting_lookup(hash: u32) -> Option<(Box<str>, Box<str>)> {
    use sc_world::block_dictionary::BlockStateDictionary;
    let entry = BlockStateDictionary::global().get(hash)?;
    let key = typed_key_for_nbt(&entry.name, entry.states.as_ref());
    Some((Box::from(entry.name.as_str()), key))
}

/// 发布成功后把快照状态登记进全局字典（作为超集保留）。
/// 当且仅当整个包成功时调用；失败路径绝不调用（无部分发布）。
///
/// Registers snapshot states into the global dictionary as the superset after a
/// successful publish. Call if and only if the whole pack succeeded; never on failure
/// paths (no partial publish).
///
/// - `snapshot`: 已发布的快照
pub fn publish_legacy_support(snapshot: &BlockJsonSnapshot) {
    use sc_world::block_dictionary::{BlockStateDictionary, BlockStateEntry};
    let dictionary = BlockStateDictionary::global();
    for state in snapshot.states.iter() {
        let Some(type_view) = snapshot.block_type(state.type_id) else {
            continue;
        };
        dictionary.record_with_network_id(state.hash, state.network_id, || BlockStateEntry {
            name: type_view.identifier.to_string(),
            states: Some(states_nbt_of(&state.props)),
        });
    }
}

// ---------------------------------------------------------------------------
// Legacy/new data equivalence comparison.
// ---------------------------------------------------------------------------

/// 新旧等价性报告（逐状态比对类型、哈希与顺序 id）。
///
/// Legacy/new equivalence report (compares type, hash, and sequential id per state).
#[derive(Clone, Debug, Default)]
pub struct EquivalenceReport {
    /// 快照状态数。
    ///
    /// Snapshot state count.
    pub snapshot_states: usize,
    /// 旧调色板状态数。
    ///
    /// Legacy palette state count.
    pub legacy_states: usize,
    /// 快照中缺失的状态。
    ///
    /// States missing from the snapshot.
    pub missing_in_snapshot: Vec<String>,
    /// 快照中多出的状态。
    ///
    /// States extra in the snapshot.
    pub extra_in_snapshot: Vec<String>,
    /// 哈希不一致的状态。
    ///
    /// States with hash mismatches.
    pub hash_mismatches: Vec<String>,
    /// 顺序 id 不一致的状态。
    ///
    /// States with sequential id mismatches.
    pub network_id_mismatches: Vec<String>,
}

impl EquivalenceReport {
    /// 是否完全等价。
    ///
    /// Whether fully equivalent.
    pub fn is_equal(&self) -> bool {
        self.missing_in_snapshot.is_empty()
            && self.extra_in_snapshot.is_empty()
            && self.hash_mismatches.is_empty()
            && self.network_id_mismatches.is_empty()
    }
}

/// 把快照与旧调色板逐状态比对（标识符 + 类型化键、哈希、协议 id）。
///
/// Compares the snapshot against the legacy palette (per state: identifier plus typed key, hash, protocol id).
///
/// - `snapshot`: 新编译快照
/// - `legacy_entries`: 旧调色板条目
pub fn compare_legacy_snapshot(
    snapshot: &BlockJsonSnapshot,
    legacy_entries: &[LegacyPaletteEntry],
) -> EquivalenceReport {
    let mut legacy_map: HashMap<(String, Box<str>), Vec<usize>> = HashMap::new();
    for (i, entry) in legacy_entries.iter().enumerate() {
        if entry.name.is_empty() {
            continue;
        }
        let key = typed_key_for_nbt(&entry.name, entry.states.as_ref());
        legacy_map
            .entry((entry.name.clone(), key))
            .or_default()
            .push(i);
    }
    let mut report = EquivalenceReport {
        snapshot_states: snapshot.states.len(),
        legacy_states: legacy_entries.len(),
        ..Default::default()
    };
    let mut matched_legacy = vec![false; legacy_entries.len()];
    for state in snapshot.states.iter() {
        let type_view = match snapshot.block_type(state.type_id) {
            Some(t) => t,
            None => continue,
        };
        let key = typed_state_key(&type_view.identifier, &state.props);
        match legacy_map.get(&(type_view.identifier.to_string(), key)) {
            None => report
                .extra_in_snapshot
                .push(format!("{}#{:?}", type_view.identifier, state.key)),
            Some(indices) => {
                for i in indices {
                    matched_legacy[*i] = true;
                    let legacy = &legacy_entries[*i];
                    if legacy.network_id != state.network_id {
                        report.network_id_mismatches.push(format!(
                            "{}#{:?}: 快照 {} vs 旧 {}",
                            type_view.identifier, state.key, state.network_id, legacy.network_id
                        ));
                    }
                    let legacy_hash = sc_world::leveldb::block_hash::block_state_hash(
                        &legacy.name,
                        legacy.states.as_ref(),
                    );
                    if state.hash != legacy_hash {
                        report
                            .hash_mismatches
                            .push(format!("{}#{:?}", type_view.identifier, state.key));
                    }
                }
            }
        }
    }
    for (i, entry) in legacy_entries.iter().enumerate() {
        if matched_legacy[i] {
            continue;
        }
        report.missing_in_snapshot.push(format!(
            "{}#{}",
            entry.name,
            typed_key_for_nbt(&entry.name, entry.states.as_ref())
        ));
    }
    report
}