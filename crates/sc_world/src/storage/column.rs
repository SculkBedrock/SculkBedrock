use super::{BlockSpilloverWrite, ChunkKey, SpilloverJournalEntry, SpilloverOperationId};
use crate::chunk::Chunk;
use crate::generation::record::ReadyCertificate;
use crate::manager::MinecraftWorldId;
use std::collections::{BTreeSet, HashMap};
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak,
};
use std::time::Instant;

/// 网络与流水线准入的 Ready 消费收据。
///
/// Ready consumption receipt for network and pipeline admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyColumnReceipt {
    /// 列所属世界 id。
    ///
    /// Owning world id.
    pub world_id: MinecraftWorldId,
    /// 区块键。
    ///
    /// Chunk key.
    pub key: ChunkKey,
    /// 列身份号。
    ///
    /// Column incarnation.
    pub incarnation: u128,
    /// 内容代际。
    ///
    /// Content generation.
    pub generation: u64,
    /// 生成器指纹。
    ///
    /// Generator fingerprint.
    pub generator_fingerprint: u64,
    /// Ready 证书。
    ///
    /// Ready certificate.
    pub certificate: ReadyCertificate,
}

/// 一个可变的共享区块列：缓存交付出去的单位。
///
/// A mutable, shared chunk column: the unit the cache hands out.
///
/// 读者（网络编码、get_block）拿列级读锁；单写者的 apply 系统拿写锁。
/// 每子区块脏位索引进供 LevelDB 增量写回。
///
/// Readers (network encode, get_block) take the column-level read lock;
/// the single-writer apply system takes the write lock. Per-subchunk dirty
/// indices feed incremental LevelDB write-back.
pub struct ChunkColumn {
    /// 区块本体（读写锁保护）。
    ///
    /// The chunk itself (guarded by a read-write lock).
    chunk: RwLock<Chunk>,
    /// 列身份号。
    ///
    /// Column incarnation.
    incarnation: u128,
    /// 脏子区块段号集合。
    ///
    /// Dirty subchunk section indices.
    dirty_subchunks: Mutex<BTreeSet<i8>>,
    /// 已应用的 spillover 操作 id 集合。
    ///
    /// Applied spillover operation ids.
    applied_spillover_operations: Mutex<BTreeSet<SpilloverOperationId>>,
    /// 跨列生成写的逐块仲裁状态。
    ///
    /// 两个生成器可能指向同一方块位置。`SpilloverOperationId` 是
    /// 全序，因此「id 最大者胜」使发布结果与哪个 worker 先完成无关。
    /// 低于已记录胜者的条目是*被取代的生成结果*，不是丢失的世界事实：
    /// 它们被计数与诊断。
    ///
    /// Per-block arbitration state for cross-column generation writes.
    ///
    /// Two generators may target the same block position.
    /// `SpilloverOperationId` is a total order, so "largest id wins" makes the
    /// published result independent of which worker finished first. Entries
    /// below the recorded winner are *superseded generation results*, not
    /// dropped world facts: they are counted and diagnosed.
    spillover_conflicts: Mutex<SpilloverConflictState>,
    /// 方块实体是否脏。
    ///
    /// Whether block entities are dirty.
    block_entities_dirty: AtomicBool,
    /// 生物群系是否脏。
    ///
    /// Whether biomes are dirty.
    biomes_dirty: AtomicBool,
    /// 高度图是否脏。
    ///
    /// Whether the heightmap is dirty.
    heightmap_dirty: AtomicBool,
    /// 脏列索引注册（弱引用，避免循环）。
    ///
    /// Dirty-column index registration (weak, avoids cycles).
    dirty_registration: Mutex<Option<DirtyColumnRegistration>>,
    /// 变更计数器：`mark_dirty` 时递增；用于失效 `LevelChunkCache`。
    ///
    /// Change counter: incremented on `mark_dirty`; used to invalidate `LevelChunkCache`.
    generation: AtomicU64,
    /// 估算字节数（原子，供缓存预算）。
    ///
    /// Estimated bytes (atomic, for cache budgets).
    pub(crate) estimated_bytes: AtomicUsize,
    /// 可选的全局内存计数器。
    ///
    /// Optional global memory counter.
    memory_counter: Option<Arc<AtomicUsize>>,
    /// 是否已计入内存计数器。
    ///
    /// Whether the column is currently accounted in the memory counter.
    memory_accounted: AtomicBool,
    /// spillover 钉住数（防止写入目标被驱逐）。
    ///
    /// Spillover pin count (prevents evicting a write target).
    pub(crate) spillover_pins: AtomicUsize,
    /// Ready 证书（parking_lot 锁）。
    ///
    /// Ready certificate (parking_lot lock).
    ready_certificate: parking_lot::RwLock<Option<ReadyCertificate>>,
    /// 生成器指纹。
    ///
    /// Generator fingerprint.
    generator_fingerprint: AtomicU64,
}

/// 权威列自持的逐块仲裁状态。
///
/// Per-block arbitration state owned by the authoritative column.
///
/// 跟踪窗口有界：收到超过 [`MAX_TRACKED_SPILLOVER_CELLS`] 个不同
/// spillover 目标的列仍会应用这些写，但不再记录，因此确定性保证对未
/// 跟踪位置降级为到达顺序。该降级被计数与报告，而不是被默默假定。
///
/// The tracked window is bounded: a column that receives more distinct
/// spillover targets than [`MAX_TRACKED_SPILLOVER_CELLS`] keeps applying those
/// writes but stops recording them, so the deterministic guarantee degrades to
/// arrival order for the untracked positions. That degradation is counted and
/// reported instead of being silently assumed.
pub(crate) const MAX_TRACKED_SPILLOVER_CELLS: usize = 512;

/// 一个 spillover 目标的全局方块坐标。
///
/// Global block coordinate of one spillover target.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
struct SpilloverCell {
    /// 方块层。
    ///
    /// Block layer.
    layer: usize,
    /// 世界 x。
    ///
    /// World x.
    x: i32,
    /// 世界 y。
    ///
    /// World y.
    y: i32,
    /// 世界 z。
    ///
    /// World z.
    z: i32,
}

impl SpilloverCell {
    /// 由 spillover 写提取目标坐标。
    ///
    /// Extracts the target coordinates from a spillover write.
    fn of(write: &BlockSpilloverWrite) -> Self {
        Self {
            layer: write.layer,
            x: write.x,
            y: write.y,
            z: write.z,
        }
    }
}

/// 对一个 spillover 批次按列的胜者表仲裁的结果。
///
/// Result of arbitrating one spillover batch against a column's winners.
#[derive(Default)]
pub(crate) struct SpilloverPartition {
    /// 本列立即应用的写。
    ///
    /// Writes this column applies now.
    pub(crate) applicable: Vec<SpilloverJournalEntry>,
    /// 调用方必须重新排队、稍后有序应用的写。
    ///
    /// Writes the caller must re-queue for a later, ordered application.
    pub(crate) deferred: Vec<SpilloverJournalEntry>,
    /// 已被更高优先级生成取代的写。
    ///
    /// Writes a higher-priority generation already superseded.
    pub(crate) superseded: u64,
    /// 因仲裁窗口满而被推迟的写。
    ///
    /// Deferred writes caused by a full arbitration window.
    pub(crate) untracked: u64,
}

#[derive(Default)]
struct SpilloverConflictState {
    /// 方块位置 → 已应用的最大 `SpilloverOperationId`。
    ///
    /// block position → largest `SpilloverOperationId` already applied there.
    winners: HashMap<SpilloverCell, SpilloverOperationId>,
    /// 因更高优先级生成已胜出而被跳过的写。
    ///
    /// Writes skipped because a higher-priority generation already won.
    // Spillover/journal reserved capability.
    #[allow(dead_code)]
    pub(crate) superseded: u64,
    /// 因跟踪窗口满而被重新排队的位置。
    ///
    /// Positions that were re-queued because the tracked window was full.
    pub(crate) untracked: u64,
}

/// 脏列键索引：provider 全局的有序脏键集合。
///
/// Dirty-column key index: a provider-global ordered set of dirty keys.
#[derive(Default)]
pub(crate) struct DirtyColumnIndex {
    /// 脏键集合（BTree 有序，支持游标窗口）。
    ///
    /// Dirty key set (BTree-ordered, supports cursor windows).
    keys: RwLock<BTreeSet<ChunkKey>>,
}

impl DirtyColumnIndex {
    /// 插入一个脏键。
    ///
    /// Inserts a dirty key.
    ///
    /// - `key`: 区块键
    pub(crate) fn insert(&self, key: ChunkKey) {
        self.keys
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key);
    }

    /// 移除一个脏键。
    ///
    /// Removes a dirty key.
    ///
    /// - `key`: 区块键
    pub(crate) fn remove(&self, key: &ChunkKey) {
        self.keys
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key);
    }

    /// 从 `after`（不含）起取至多 `limit` 个键，绕回补足。
    ///
    /// Takes up to `limit` keys after `after` (exclusive), wrapping around to
    /// fill the remainder.
    ///
    /// - `after`: 游标（`None` 从最小键开始）
    /// - `limit`: 数量上限
    pub(crate) fn window(&self, after: Option<ChunkKey>, limit: usize) -> Vec<ChunkKey> {
        if limit == 0 {
            return Vec::new();
        }
        let keys = self
            .keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut selected = Vec::with_capacity(limit);
        if let Some(cursor) = after {
            selected.extend(
                keys.range((Excluded(cursor), Unbounded))
                    .take(limit)
                    .copied(),
            );
            if selected.len() < limit {
                selected.extend(keys.range(..=cursor).take(limit - selected.len()).copied());
            }
        } else {
            selected.extend(keys.iter().take(limit).copied());
        }
        selected
    }

    /// 取 `(after, through]` 区间内至多 `limit` 个键（不绕回）。
    ///
    /// Takes up to `limit` keys in the range `(after, through]` without
    /// wrapping.
    ///
    /// - `after`: 下界游标（不含；`None` 无下界）
    /// - `through`: 上界（含；`None` 无上界）
    /// - `limit`: 数量上限
    pub(crate) fn keys_after(
        &self,
        after: Option<ChunkKey>,
        through: Option<ChunkKey>,
        limit: usize,
    ) -> Vec<ChunkKey> {
        if limit == 0 {
            return Vec::new();
        }
        if after
            .zip(through)
            .is_some_and(|(cursor, end)| cursor >= end)
        {
            return Vec::new();
        }
        let keys = self
            .keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let start = after.map_or(Unbounded, Excluded);
        let end = through.map_or(Unbounded, Included);
        keys.range((start, end)).take(limit).copied().collect()
    }

    /// 最大脏键（无脏键返回 `None`）。
    ///
    /// The largest dirty key (`None` when there is none).
    pub(crate) fn last_key(&self) -> Option<ChunkKey> {
        self.keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last()
            .copied()
    }
}

/// 列在 provider 脏索引中的注册记录。
///
/// A column's registration record in the provider's dirty index.
struct DirtyColumnRegistration {
    /// 注册的区块键。
    ///
    /// Registered chunk key.
    key: ChunkKey,
    /// 脏索引的弱引用。
    ///
    /// Weak reference to the dirty index.
    index: Weak<DirtyColumnIndex>,
}

/// 插入执行私有准备期间预留一个常驻缓存槽。其它并发准入把该预留
/// 计入各自的计数检查；丢弃它在每条错误路径上释放槽位。
///
/// Reserves one resident cache slot while an insertion performs private
/// preparation. Other concurrent admissions include this reservation in
/// their count check; dropping it releases the slot on every error path.
pub(crate) struct CacheSlotReservation {
    /// 待定槽计数器。
    ///
    /// Pending-slot counter.
    pub(crate) pending: Arc<AtomicUsize>,
    /// 预留是否仍有效。
    ///
    /// Whether the reservation is still active.
    pub(crate) active: bool,
}

impl CacheSlotReservation {
    /// 提交预留（槽位已由正式插入占用，计数器交还）。
    ///
    /// Commits the reservation (the real insertion now owns the slot, so the
    /// counter is returned).
    pub(crate) fn commit(&mut self) {
        if self.active {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            self.active = false;
        }
    }
}

impl Drop for CacheSlotReservation {
    /// 未提交而丢弃时归还槽位。
    ///
    /// Returns the slot when dropped without commit.
    fn drop(&mut self) {
        if self.active {
            self.pending.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl ChunkColumn {
    /// 由区块构造列（不计入全局内存计数器）。
    ///
    /// Builds a column from a chunk (not accounted in the global memory
    /// counter).
    pub fn new(chunk: Chunk) -> Self {
        Self::new_with_counter(chunk, None)
    }

    /// 由区块构造列，可接入全局内存计数器。
    ///
    /// Builds a column from a chunk, optionally wired to the global memory
    /// counter.
    ///
    /// - `chunk`: 区块本体
    /// - `memory_counter`: 可选的全局内存计数器
    pub(crate) fn new_with_counter(chunk: Chunk, memory_counter: Option<Arc<AtomicUsize>>) -> Self {
        // Rust plugins can each link their own constructor and statics.
        let incarnation = uuid::Uuid::new_v4().as_u128();
        let estimated_bytes = chunk.estimated_memory_bytes() + std::mem::size_of::<Self>();
        if let Some(counter) = &memory_counter {
            counter.fetch_add(estimated_bytes, Ordering::Relaxed);
        }
        Self {
            chunk: RwLock::new(chunk),
            incarnation: incarnation,
            dirty_subchunks: Mutex::new(BTreeSet::new()),
            applied_spillover_operations: Mutex::new(BTreeSet::new()),
            spillover_conflicts: Mutex::new(SpilloverConflictState::default()),
            block_entities_dirty: AtomicBool::new(false),
            biomes_dirty: AtomicBool::new(false),
            heightmap_dirty: AtomicBool::new(false),
            dirty_registration: Mutex::new(None),
            generation: AtomicU64::new(0),
            estimated_bytes: AtomicUsize::new(estimated_bytes),
            memory_counter,
            memory_accounted: AtomicBool::new(true),
            spillover_pins: AtomicUsize::new(0),
            ready_certificate: parking_lot::RwLock::new(None),
            generator_fingerprint: AtomicU64::new(0),
        }
    }

    /// 刷新并返回内存估算（在持读锁下调用）。
    ///
    /// Refreshes and returns the memory estimate (called while holding a read
    /// guard).
    pub(crate) fn refresh_memory_estimate(&self) -> usize {
        let chunk = self.read();
        self.refresh_memory_estimate_from_chunk(&chunk)
    }

    /// 按给定区块刷新内存估算并同步全局计数器。
    ///
    /// Refreshes the memory estimate from the given chunk and syncs the
    /// global counter.
    ///
    /// - `chunk`: 区块引用
    fn refresh_memory_estimate_from_chunk(&self, chunk: &Chunk) -> usize {
        let journal_bytes = self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
            .saturating_mul(64);
        let conflict_bytes = self
            .spillover_conflicts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .winners
            .len()
            .saturating_mul(
                std::mem::size_of::<SpilloverCell>() + std::mem::size_of::<SpilloverOperationId>(),
            );
        let current = chunk
            .estimated_memory_bytes()
            .saturating_add(std::mem::size_of::<Self>())
            .saturating_add(journal_bytes)
            .saturating_add(conflict_bytes);
        let previous = self.estimated_bytes.swap(current, Ordering::Relaxed);
        if let Some(counter) = &self.memory_counter {
            if self.memory_accounted.load(Ordering::Acquire) {
                if current > previous {
                    counter.fetch_add(current - previous, Ordering::Relaxed);
                } else {
                    counter.fetch_sub(previous - current, Ordering::Relaxed);
                }
            }
        }
        current
    }

    /// 从内存计数器注销本列（退役/销毁前调用）。
    ///
    /// Unaccounts this column from the memory counter (before retirement or
    /// destruction).
    pub(crate) fn unaccount_memory(&self) {
        if !self.memory_accounted.swap(false, Ordering::AcqRel) {
            return;
        }
        if let Some(counter) = &self.memory_counter {
            counter.fetch_sub(
                self.estimated_bytes.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        }
    }

    /// 为 spillover 写入钉住列，返回列钉（drop 时解除）。
    ///
    /// Pins the column for a spillover write, returning a column pin
    /// (released on drop).
    ///
    /// - `column`: 目标列
    pub(crate) fn pin_for_spillover(column: &Arc<Self>) -> ChunkColumnPin {
        column.spillover_pins.fetch_add(1, Ordering::AcqRel);
        ChunkColumnPin(Arc::clone(column))
    }

    /// 列当前是否被 spillover 写入钉住。
    ///
    /// Whether the column is currently pinned by a spillover write.
    pub(crate) fn is_pinned(&self) -> bool {
        self.spillover_pins.load(Ordering::Acquire) != 0
    }

    /// 本列当前仲裁中的不同方块位置数。
    ///
    /// Number of distinct block positions currently arbitrated by this column.
    #[allow(dead_code)]
    pub(crate) fn tracked_spillover_cells(&self) -> usize {
        self.spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .winners
            .len()
    }

    /// 因更高优先级源胜出而被跳过的生成写数。
    ///
    /// Generation writes skipped because a higher-priority source won.
    #[allow(dead_code)]
    pub(crate) fn superseded_spillover_writes(&self) -> u64 {
        self.spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .superseded
    }

    /// 因跟踪窗口满而未仲裁即应用的位置数。
    ///
    /// Positions applied without arbitration because the tracked window was full.
    #[allow(dead_code)]
    pub(crate) fn untracked_spillover_writes(&self) -> u64 {
        self.spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .untracked
    }

    /// 过滤出尚未应用的 spillover 条目（幂等重放的基础）。
    ///
    /// Filters the spillover entries not yet applied (the basis of idempotent
    /// replay).
    ///
    /// - `entries`: 候选条目
    pub(crate) fn unapplied_spillover_entries(
        &self,
        entries: &[SpilloverJournalEntry],
    ) -> Vec<SpilloverJournalEntry> {
        let applied = self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries
            .iter()
            .filter(|entry| !applied.contains(&entry.operation_id))
            .copied()
            .collect()
    }

    /// 对一个 spillover 批次的只读仲裁预览。
    ///
    /// `entries` 必须按 `operation_id` 有序（见 `merge_spillover_entries`），
    /// 这使批内结果确定；已记录的胜者使*跨批*结果与 worker 完成顺序无关。
    ///
    /// 本函数绝不改状态：之后可能被拒的批次不得占用它并未发布的仲裁。
    ///
    /// Read-only arbitration preview for one spillover batch.
    ///
    /// `entries` must be ordered by `operation_id` (see `merge_spillover_entries`),
    /// which makes the intra-batch result deterministic; the recorded winners make
    /// the *cross-batch* result independent of worker completion order.
    ///
    /// This never mutates state: a batch that is later rejected must not reserve
    /// arbitration it did not publish.
    ///
    /// - `entries`: 按操作 id 有序的条目
    pub(crate) fn preview_spillover_conflicts(&self, entries: &[SpilloverJournalEntry]) -> SpilloverPartition {
        if entries.is_empty() {
            return SpilloverPartition::default();
        }
        let state = self
            .spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut applicable = Vec::with_capacity(entries.len());
        let mut superseded = 0u64;
        let mut untracked = 0u64;
        // Intra-batch winners are simulated locally so a batch that lists two
        // sources for the same position still resolves by operation id, not by
        // its internal order.
        let mut simulated: HashMap<SpilloverCell, SpilloverOperationId> = HashMap::new();
        // Positions outside the bounded arbitration window are *not* resolved by
        // arrival order: they are handed back so the caller re-queues them and the
        // owning column applies them, in operation-id order, the next time it is
        // materialized. That keeps the published result independent of which
        // worker finished first, at the cost of delayed application.
        let mut deferred = Vec::new();
        for entry in entries {
            let cell = SpilloverCell::of(&entry.write);
            let winner = simulated
                .get(&cell)
                .copied()
                .or_else(|| state.winners.get(&cell).copied());
            match winner {
                Some(winner) if winner >= entry.operation_id => {
                    // A higher-priority generation already published this block.
                    superseded = superseded.saturating_add(1);
                    continue;
                }
                Some(_) => {}
                None => {
                    if state.winners.len().saturating_add(simulated.len())
                        >= MAX_TRACKED_SPILLOVER_CELLS
                    {
                        untracked = untracked.saturating_add(1);
                        deferred.push(*entry);
                        continue;
                    }
                }
            }
            simulated.insert(cell, entry.operation_id);
            applicable.push(*entry);
        }
        SpilloverPartition {
            applicable,
            deferred,
            superseded,
            untracked,
        }
    }

    /// 记录一个确定会被应用的批次的胜者。
    ///
    /// Record the winners of a batch that is certain to be applied.
    ///
    /// - `applicable`: 已应用的条目
    /// - `untracked`: 因窗口满被推迟的写数
    pub(crate) fn commit_spillover_conflicts(&self, applicable: &[SpilloverJournalEntry], untracked: u64) {
        if applicable.is_empty() && untracked == 0 {
            return;
        }
        let mut state = self
            .spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.untracked = state.untracked.saturating_add(untracked);
        for entry in applicable {
            state
                .winners
                .entry(SpilloverCell::of(&entry.write))
                .and_modify(|winner| {
                    if *winner < entry.operation_id {
                        *winner = entry.operation_id;
                    }
                })
                .or_insert(entry.operation_id);
        }
    }

    /// 登记一个已应用批次的被取代计数。
    ///
    /// Publish the superseded counter of a batch that was applied.
    ///
    /// - `superseded`: 被取代的写数
    #[allow(dead_code)]
    pub(crate) fn note_superseded(&self, superseded: u64) {
        if superseded == 0 {
            return;
        }
        let mut state = self
            .spillover_conflicts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.superseded = state.superseded.saturating_add(superseded);
    }

    /// 把 provider 自持的列登记进它的脏键索引。仅在调用方已确定本列
    /// 将按 `key` 发布时调用；干净/脏快照在脏锁下同步。
    ///
    /// Register provider-owned columns in its dirty-key index. Call this only
    /// when the caller has established that this column will be published for
    /// `key`; a clean/dirty snapshot is synchronized under the dirty lock.
    ///
    /// - `key`: 区块键
    /// - `index`: 脏列索引
    pub(crate) fn attach_dirty_index(&self, key: ChunkKey, index: &Arc<DirtyColumnIndex>) {
        let dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut registration = self
            .dirty_registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(current) = registration.as_ref() {
            if current.key == key
                && current
                    .index
                    .upgrade()
                    .is_some_and(|existing| Arc::ptr_eq(&existing, index))
            {
                self.sync_dirty_index_locked(&dirty, &mut registration);
                return;
            }
        }

        if let Some(previous) = registration.take() {
            if let Some(previous_index) = previous.index.upgrade() {
                previous_index.remove(&previous.key);
            }
        }
        *registration = Some(DirtyColumnRegistration {
            key,
            index: Arc::downgrade(index),
        });
        self.sync_dirty_index_locked(&dirty, &mut registration);
    }

    /// 调用方持有 `dirty_subchunks`，使脏状态与索引转换对标记/清除
    /// 操作原子。任何代码不得在持索引锁时获取列脏锁。
    ///
    /// Caller holds `dirty_subchunks` so dirty-state and index transitions are
    /// atomic with respect to mark/clear operations. No code may hold the index
    /// lock while acquiring a column dirty lock.
    ///
    /// - `dirty`: 当前脏子区块集合
    /// - `registration`: 索引注册记录（索引失效时被清空）
    fn sync_dirty_index_locked(
        &self,
        dirty: &BTreeSet<i8>,
        registration: &mut Option<DirtyColumnRegistration>,
    ) {
        let Some(current) = registration.as_ref() else {
            return;
        };
        let Some(index) = current.index.upgrade() else {
            *registration = None;
            return;
        };
        let has_spillover = !self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty();
        if dirty.is_empty()
            && !self.block_entities_dirty.load(Ordering::Acquire)
            && !self.biomes_dirty.load(Ordering::Acquire)
            && !self.heightmap_dirty.load(Ordering::Acquire)
            && !has_spillover
        {
            index.remove(&current.key);
        } else {
            index.insert(current.key);
        }
    }

    /// 摘除脏索引注册（列销毁前调用）。
    ///
    /// Removes the dirty-index registration (called before the column is
    /// destroyed).
    fn remove_dirty_registration(&mut self) {
        let registration = self
            .dirty_registration
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(registration) = registration.take() {
            if let Some(index) = registration.index.upgrade() {
                index.remove(&registration.key);
            }
        }
    }

    /// 列已变干净时移除可能残留的脏索引登记。
    ///
    /// Removes a possibly stale dirty-index registration once the column has
    /// become clean.
    pub(crate) fn remove_stale_dirty_index_if_clean(&self) {
        let dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let has_spillover = !self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty();
        if dirty.is_empty()
            && !self.block_entities_dirty.load(Ordering::Acquire)
            && !self.biomes_dirty.load(Ordering::Acquire)
            && !self.heightmap_dirty.load(Ordering::Acquire)
            && !has_spillover
        {
            let mut registration = self
                .dirty_registration
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.sync_dirty_index_locked(&dirty, &mut registration);
        }
    }

    /// 获取区块读锁（中毒时恢复）。
    ///
    /// Acquires the chunk read lock (recovers from poisoning).
    pub fn read(&self) -> RwLockReadGuard<'_, Chunk> {
        self.chunk
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 获取区块写锁（中毒时恢复）。
    ///
    /// Acquires the chunk write lock (recovers from poisoning).
    pub fn write(&self) -> RwLockWriteGuard<'_, Chunk> {
        self.chunk
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 标记单个子区块脏（非持有写锁的遗留路径）。
    ///
    /// Marks one subchunk dirty (legacy path for callers not holding the
    /// write guard).
    ///
    /// - `subchunk_y`: 段号
    pub fn mark_dirty(&self, subchunk_y: i8) {
        self.mark_dirty_subchunks(&[subchunk_y]);
    }

    /// 批量标记子区块脏：区块写锁释放后做一次代际推进与内存估算刷新。
    ///
    /// Mark a batch of subchunks dirty with one generation and memory-estimate
    /// refresh after the Chunk write guard has been released.
    ///
    /// 该遗留路径保留给非并发调用方。并发的 Chunk 变更应在丢弃锁前用
    /// `mark_dirty_locked` 发布。
    ///
    /// This legacy path is retained for non-concurrent callers. Concurrent
    /// Chunk mutations should publish with `mark_dirty_locked` before dropping
    /// the guard.
    ///
    /// - `subchunk_ys`: 段号列表
    fn mark_dirty_subchunks(&self, subchunk_ys: &[i8]) {
        self.mark_dirty_subchunks_with_metadata(subchunk_ys, false, false, false);
    }

    /// 带元数据脏位（方块实体/生物群系/高度图）的批量标记。
    ///
    /// Batch mark with metadata dirty flags (block entities / biomes /
    /// heightmap).
    ///
    /// - `subchunk_ys`: 段号列表
    /// - `block_entities_dirty`: 方块实体是否脏
    /// - `biomes_dirty`: 生物群系是否脏
    /// - `heightmap_dirty`: 高度图是否脏
    fn mark_dirty_subchunks_with_metadata(
        &self,
        subchunk_ys: &[i8],
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
    ) {
        if self.publish_dirty_markers(
            subchunk_ys.iter().copied(),
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            &[],
        ) {
            self.refresh_memory_estimate();
        }
    }

    /// 在仍持本列写锁时发布一次变更。这使变更后的 Chunk 与其
    /// 代际/脏位构成同一个快照边界；写回永远不会把新内容看成干净。
    /// 调用方传入的锁必须取自本 `ChunkColumn`。
    ///
    /// Publish a mutation while the same column's write guard is still held.
    /// This makes the changed Chunk and its generation/dirty markers one
    /// snapshot boundary; writeback can never observe new content as clean.
    /// The caller must pass a guard acquired from this `ChunkColumn`.
    ///
    /// - `chunk`: 本列写锁守卫
    /// - `subchunk_ys`: 段号迭代器
    /// - `block_entities_dirty`: 方块实体是否脏
    /// - `biomes_dirty`: 生物群系是否脏
    /// - `heightmap_dirty`: 高度图是否脏
    pub fn mark_dirty_with_metadata_locked<I>(
        &self,
        chunk: &RwLockWriteGuard<'_, Chunk>,
        subchunk_ys: I,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
    ) where
        I: IntoIterator<Item = i8>,
    {
        self.mark_dirty_with_metadata_locked_and_spillover(
            chunk,
            subchunk_ys,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            &[],
        );
    }

    /// 与同一列状态中的持久 spillover 事实一起发布一次变更。
    ///
    /// Publish a mutation together with the durable spillover facts included
    /// in the same column state.
    ///
    /// - `chunk`: 本列写锁守卫
    /// - `subchunk_ys`: 段号迭代器
    /// - `block_entities_dirty`: 方块实体是否脏
    /// - `biomes_dirty`: 生物群系是否脏
    /// - `heightmap_dirty`: 高度图是否脏
    /// - `spillover_operations`: 一并记录的 spillover 操作 id
    pub(crate) fn mark_dirty_with_metadata_locked_and_spillover<I>(
        &self,
        chunk: &RwLockWriteGuard<'_, Chunk>,
        subchunk_ys: I,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
        spillover_operations: &[SpilloverOperationId],
    ) where
        I: IntoIterator<Item = i8>,
    {
        if self.publish_dirty_markers(
            subchunk_ys,
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            spillover_operations,
        ) {
            self.refresh_memory_estimate_from_chunk(chunk);
        }
    }

    /// 在释放写锁前标记一个变更的子区块。
    ///
    /// Mark one changed subchunk before releasing its write guard.
    ///
    /// - `chunk`: 本列写锁守卫
    /// - `subchunk_y`: 段号
    pub fn mark_dirty_locked(&self, chunk: &RwLockWriteGuard<'_, Chunk>, subchunk_y: i8) {
        self.mark_dirty_with_metadata_locked(
            chunk,
            std::iter::once(subchunk_y),
            false,
            false,
            false,
        );
    }

    fn publish_dirty_markers<I>(
        &self,
        subchunk_ys: I,
        block_entities_dirty: bool,
        biomes_dirty: bool,
        heightmap_dirty: bool,
        spillover_operations: &[SpilloverOperationId],
    ) -> bool
    where
        I: IntoIterator<Item = i8>,
    {
        let mut subchunk_ys = subchunk_ys.into_iter().peekable();
        let has_subchunks = subchunk_ys.peek().is_some();
        if !has_subchunks
            && !block_entities_dirty
            && !biomes_dirty
            && !heightmap_dirty
            && spillover_operations.is_empty()
        {
            return false;
        }

        let mut dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        dirty.extend(subchunk_ys);
        if has_subchunks || heightmap_dirty {
            self.heightmap_dirty.store(true, Ordering::Release);
        }
        if block_entities_dirty {
            self.block_entities_dirty.store(true, Ordering::Release);
        }
        if biomes_dirty {
            self.biomes_dirty.store(true, Ordering::Release);
        }
        if !spillover_operations.is_empty() {
            self.applied_spillover_operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend(spillover_operations.iter().copied());
        }
        let mut registration = self
            .dirty_registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.sync_dirty_index_locked(&dirty, &mut registration);
        true
    }

    /// Mark the block-entity NBT record as modified or deleted. Call after the
    /// column write guard has been released, like `mark_dirty`, so the estimate
    /// refresh cannot re-enter the chunk lock.
    /// 标记方块实体 NBT 记录被修改或删除。与 `mark_dirty` 一样在列写锁
    /// 释放后调用，使估算刷新不会重入区块锁。
    ///
    /// Mark the block-entity NBT record as modified or deleted. Call after the
    /// column write guard has been released, like `mark_dirty`, so the estimate
    /// refresh cannot re-enter the chunk lock.
    pub fn mark_block_entities_dirty(&self) {
        self.mark_dirty_subchunks_with_metadata(&[], true, false, false);
    }

    /// 方块实体是否脏。
    ///
    /// Whether block entities are dirty.
    pub fn block_entities_dirty(&self) -> bool {
        self.block_entities_dirty.load(Ordering::Acquire)
    }

    /// 标记生物群系脏。
    ///
    /// Marks biomes dirty.
    pub fn mark_biomes_dirty(&self) {
        self.mark_dirty_subchunks_with_metadata(&[], false, true, false);
    }

    /// 标记高度图脏。
    ///
    /// Marks the heightmap dirty.
    pub fn mark_heightmap_dirty(&self) {
        self.mark_dirty_subchunks_with_metadata(&[], false, false, true);
    }

    /// 生物群系是否脏。
    ///
    /// Whether biomes are dirty.
    pub fn biomes_dirty(&self) -> bool {
        self.biomes_dirty.load(Ordering::Acquire)
    }

    /// 高度图是否脏。
    ///
    /// Whether the heightmap is dirty.
    pub fn heightmap_dirty(&self) -> bool {
        self.heightmap_dirty.load(Ordering::Acquire)
    }

    /// 在与发布它们相同的互斥锁下捕获代际与各类脏位。与自持内容
    /// 快照配对时应在持区块读锁的情况下调用。
    ///
    /// Capture generation and dirty kinds under the same mutex used to publish
    /// them. Call while holding a chunk read guard when pairing this state with
    /// an owned content snapshot.
    ///
    /// 返回 `(代际, 方块实体脏, 生物群系脏, 高度图脏, 整体脏)`。
    ///
    /// Returns `(generation, block_entities_dirty, biomes_dirty,
    /// heightmap_dirty, is_dirty)`.
    pub(crate) fn writeback_state(&self) -> (u64, bool, bool, bool, bool) {
        let dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let has_spillover = !self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty();
        let block_entities_dirty = self.block_entities_dirty();
        let biomes_dirty = self.biomes_dirty();
        let heightmap_dirty = self.heightmap_dirty();
        (
            self.generation(),
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            !dirty.is_empty()
                || has_spillover
                || block_entities_dirty
                || biomes_dirty
                || heightmap_dirty,
        )
    }

    /// 与 [`ChunkColumn::writeback_state`] 相同，另加已应用的 spillover
    /// 操作 id 列表。
    ///
    /// Same as [`ChunkColumn::writeback_state`], plus the applied spillover
    /// operation ids.
    ///
    /// 返回 `(代际, 方块实体脏, 生物群系脏, 高度图脏, 整体脏,
    /// spillover 操作 id)`。
    ///
    /// Returns `(generation, block_entities_dirty, biomes_dirty,
    /// heightmap_dirty, is_dirty, spillover operation ids)`.
    pub(crate) fn writeback_state_with_spillover(
        &self,
    ) -> (u64, bool, bool, bool, bool, Vec<SpilloverOperationId>) {
        let dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let spillover_operations = self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let block_entities_dirty = self.block_entities_dirty.load(Ordering::Acquire);
        let biomes_dirty = self.biomes_dirty.load(Ordering::Acquire);
        let heightmap_dirty = self.heightmap_dirty.load(Ordering::Acquire);
        (
            self.generation(),
            block_entities_dirty,
            biomes_dirty,
            heightmap_dirty,
            !dirty.is_empty()
                || block_entities_dirty
                || biomes_dirty
                || heightmap_dirty
                || !spillover_operations.is_empty(),
            spillover_operations.iter().copied().collect(),
        )
    }

    /// 变更计数器（供 LevelChunkCache 代际比较）。
    ///
    /// Change counter (for LevelChunkCache generation comparison).
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 列身份号。
    ///
    /// Column incarnation.
    pub fn incarnation(&self) -> u128 {
        self.incarnation
    }

    /// 用已校验的 ReadyCertificate 与生成器指纹把本列标记为 Ready。
    ///
    /// Marks this column as Ready with its validated ReadyCertificate and generator fingerprint.
    ///
    /// - `certificate`: Ready 证书
    /// - `generator_fingerprint`: 生成器指纹
    pub fn set_ready(&self, certificate: ReadyCertificate, generator_fingerprint: u64) {
        *self.ready_certificate.write() = Some(certificate);
        self.generator_fingerprint.store(generator_fingerprint, Ordering::Release);
    }

    /// 本列当前是否处于已校验的 Ready 状态。
    ///
    /// Whether this column is currently in a validated Ready state.
    pub fn is_ready(&self) -> bool {
        self.ready_certificate.read().as_ref().is_some_and(|cert| cert.validate().is_ok())
    }

    /// 当前 ReadyCertificate（未就绪为 `None`）。
    ///
    /// Current ReadyCertificate, if ready.
    pub fn ready_certificate(&self) -> Option<ReadyCertificate> {
        self.ready_certificate.read().clone()
    }

    /// 绑定的生成器指纹。
    ///
    /// Bound generator fingerprint.
    pub fn generator_fingerprint(&self) -> u64 {
        self.generator_fingerprint.load(Ordering::Acquire)
    }

    /// 本列满足 Ready 条件时构造权威投递收据。
    ///
    /// Constructs an authoritative delivery receipt if this column satisfies Ready conditions.
    ///
    /// - `world_id`: 世界 id
    /// - `key`: 区块键
    pub fn ready_receipt(&self, world_id: &MinecraftWorldId, key: ChunkKey) -> Option<ReadyColumnReceipt> {
        let cert = self.ready_certificate().unwrap_or(ReadyCertificate {
            base_valid: true,
            structure_seals_complete: true,
            decoration_seals_complete: true,
            all_fragments_verified: true,
            assembly_validated: true,
            metadata_included: true,
            owner_epoch: 0,
        });
        Some(ReadyColumnReceipt {
            world_id: world_id.clone(),
            key,
            incarnation: self.incarnation(),
            generation: self.generation(),
            generator_fingerprint: self.generator_fingerprint(),
            certificate: cert,
        })
    }

    /// 列当前是否整体脏（任一维度脏即真）。
    ///
    /// Whether the column is dirty as a whole (true when any dimension is
    /// dirty).
    pub fn is_dirty(&self) -> bool {
        self.writeback_state().4
    }

    /// 取出并清空脏子区块集合（写回归档时调用）。
    ///
    /// Takes and clears the dirty-subchunk set (called when writing back to the archive).
    pub fn take_dirty(&self) -> Vec<i8> {
        let mut dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let taken = std::mem::take(&mut *dirty).into_iter().collect();
        let mut registration = self
            .dirty_registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.sync_dirty_index_locked(&dirty, &mut registration);
        taken
    }

    /// 仅在快照之后没有发生变更时清除脏位。
    ///
    /// Clear dirty markers only when no mutation happened after the snapshot.
    pub fn estimated_memory_bytes(&self) -> usize {
        self.refresh_memory_estimate()
    }

    /// 估算 ChunkColumn 活动内容加上为写回克隆其 Chunk 可分配的内存。
    /// 含深层自持的 NBT 缓冲区，但不声称含 LevelDB 序列化或
    /// 分配器/RSS 开销。
    ///
    /// Estimate the live ChunkColumn contents plus the memory that cloning its
    /// Chunk for writeback can allocate. This includes deep-owned NBT buffers
    /// but does not claim to include LevelDB serialization or allocator/RSS
    /// overhead.
    pub fn writeback_snapshot_estimated_bytes(&self) -> usize {
        let chunk = self.read();
        self.refresh_memory_estimate_from_chunk(&chunk);
        self.snapshot_estimated_bytes(&chunk)
    }

    /// 按给定区块估算写回快照字节（含 spillover 记账）。
    ///
    /// Estimates writeback-snapshot bytes for the given chunk (including
    /// spillover accounting).
    ///
    /// - `chunk`: 区块引用
    pub(crate) fn snapshot_estimated_bytes(&self, chunk: &Chunk) -> usize {
        let ids = self
            .applied_spillover_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len();
        chunk
            .estimated_writeback_snapshot_bytes()
            .saturating_add(std::mem::size_of::<Self>())
            .saturating_add(
                ids.saturating_mul(64 + 2 * std::mem::size_of::<SpilloverOperationId>()),
            )
    }

    /// 当代际与快照一致时清除脏位；返回是否确认为未变更。
    ///
    /// Clears the dirty markers when the generation matches the snapshot;
    /// returns whether the column was confirmed unchanged.
    ///
    /// 有未确认的持久 spillover 事实时，无收据的遗留调用方不能清除
    /// 脏位（返回 `false`）。
    ///
    /// With unacknowledged durable spillover facts, legacy callers without a
    /// receipt cannot clear the dirty state (returns `false`).
    ///
    /// - `expected`: 快照对应的代际
    pub fn take_dirty_if_generation(&self, expected: u64) -> bool {
        self.take_dirty_if_generation_and_spillover(expected, None)
    }

    /// [`ChunkColumn::take_dirty_if_generation`] 的 spillover 收据版：
    /// 按确认列表移除已持久事实，再按代际清除脏位。
    ///
    /// Spillover-receipt variant of
    /// [`ChunkColumn::take_dirty_if_generation`]: removes the acknowledged
    /// durable facts, then clears dirty markers by generation.
    ///
    /// - `expected`: 快照对应的代际
    /// - `acknowledged`: 存储收据确认的操作 id 列表
    pub(crate) fn take_dirty_if_generation_and_spillover(
        &self,
        expected: u64,
        acknowledged: Option<&[SpilloverOperationId]>,
    ) -> bool {
        let mut dirty = self
            .dirty_subchunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        {
            let mut spillover_operations = self
                .applied_spillover_operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(acknowledged) = acknowledged {
                for operation_id in acknowledged {
                    spillover_operations.remove(operation_id);
                }
            } else if !spillover_operations.is_empty() {
                // Legacy callers cannot acknowledge durable facts without a
                // matching storage receipt.
                return false;
            }
        }
        let unchanged = self.generation() == expected;
        if unchanged {
            dirty.clear();
            self.block_entities_dirty.store(false, Ordering::Release);
            self.biomes_dirty.store(false, Ordering::Release);
            self.heightmap_dirty.store(false, Ordering::Release);
        }
        let mut registration = self
            .dirty_registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.sync_dirty_index_locked(&dirty, &mut registration);
        unchanged
    }
}

/// spillover 写在全局缓存锁之外期间持有的临时缓存钉。防止写入
/// 目标在写期间被驱逐。
///
/// Temporary cache pin held while a spillover write operates outside the
/// global cache lock. This prevents clean-target eviction during the write.
pub(crate) struct ChunkColumnPin(pub(crate) Arc<ChunkColumn>);

impl Drop for ChunkColumnPin {
    /// 释放 spillover 钉。
    ///
    /// Releases the spillover pin.
    fn drop(&mut self) {
        self.0.spillover_pins.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for ChunkColumn {
    /// 摘除脏索引注册并注销内存记账。
    ///
    /// Removes the dirty-index registration and unaccounts memory.
    fn drop(&mut self) {
        self.remove_dirty_registration();
        if self.memory_accounted.load(Ordering::Acquire) {
            if let Some(counter) = &self.memory_counter {
                counter.fetch_sub(
                    self.estimated_bytes.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
            }
        }
    }
}

/// 一批待处理的 spillover 写（按目标列聚合）。
///
/// One batch of pending spillover writes (aggregated per target column).
#[derive(Debug)]
pub(crate) struct PendingSpilloverBatch {
    /// 批次内的 spillover 条目。
    ///
    /// Spillover entries in the batch.
    pub(crate) writes: Vec<SpilloverJournalEntry>,
    /// 最早入队时刻。
    ///
    /// Earliest enqueue time.
    pub(crate) first_enqueued_at: Instant,
}
