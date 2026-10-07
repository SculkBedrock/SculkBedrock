//! Read-only block access.
//!
//! Reads go through the `WorldChunkProvider` `ChunkColumn` cache
//! (cache hit = cache read lock + `Arc` clone + column read lock, no data copy).
//! For writes see [`crate::write`] (single-writer queue).

use sc_world::chunk::BlockRuntimeId;
use sc_world::storage::WorldStorageError;
use sc_world::world::MinecraftWorld;

use crate::position::BlockPosition;

pub trait BlockRead {
    /// Reads one block (layer 0). `Ok(None)` = chunk missing or y out of range;
    /// `Ok(Some(air))` = air inside a generated chunk.
    fn get_block(
        &self,
        position: BlockPosition,
    ) -> Result<Option<BlockRuntimeId>, WorldStorageError>;
}

impl BlockRead for MinecraftWorld {
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
