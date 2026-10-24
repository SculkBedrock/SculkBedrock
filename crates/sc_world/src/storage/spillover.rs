use super::{ChunkKey, MAX_TRACKED_SPILLOVER_CELLS, SpilloverOperationId, WorldStorageError};
use crate::chunk::Chunk;
use sc_log::t_log;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

/// `generate_chunk_with_spillover` 的结果：区块本体 + 跨区块 spillover 写。
///
/// Result of `generate_chunk_with_spillover`: the chunk itself plus cross-chunk spillover writes.
pub struct GeneratedChunk {
    /// 生成的区块。
    ///
    /// The generated chunk.
    pub chunk: Chunk,
    /// spillover 写（`key` 是目标区块；`x/y/z` 为世界坐标）。
    ///
    /// Spillover writes (`key` is the target chunk; `x/y/z` are world coordinates).
    pub spillover: Vec<BlockSpilloverWrite>,
}

/// 即将构造一次生成的区块的有界 spillover 结果的 worker 的
/// 计数/字节准入。
///
/// Count/byte admission for a worker that is about to construct one generated
/// chunk's bounded spillover result.
pub(crate) struct SpilloverGenerationPermit {
    /// 预留字节计数器（drop 时归还）。
    ///
    /// Reserved-byte counter (returned on drop).
    pub(crate) reserved_bytes: Arc<AtomicUsize>,
    /// 本许可预留的字节数。
    ///
    /// Bytes reserved by this permit.
    pub(crate) bytes: usize,
}

impl Drop for SpilloverGenerationPermit {
    /// 释放时归还预留字节。
    ///
    /// Returns the reserved bytes on drop.
    fn drop(&mut self) {
        self.reserved_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// 一次跨区块的 spillover 方块写。
///
/// One cross-chunk spillover block write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockSpilloverWrite {
    /// 目标区块（含维度）。
    ///
    /// Target chunk (includes dimension).
    pub key: ChunkKey,
    /// 世界坐标 x。
    ///
    /// World x.
    pub x: i32,
    /// 世界坐标 y。
    ///
    /// World y.
    pub y: i32,
    /// 世界坐标 z。
    ///
    /// World z.
    pub z: i32,
    /// 方块层（0 = 主层，1 = 额外层）。
    ///
    /// Block layer (0 = main layer, 1 = extra layer).
    pub layer: usize,
    /// 方块状态 id。
    ///
    /// Block runtime id.
    pub block: crate::chunk::BlockRuntimeId,
}

/// 一条 spillover 写及其持久的、重试稳定的操作 id。
///
/// A spillover write together with its durable, retry-stable operation id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpilloverJournalEntry {
    /// 操作 id。
    ///
    /// Operation id.
    pub operation_id: SpilloverOperationId,
    /// spillover 写内容。
    ///
    /// The spillover write.
    pub write: BlockSpilloverWrite,
}

/// 一次生成的源区块提交结果：源区块、收到的 spillover 与是否新提交。
///
/// Result of committing one generated source chunk: the source chunk, its
/// incoming spillover and whether it was newly committed.
pub struct GeneratedChunkCommit {
    /// 源区块。
    ///
    /// Source chunk.
    pub source: Chunk,
    /// 收到的 spillover 条目。
    ///
    /// Incoming spillover entries.
    pub incoming: Vec<SpilloverJournalEntry>,
    /// 是否为新提交（重放时为 `false`）。
    ///
    /// Whether this was a new commit (`false` on replay).
    pub newly_committed: bool,
}

/// 一次区块生成请求：目标键 + 竖直范围。
///
/// One chunk generation request: target key plus vertical bounds.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ChunkGenerationRequest {
    /// 目标区块键。
    ///
    /// Target chunk key.
    pub key: ChunkKey,
    /// 世界底部 y。
    ///
    /// World bottom y.
    pub min_y: i32,
    /// 世界顶部 y。
    ///
    /// World top y.
    pub max_y: i32,
}

/// spillover 仲裁结果的限流诊断。
///
/// Rate-limited diagnostic for spillover arbitration outcomes.
///
/// `superseded` 是既定的生成冲突策略（操作 id 最大者胜），不是数据
/// 丢失。`deferred` 表示该列的有界仲裁窗口已满：这些写被重新排队，
/// 在列下次物化时按操作 id 顺序应用，因此任何位置都不会按到达
/// 顺序裁决。
///
/// `superseded` is a defined generation conflict policy (largest operation id
/// wins), not data loss. `deferred` means the column's bounded arbitration window
/// was full: those writes are re-queued and applied in operation-id order when
/// the column is next materialized, so no position is ever resolved by arrival
/// order.
///
/// - `target`: 目标区块键（未知为 `None`）
/// - `superseded`: 被更高优先级源取代的写数
/// - `untracked`: 因窗口满而被推迟的写数
pub(crate) fn report_spillover_arbitration(target: Option<ChunkKey>, superseded: u64, untracked: u64) {
    static TOTAL_SUPERSEDED: AtomicU64 = AtomicU64::new(0);
    static TOTAL_UNTRACKED: AtomicU64 = AtomicU64::new(0);
    static UNTRACKED_WARNED: AtomicBool = AtomicBool::new(false);
    let total_superseded = TOTAL_SUPERSEDED.fetch_add(superseded, Ordering::Relaxed) + superseded;
    let total_untracked = TOTAL_UNTRACKED.fetch_add(untracked, Ordering::Relaxed) + untracked;
    if total_untracked > 0 && !UNTRACKED_WARNED.swap(true, Ordering::AcqRel) {
        log::warn!(
            "{}",
            t_log!(
                "console.world.spillover_window",
                max = MAX_TRACKED_SPILLOVER_CELLS,
                deferred = total_untracked,
                target = format!("{target:?}")
            )
        );
    }
    if superseded > 0 {
        log::debug!(
            "[spillover] {} generation write(s) superseded by a higher-priority source \
             (target={target:?}, total_superseded={total_superseded})",
            superseded
        );
    }
}

/// 按操作 id 合并 spillover 条目：同 id 重放必须字节级一致，
/// 否则视为数据损坏；结果按操作 id 排序。
///
/// Merges spillover entries by operation id: a replay of the same id must be
/// byte-identical, otherwise it is treated as corruption; the result is
/// sorted by operation id.
///
/// - `entries`: 待合并条目
///
/// 错误：同一操作 id 载荷不同时返回
/// [`WorldStorageError::Corrupt`]。
///
/// Errors: returns [`WorldStorageError::Corrupt`] when the same operation id
/// carries a different payload.
pub(crate) fn merge_spillover_entries(
    entries: impl IntoIterator<Item = SpilloverJournalEntry>,
) -> Result<Vec<SpilloverJournalEntry>, WorldStorageError> {
    let mut by_operation = HashMap::new();
    for entry in entries {
        if let Some(existing) = by_operation.get(&entry.operation_id) {
            if existing != &entry {
                return Err(WorldStorageError::corrupt(format!(
                    "spillover operation id reused with different payload: {:?}",
                    entry.operation_id
                )));
            }
            continue;
        }
        by_operation.insert(entry.operation_id, entry);
    }
    let mut entries = by_operation.into_values().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.operation_id);
    Ok(entries)
}
