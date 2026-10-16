use crate::chunk::ChunkPosition;

/// A stable key shared by all world storage backends.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChunkKey {
    pub dimension: i32,
    pub position: ChunkPosition,
}

impl ChunkKey {
    pub const fn new(dimension: i32, position: ChunkPosition) -> Self {
        Self {
            dimension,
            position,
        }
    }
}

/// Stable identity for one generated spillover fact.
///
/// The source column and deterministic result ordinal remain stable when a
/// load job is retried.  The ordinal is assigned before routing and is part of
/// the persistence key, so a replay cannot create a second copy of the same
/// accepted fact.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SpilloverOperationId {
    pub source: ChunkKey,
    pub ordinal: u32,
}

impl SpilloverOperationId {
    pub const fn new(source: ChunkKey, ordinal: u32) -> Self {
        Self { source, ordinal }
    }
}
