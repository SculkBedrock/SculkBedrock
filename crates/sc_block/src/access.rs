//! 只读方块访问。
//!
//! Read-only block access.
//!
//! 读取经由 `WorldChunkProvider` 的 `ChunkColumn` 缓存
//! （命中 = 缓存读锁 + `Arc` 克隆 + 列读锁，无数据拷贝）。
//! 写入见 [`crate::write`]（单写者队列）。
//!
//! Reads go through the `WorldChunkProvider` `ChunkColumn` cache
//! (cache hit = cache read lock + `Arc` clone + column read lock, no data copy).
//! For writes see [`crate::write`] (single-writer queue).

use sc_world::chunk::BlockRuntimeId;
use sc_world::storage::WorldStorageError;
use sc_world::world::MinecraftWorld;

use crate::position::BlockPosition;

/// 只读方块读取 trait（世界侧实现，逻辑侧只依赖此接口）。
///
/// Read-only block access trait (implemented by the world side; logic depends only
/// on this interface).
pub trait BlockRead {
    /// 读取一个方块（layer 0）。`Ok(None)` = 区块缺失或 y 越界；
    /// `Ok(Some(air))` = 已生成区块内的空气。
    ///
    /// Reads one block (layer 0). `Ok(None)` = chunk missing or y out of range;
    /// `Ok(Some(air))` = air inside a generated chunk.
    ///
    /// - `position`: 目标方块坐标
    fn get_block(
        &self,
        position: BlockPosition,
    ) -> Result<Option<BlockRuntimeId>, WorldStorageError>;
}

impl BlockRead for MinecraftWorld {
    /// 读取一个方块：先查垂直边界，再经区块缓存定位所在区块并读取 layer 0。
    ///
    /// Reads one block: checks vertical bounds first, then locates the owning chunk
    /// via the chunk cache and reads layer 0.
    ///
    /// - `position`: 目标方块坐标
    fn get_block(
        &self,
        position: BlockPosition,
    ) -> Result<Option<BlockRuntimeId>, WorldStorageError> {
        let (min_y, max_y) = self.vertical_bounds();
        if position.y < min_y || position.y > max_y {
            return Ok(None);
        }
        let Some(column) = self.load_chunk(position.chunk_position())? else {
            return Ok(None);
        };
        let chunk = column.read();
        Ok(chunk.block_at(position.local_x(), position.y, position.local_z()))
    }
}

#[cfg(test)]
mod tests {
    use sc_world::block_dictionary::air_runtime_id;
    use sc_world::chunk::{
        BlockRuntimeId, Chunk, ChunkPosition, LocalBlockPosition, PalettedBlockStorage, SubChunk,
        SubChunkIndex, SUBCHUNK_VOLUME,
    };

    fn test_chunk() -> Chunk {
        let mut chunk = Chunk::empty(ChunkPosition::new(0, 0), 0, -64, 319);
        // Segment 0 (y -64..-49): two-entry palette, block (x=1, y=-63, z=2) points at entry 1.
        let mut indices = vec![0u16; SUBCHUNK_VOLUME];
        let local = LocalBlockPosition::new(1, 1, 2).unwrap();
        indices[local.linear_index()] = 1;
        chunk.subchunks[0] = SubChunk {
            index: SubChunkIndex::new(-4),
            layers: vec![PalettedBlockStorage::from_indices(
                vec![BlockRuntimeId(100), BlockRuntimeId(200)],
                &indices,
            )],
        };
        chunk
    }

    #[test]
    fn reads_paletted_block_in_bedrock_index_order() {
        let chunk = test_chunk();
        assert_eq!(chunk.block_at(1, -63, 2), Some(BlockRuntimeId(200)));
        assert_eq!(chunk.block_at(0, -64, 0), Some(BlockRuntimeId(100)));
    }

    #[test]
    fn empty_section_reads_as_air() {
        let chunk = test_chunk();
        assert_eq!(
            chunk.block_at(5, 100, 5),
            Some(BlockRuntimeId(air_runtime_id()))
        );
    }

    #[test]
    fn out_of_bounds_y_is_none() {
        let chunk = test_chunk();
        assert_eq!(chunk.block_at(0, -65, 0), None);
        assert_eq!(chunk.block_at(0, 320, 0), None);
    }
}
