use crate::chunk::ChunkPosition;

/// 所有世界存储后端共用的稳定键。
///
/// A stable key shared by all world storage backends.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChunkKey {
    /// 维度 id。
    ///
    /// Dimension id.
    pub dimension: i32,
    /// 区块列坐标。
    ///
    /// Chunk column coordinates.
    pub position: ChunkPosition,
}

impl ChunkKey {
    /// 由维度 id 与列坐标构造键。
    ///
    /// Builds a key from a dimension id and column coordinates.
    ///
    /// - `dimension`: 维度 id
    /// - `position`: 区块列坐标
    pub const fn new(dimension: i32, position: ChunkPosition) -> Self {
        Self {
            dimension,
            position,
        }
    }
}

/// 一次已生成 spillover 事实的稳定身份。
///
/// Stable identity for one generated spillover fact.
///
/// 源列与确定性结果序号在加载任务重试时保持不变。序号在路由前分配，
/// 且是持久化键的一部分，因此重放不会为同一已接受事实制造第二份拷贝。
///
/// The source column and deterministic result ordinal remain stable when a
/// load job is retried.  The ordinal is assigned before routing and is part of
/// the persistence key, so a replay cannot create a second copy of the same
/// accepted fact.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SpilloverOperationId {
    /// 生成该事实的源区块键。
    ///
    /// Source chunk key that produced the fact.
    pub source: ChunkKey,
    /// 结果序号（同源内确定）。
    ///
    /// Result ordinal (deterministic within one source).
    pub ordinal: u32,
}

impl SpilloverOperationId {
    /// 由源键与序号构造操作 id。
    ///
    /// Builds an operation id from a source key and an ordinal.
    ///
    /// - `source`: 源区块键
    /// - `ordinal`: 结果序号
    pub const fn new(source: ChunkKey, ordinal: u32) -> Self {
        Self { source, ordinal }
    }
}
