//! Chunk state enum + worldgen-chunk adapter (generation subset).
//!
//! Layout notes:
//! - [`ChunkState`] derives `Ord` with ordinals matching the reference:
//!   New=0 < Started=1 < Generated=2 < Populated=3 < Finished=4.
//! - The chunk interface is very large (entities/block entities/light/
//!   scheduler/borders/...); the generation pipeline only needs a small
//!   subset (coordinates/blocks/biomes/heightmap/state/sections).
//!   [`WorldgenChunk`] implements just that subset around
//!   `sc_world::chunk::Chunk`.
//! - Heightmap array maps to Rust `[i16; 256]`.
//! - Batch processing runs as direct calls (single-threaded generation
//!   needs no synchronization wrapper).
//! - Block/biome index: `(x << 8) | (z << 4) | y` (XZY, matching
//!   `LocalBlockPosition::linear_index`).

use std::sync::Arc;
use sc_world::chunk::{BlockRuntimeId, Chunk, PalettedBiomeStorage, SubChunk, SUBCHUNK_SIZE};

// ---------------------------------------------------------------------------
// ChunkState
// ---------------------------------------------------------------------------

/// Chunk generation state.
///
/// Ordinals match the reference `ordinal()`, so a derived `Ord` allows
/// direct comparisons like `state >= ChunkState::Generated`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChunkState {
    /// Fresh chunk (ordinal 0).
    New,
    /// Generation started (ordinal 1).
    Started,
    /// Terrain generated (ordinal 2).
    Generated,
    /// Populated with features (ordinal 3).
    Populated,
    /// Fully finished (ordinal 4).
    Finished,
}

impl ChunkState {
    /// Returns whether the chunk may be sent (Populated or Finished).
    pub fn can_send(self) -> bool {
        self >= ChunkState::Populated
    }
}

// ---------------------------------------------------------------------------
// WorldgenChunk (generation-subset chunk adapter)
// ---------------------------------------------------------------------------

/// Generation-subset chunk adapter around `sc_world::chunk::Chunk`.
///
/// Additionally maintains:
/// - `state: ChunkState` — chunk generation state;
/// - `heightmap: [i16; 256]` — column heightmap.
///
/// Block reads/writes and section access delegate directly to `Chunk`'s
/// existing API.
pub struct WorldgenChunk {
    chunk: Chunk,
    state: ChunkState,
    heightmap: [i16; 256],
}

impl WorldgenChunk {
    /// Wraps a `Chunk` with initial state `New` and a zeroed heightmap.
    pub fn new(chunk: Chunk) -> Self {
        let heightmap = chunk
            .data_3d_heightmap
            .as_ref()
            .map(|bytes| {
                std::array::from_fn(|index| {
                    i16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]])
                })
            })
            .unwrap_or([0; 256]);
        Self {
            chunk,
            state: ChunkState::New,
            heightmap,
        }
    }

    // --- Coordinates / dimensions ---

    /// Returns the chunk X coordinate.
    pub fn x(&self) -> i32 {
        self.chunk.position.x
    }

    /// Returns the chunk Z coordinate.
    pub fn z(&self) -> i32 {
        self.chunk.position.z
    }

    /// Moves the chunk to new chunk coordinates.
    pub fn set_position(&mut self, x: i32, z: i32) {
        self.chunk.position.x = x;
        self.chunk.position.z = z;
    }

    pub fn min_y(&self) -> i32 {
        self.chunk.min_y
    }

    pub fn max_y(&self) -> i32 {
        self.chunk.max_y
    }

    // --- State machine ---

    /// Returns the current generation state.
    pub fn state(&self) -> ChunkState {
        self.state
    }

    /// Sets the generation state.
    pub fn set_state(&mut self, state: ChunkState) {
        self.state = state;
    }

    /// Returns whether terrain generation has completed.
    pub fn is_generated(&self) -> bool {
        self.state >= ChunkState::Generated
    }

    /// Returns whether feature population has completed.
    pub fn is_populated(&self) -> bool {
        self.state >= ChunkState::Populated
    }

    /// Returns whether the chunk is fully finished.
    pub fn is_finished(&self) -> bool {
        self.state == ChunkState::Finished
    }

    /// Marks the chunk generated.
    pub fn set_generated(&mut self) {
        self.state = ChunkState::Generated;
    }

    /// Marks the chunk populated.
    pub fn set_populated(&mut self) {
        self.state = ChunkState::Populated;
    }

    // --- Block reads/writes ---

    /// Reads the block state at a local position and layer.
    ///
    /// Out-of-range y returns the air runtime id.
    pub fn block_state(&self, x: u8, y: i32, z: u8, layer: usize) -> BlockRuntimeId {
        self.chunk
            .block_at_layer(layer, x, y, z)
            .unwrap_or(BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()))
    }

    /// Writes a block state at a local position and layer.
    pub fn set_block_state(&mut self, x: u8, y: i32, z: u8, layer: usize, block: BlockRuntimeId) {
        self.chunk.set_block_at(layer, x, y, z, block);
    }

    /// Atomic get-then-set (single-threaded generation calls it directly).
    pub fn get_and_set_block_state(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        layer: usize,
        block: BlockRuntimeId,
    ) -> BlockRuntimeId {
        let old = self.block_state(x, y, z, layer);
        self.set_block_state(x, y, z, layer, block);
        old
    }

    // --- Biome reads/writes ---

    /// Reads the biome id at a local position (0 when biomes are
    /// uninitialized).
    pub fn biome_id(&self, x: u8, y: i32, z: u8) -> i32 {
        let section = match self.chunk.section_of(y) {
            Some(idx) => idx,
            None => return 0,
        };
        let local_y = ((y - self.chunk.min_y) % SUBCHUNK_SIZE) as u8;
        let index = ((x as usize) << 8) | ((z as usize) << 4) | local_y as usize;
        self.chunk
            .biomes
            .get(section)
            .and_then(|storage| storage.get(index))
            .unwrap_or(0) as i32
    }

    /// Writes a biome id at a local position, lazily initializing biome
    /// storage first (one `Single(0)` per section).
    pub fn set_biome_id(&mut self, x: u8, y: i32, z: u8, biome_id: i32) {
        // Make sure biome storage has enough sections
        if self.chunk.biomes.is_empty() {
            self.chunk.biomes = (0..self.chunk.subchunks.len())
                .map(|_| PalettedBiomeStorage::Single(0))
                .collect();
        }
        let section = match self.chunk.section_of(y) {
            Some(idx) => idx,
            None => return,
        };
        let local_y = ((y - self.chunk.min_y) % SUBCHUNK_SIZE) as u8;
        let index = ((x as usize) << 8) | ((z as usize) << 4) | local_y as usize;
        if let Some(storage) = self.chunk.biomes.get_mut(section) {
            storage.set(index, biome_id as u32);
        }
    }

    // --- Heightmap ---

    /// Reads the heightmap value at a column.
    pub fn height_map(&self, x: u8, z: u8) -> i32 {
        self.heightmap[(x as usize) * 16 + z as usize] as i32
    }

    /// Writes a heightmap value (stored as `i16`, truncating).
    pub fn set_height_map(&mut self, x: u8, z: u8, value: i32) {
        self.heightmap[(x as usize) * 16 + z as usize] = value as i16;
    }

    /// Returns the heightmap array (read-only).
    pub fn height_map_array(&self) -> &[i16; 256] {
        &self.heightmap
    }

    /// Returns the heightmap array (mutable, for batch writes).
    pub fn height_map_array_mut(&mut self) -> &mut [i16; 256] {
        &mut self.heightmap
    }

    /// Scans a column top-down and returns the y of the highest non-air
    /// block (0 means an all-air column).
    pub fn recalculate_height_map_column(&mut self, x: u8, z: u8) -> i32 {
        let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        let highest = self.chunk.highest_block_at(x, z, air).unwrap_or(0);
        self.set_height_map(x, z, highest);
        highest
    }

    /// Recalculates the whole heightmap.
    pub fn recalculate_height_map(&mut self) {
        for x in 0u8..16 {
            for z in 0u8..16 {
                self.recalculate_height_map_column(x, z);
            }
        }
    }

    // --- Section access ---

    /// Returns whether a section is empty (`fY` is the world section Y,
    /// e.g. -4..19 in the overworld).
    pub fn is_section_empty(&self, f_y: i32) -> bool {
        let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        match self.section_index_of(f_y) {
            None => true,
            Some(idx) => self
                .chunk
                .subchunks
                .get(idx)
                .map(|s| s.is_all_air(air))
                .unwrap_or(true),
        }
    }

    /// Returns a section by world section Y.
    pub fn get_section(&self, f_y: i32) -> Option<&SubChunk> {
        let idx = self.section_index_of(f_y)?;
        self.chunk.subchunks.get(idx)
    }

    /// Returns all sections.
    pub fn get_sections(&self) -> &[SubChunk] {
        &self.chunk.subchunks
    }

    /// Mutable section access (for generation-pipeline writes).
    pub fn get_section_mut(&mut self, f_y: i32) -> Option<&mut SubChunk> {
        let idx = self.section_index_of(f_y)?;
        self.chunk.subchunks.get_mut(idx)
    }

    /// Maps a world section Y to an array index.
    ///
    /// Overworld min_y=-64 → first section Y = -64/16 = -4;
    /// array index = `fY - (-4)` = `fY + 4`.
    fn section_index_of(&self, f_y: i32) -> Option<usize> {
        let first = self.chunk.min_y.div_euclid(SUBCHUNK_SIZE);
        let idx = f_y - first;
        if idx < 0 {
            return None;
        }
        Some(idx as usize)
    }

    // --- Inner chunk access ---

    /// Accesses the wrapped `Chunk` (read-only).
    pub fn inner(&self) -> &Chunk {
        &self.chunk
    }

    /// Accesses the wrapped `Chunk` (mutable).
    pub fn inner_mut(&mut self) -> &mut Chunk {
        &mut self.chunk
    }

    /// Consumes the adapter and returns the wrapped `Chunk`.
    pub fn into_inner(mut self) -> Chunk {
        let mut bytes = [0u8; 512];
        for (index, height) in self.heightmap.iter().enumerate() {
            bytes[index * 2..index * 2 + 2].copy_from_slice(&height.to_le_bytes());
        }
        self.chunk.data_3d_heightmap = Some(Arc::new(bytes));
        self.chunk
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sc_world::chunk::ChunkPosition;

    fn make_chunk() -> WorldgenChunk {
        let chunk = Chunk::empty_overworld(ChunkPosition::new(0, 0));
        WorldgenChunk::new(chunk)
    }

    #[test]
    fn chunk_state_ordinals() {
        // Ordinals match the reference ordinal()
        assert!(ChunkState::New < ChunkState::Started);
        assert!(ChunkState::Started < ChunkState::Generated);
        assert!(ChunkState::Generated < ChunkState::Populated);
        assert!(ChunkState::Populated < ChunkState::Finished);
    }

    #[test]
    fn chunk_state_can_send() {
        // canSend() = ordinal() >= 3
        assert!(!ChunkState::New.can_send());
        assert!(!ChunkState::Started.can_send());
        assert!(!ChunkState::Generated.can_send());
        assert!(ChunkState::Populated.can_send());
        assert!(ChunkState::Finished.can_send());
    }

    #[test]
    fn chunk_state_transitions() {
        let mut c = make_chunk();
        assert!(!c.is_generated());
        c.set_generated();
        assert!(c.is_generated());
        assert!(!c.is_populated());
        c.set_populated();
        assert!(c.is_populated());
        c.set_state(ChunkState::Finished);
        assert!(c.is_finished());
    }

    #[test]
    fn heightmap_read_write() {
        let mut c = make_chunk();
        c.set_height_map(5, 7, 100);
        assert_eq!(c.height_map(5, 7), 100);
        // Truncates to i16 (wrapping: 40000 → -25536)
        c.set_height_map(0, 0, 40000);
        assert_eq!(
            c.height_map(0, 0),
            40000i32.wrapping_rem_euclid(65536) as i16 as i32
        );
    }

    #[test]
    fn generated_heightmap_is_carried_in_the_chunk_snapshot() {
        let mut generated = make_chunk();
        generated.set_height_map(5, 7, 83);
        let chunk = generated.into_inner();
        let bytes = chunk
            .data_3d_heightmap
            .as_ref()
            .expect("heightmap snapshot");
        let index = 5 * 16 + 7;
        assert_eq!(
            i16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]]),
            83
        );
        assert_eq!(WorldgenChunk::new(chunk).height_map(5, 7), 83);
    }

    #[test]
    fn biome_set_get() {
        let mut c = make_chunk();
        // y=64 (section 8, local y=0), x=3, z=5
        c.set_biome_id(3, 64, 5, 42);
        assert_eq!(c.biome_id(3, 64, 5), 42);
        // Same section, different position
        c.set_biome_id(3, 70, 5, 7);
        assert_eq!(c.biome_id(3, 70, 5), 7);
        assert_eq!(c.biome_id(3, 64, 5), 42); // Unaffected by the later write
    }

    #[test]
    fn block_set_get() {
        let mut c = make_chunk();
        c.set_block_state(3, 64, 5, 0, BlockRuntimeId(10));
        assert_eq!(c.block_state(3, 64, 5, 0), BlockRuntimeId(10));
        // Out-of-range y returns air
        let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        assert_eq!(c.block_state(0, 9999, 0, 0), air);
    }

    #[test]
    fn section_index_mapping() {
        let c = make_chunk();
        // Overworld min_y=-64, first section = -4
        assert_eq!(c.section_index_of(-4), Some(0));
        assert_eq!(c.section_index_of(0), Some(4));
        assert_eq!(c.section_index_of(19), Some(23));
        assert_eq!(c.section_index_of(-5), None); // Out of range
    }

    #[test]
    fn coordinates() {
        let chunk = Chunk::empty_overworld(ChunkPosition::new(3, -7));
        let c = WorldgenChunk::new(chunk);
        assert_eq!(c.x(), 3);
        assert_eq!(c.z(), -7);
        assert_eq!(c.min_y(), -64);
        assert_eq!(c.max_y(), 319);
    }
}
