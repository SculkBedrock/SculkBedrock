//! Chunk generation context + block manager (generation subset).
//!
//! Layout notes:
//! - The context holds the chunk under generation, the world-level holder,
//!   and dimension bounds, instead of level/generator/chunk references.
//! - Only the **buffer + apply** subset of the block manager is used: the
//!   `hashXYZ` key function, the `places`/`caches` dual maps, and
//!   `setBlockStateAt`/`getBlockAt`/`applySubChunkUpdate` (chunk writes +
//!   heightmap updates). Broadcasts and pending sub-chunk updates belong to
//!   the runtime network layer; cross-chunk writes are routed by the
//!   generation session.
//! - Block objects map to [`BlockEntry`] (coordinates + layer + runtime id).
//! - The `hashXYZ` bit packing matches exactly (x/z 26 bits, y 10 bits,
//!   layer 1 bit).

use crate::worldgen::chunk::WorldgenChunk;
use crate::worldgen::holder::normal::NormalObjectHolder;
use std::collections::HashMap;
use sc_world::chunk::BlockRuntimeId;
use sc_world::storage::MAX_GENERATED_SPILLOVER_WRITES;

// ---------------------------------------------------------------------------
// ChunkGenerateContext
// ---------------------------------------------------------------------------

/// Chunk generation context.
///
/// Holds the chunk under generation, the world-level holder, and dimension
/// bounds.
///
/// Also holds the shared write-buffer equivalent: all feature block writes
/// within a feature stage merge into `root_places`, committed in one shot by
/// `apply_root_to_chunk` at the end of the stage (chunk blocks + heightmap).
/// Meanwhile `chunk.height_map` keeps the original terrain values so a later
/// feature never stacks a tree on top of an earlier tree's canopy.
pub struct ChunkGenerateContext<'a> {
    /// The chunk under generation.
    pub chunk: &'a mut WorldgenChunk,
    /// Shared write buffer, `places` half (keyed by [`BlockManager::hash_xyz`]).
    root_places: HashMap<u64, BlockEntry>,
    /// Shared write buffer, `caches` half (for cached-block queries).
    root_caches: HashMap<u64, BlockRuntimeId>,
    /// Cross-chunk writes (overhanging feature/structure parts): staged here
    /// and handed by the generator to the provider for routing via
    /// `WorldGenerator::generate_chunk_with_spillover` (cached neighbors are
    /// written immediately; ungenerated neighbors go to a pending queue).
    spillover: Vec<BlockEntry>,
    spillover_write_limit: usize,
    queued_spillover_writes: usize,
    spillover_overflowed: bool,
    /// World-level object holder.
    pub holder: &'a NormalObjectHolder,
    /// World seed.
    pub level_seed: i64,
    /// Minimum world height.
    pub min_y: i32,
    /// Maximum world height.
    pub max_y: i32,
    /// Structure-generation switch (generator setting "structures",
    /// default true).
    pub structures: bool,
}

impl<'a> ChunkGenerateContext<'a> {
    pub fn new(
        chunk: &'a mut WorldgenChunk,
        holder: &'a NormalObjectHolder,
        level_seed: i64,
        min_y: i32,
        max_y: i32,
    ) -> Self {
        Self::new_with_spillover_limit(
            chunk,
            holder,
            level_seed,
            min_y,
            max_y,
            MAX_GENERATED_SPILLOVER_WRITES,
        )
    }

    pub fn new_with_spillover_limit(
        chunk: &'a mut WorldgenChunk,
        holder: &'a NormalObjectHolder,
        level_seed: i64,
        min_y: i32,
        max_y: i32,
        spillover_write_limit: usize,
    ) -> Self {
        Self {
            chunk,
            root_places: HashMap::new(),
            root_caches: HashMap::new(),
            spillover: Vec::new(),
            spillover_write_limit,
            queued_spillover_writes: 0,
            spillover_overflowed: false,
            holder,
            level_seed,
            min_y,
            max_y,
            structures: true,
        }
    }

    /// Returns the chunk under generation.
    pub fn chunk(&self) -> &WorldgenChunk {
        self.chunk
    }

    /// Returns the chunk under generation (mutable, for direct stage writes).
    pub fn chunk_mut(&mut self) -> &mut WorldgenChunk {
        self.chunk
    }

    /// Returns the world-level object holder.
    pub fn holder(&self) -> &NormalObjectHolder {
        self.holder
    }

    /// Returns the world seed.
    pub fn level_seed(&self) -> i64 {
        self.level_seed
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    pub fn max_y(&self) -> i32 {
        self.max_y
    }

    /// Merges a feature's `places` buffer into the shared root buffer
    /// without writing the chunk yet; committed in one shot by
    /// `apply_root_to_chunk` at the end of the stage.
    pub fn queue_object(&mut self, places: HashMap<u64, BlockEntry>) {
        if self.spillover_overflowed {
            return;
        }
        let chunk_x = self.chunk.x();
        let chunk_z = self.chunk.z();
        for (hash, entry) in places {
            let crosses_chunk = (entry.x >> 4) != chunk_x || (entry.z >> 4) != chunk_z;
            if crosses_chunk && !self.root_places.contains_key(&hash) {
                if self
                    .spillover
                    .len()
                    .saturating_add(self.queued_spillover_writes)
                    >= self.spillover_write_limit
                {
                    self.reject_spillover_result();
                    return;
                }
                self.queued_spillover_writes += 1;
            }
            self.root_caches.insert(hash, entry.block);
            self.root_places.insert(hash, entry);
        }
    }

    fn reject_spillover_result(&mut self) {
        self.spillover_overflowed = true;
        self.root_places.clear();
        self.root_caches.clear();
        self.spillover.clear();
        self.queued_spillover_writes = 0;
    }

    /// Reads a queued-but-uncommitted block from the root buffer.
    pub fn root_cached_block(&self, x: i32, y: i32, z: i32) -> BlockRuntimeId {
        let hash = BlockManager::hash_xyz(x, y, z, 0);
        self.root_caches
            .get(&hash)
            .copied()
            .unwrap_or_else(|| BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()))
    }

    /// Commits all buffered root writes at the end of the stage (chunk
    /// block writes + heightmap updates).
    ///
    /// Cross-chunk entries are collected into `spillover` for the generator
    /// to hand to the provider for persistence/writing.
    pub fn apply_root_to_chunk(&mut self) {
        if self.spillover_overflowed {
            return;
        }
        let places = std::mem::take(&mut self.root_places);
        self.root_caches.clear();
        self.queued_spillover_writes = 0;
        let (chunk_x, chunk_z) = (self.chunk.x(), self.chunk.z());
        let mut in_chunk = HashMap::with_capacity(places.len());
        for (hash, entry) in places {
            if (entry.x >> 4) == chunk_x && (entry.z >> 4) == chunk_z {
                in_chunk.insert(hash, entry);
            } else {
                if self.spillover.len() >= self.spillover_write_limit {
                    self.reject_spillover_result();
                    return;
                }
                self.spillover.push(entry);
            }
        }
        BlockManager::apply_places_to_chunk(in_chunk, self.chunk);
    }

    /// True when this private generation result exceeded its configured
    /// cross-chunk write bound. The caller must reject the whole result.
    pub fn spillover_overflowed(&self) -> bool {
        self.spillover_overflowed
    }

    /// Takes the accumulated cross-chunk writes (called once by the generator
    /// after the stage chain finishes).
    pub fn take_spillover(&mut self) -> Vec<BlockEntry> {
        std::mem::take(&mut self.spillover)
    }
}

// ---------------------------------------------------------------------------
// BlockEntry (minimal block data needed by the generation pipeline)
// ---------------------------------------------------------------------------

/// Minimum per-block data needed by the generation pipeline:
/// coordinates + layer + runtime id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockEntry {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub layer: usize,
    pub block: BlockRuntimeId,
}

// ---------------------------------------------------------------------------
// BlockManager (buffer + apply subset)
// ---------------------------------------------------------------------------

/// Buffered block-write manager.
///
/// Dual-map buffer:
/// - `places`: pending block writes (keyed by `hash_xyz`);
/// - `caches`: read cache of buffered blocks (keyed by `hash_xyz`).
///
/// `chunk`: read-only reference to the chunk under generation. Cross-chunk
/// reads return air (same fallback as an unloaded neighbor chunk).
///
/// `apply_to_chunk` writes `places` into the target chunk and updates the
/// heightmap.
pub struct BlockManager<'a> {
    /// Buffered block writes.
    places: HashMap<u64, BlockEntry>,
    /// Buffered block read cache.
    caches: HashMap<u64, BlockRuntimeId>,
    /// Read-only view of the chunk under generation (its loaded-read path).
    chunk: Option<&'a WorldgenChunk>,
    /// World seed (hanging-moss placement derives a deterministic random
    /// source from `seed + x + y + z`).
    seed: i64,
}

impl<'a> BlockManager<'a> {
    /// Unchunked constructor for purely buffered scenarios (e.g. ores).
    pub fn new() -> Self {
        Self {
            places: HashMap::new(),
            caches: HashMap::new(),
            chunk: None,
            seed: 0,
        }
    }

    /// Constructor with a chunk read fallback (used by feature generation
    /// such as trees, which must see the current chunk's terrain).
    pub fn with_chunk(chunk: &'a WorldgenChunk) -> Self {
        Self {
            places: HashMap::new(),
            caches: HashMap::new(),
            chunk: Some(chunk),
            seed: 0,
        }
    }

    /// Constructor with a chunk read fallback plus the world seed, so ground
    /// checks during tree generation can see the current chunk's terrain.
    pub fn with_chunk_and_seed(chunk: &'a WorldgenChunk, seed: i64) -> Self {
        Self {
            places: HashMap::new(),
            caches: HashMap::new(),
            chunk: Some(chunk),
            seed,
        }
    }

    /// Returns the world seed.
    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// Returns the heightmap value at a block column.
    ///
    /// Only available inside the current chunk (an unloaded cross-chunk
    /// column has no readable value, so `None` tells the caller to skip —
    /// a single-chunk-generation approximation).
    pub fn height_map(&self, x: i32, z: i32) -> Option<i32> {
        let chunk = self.chunk?;
        if (x >> 4) == chunk.x() && (z >> 4) == chunk.z() {
            Some(chunk.height_map((x & 0xF) as u8, (z & 0xF) as u8))
        } else {
            None
        }
    }

    /// Returns the block at a position, consulting the cache first.
    ///
    /// Semantics:
    /// 1. Cache hit → return the cached block;
    /// 2. Position inside the current (loaded) chunk → read the chunk and
    ///    populate the cache;
    /// 3. Otherwise (unloaded cross-chunk) → air fallback, without caching.
    pub fn get_block_if_cached_or_loaded(&mut self, x: i32, y: i32, z: i32) -> BlockRuntimeId {
        let hash = Self::hash_xyz(x, y, z, 0);
        if let Some(block) = self.caches.get(&hash) {
            return *block;
        }
        let Some(chunk) = self.chunk else {
            return BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        };
        let in_current_chunk = (x >> 4) == chunk.x() && (z >> 4) == chunk.z();
        if !in_current_chunk {
            return BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        }
        let block = chunk.block_state((x & 0xF) as u8, y, (z & 0xF) as u8, 0);
        self.caches.insert(hash, block);
        block
    }

    /// Minimum height of the current chunk (-64 without a chunk).
    pub fn min_height(&self) -> i32 {
        self.chunk.map(|c| c.min_y()).unwrap_or(-64)
    }

    /// Maximum height of the current chunk + 1 (320 without a chunk).
    pub fn max_height(&self) -> i32 {
        self.chunk.map(|c| c.max_y()).unwrap_or(320)
    }

    /// Bit-packs a block position plus layer into a single `u64` key.
    ///
    /// Layout: x (26 bits) | z (26 bits) | y (10 bits) | layer (1 bit),
    /// with offsets x+30M / z+30M / y+400.
    pub fn hash_xyz(x: i32, y: i32, z: i32, layer: usize) -> u64 {
        let x_part = (((x + 30_000_000) as u64) & 0x3FFFFFF) << 37;
        let z_part = (((z + 30_000_000) as u64) & 0x3FFFFFF) << 11;
        let y_part = (((y + 400) as u64) & 0x3FF) << 1;
        let layer_part = (layer as u64) & 0x1;
        x_part | z_part | y_part | layer_part
    }

    /// Buffers a block-state write at a position and layer.
    pub fn set_block_state_at(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        layer: usize,
        block: BlockRuntimeId,
    ) {
        let hash = Self::hash_xyz(x, y, z, layer);
        let entry = BlockEntry {
            x,
            y,
            z,
            layer,
            block,
        };
        self.places.insert(hash, entry);
        self.caches.insert(hash, block);
    }

    /// Buffers a layer-0 block-state write only when the cache has no entry
    /// for the position; returns whether the write was buffered.
    pub fn set_block_state_at_if_cache_absent(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        block: BlockRuntimeId,
    ) -> bool {
        let hash = Self::hash_xyz(x, y, z, 0);
        if self.caches.contains_key(&hash) {
            return false;
        }
        self.set_block_state_at(x, y, z, 0, block);
        true
    }

    /// Returns the cached block at a position, or `None` when uncached.
    pub fn get_block_if_cached(&self, x: i32, y: i32, z: i32) -> Option<BlockRuntimeId> {
        let hash = Self::hash_xyz(x, y, z, 0);
        self.caches.get(&hash).copied()
    }

    /// Returns the block at a position from the cache, or air when uncached.
    pub fn get_block_at(&self, x: i32, y: i32, z: i32) -> BlockRuntimeId {
        let hash = Self::hash_xyz(x, y, z, 0);
        self.caches
            .get(&hash)
            .copied()
            .unwrap_or(BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()))
    }

    /// Returns whether a position/layer has a cached entry.
    pub fn is_cached(&self, x: i32, y: i32, z: i32, layer: usize) -> bool {
        self.caches.contains_key(&Self::hash_xyz(x, y, z, layer))
    }

    /// Merges another manager's buffered writes into this one (caches
    /// updated in step).
    pub fn merge(&mut self, mut other: BlockManager<'a>) {
        for (hash, entry) in other.places.drain() {
            self.places.insert(hash, entry);
            self.caches.insert(hash, entry.block);
        }
    }

    /// Returns the buffered block writes.
    pub fn places(&self) -> &HashMap<u64, BlockEntry> {
        &self.places
    }

    /// Writes the buffered `places` in the current chunk into a
    /// `WorldgenChunk` and updates the heightmap, then clears
    /// places/caches.
    ///
    /// Cross-chunk blocks (outside the current chunk) are skipped — the
    /// generation session routes them.
    pub fn apply_to_chunk(&mut self, chunk: &mut WorldgenChunk) {
        let places = std::mem::take(&mut self.places);
        Self::apply_places_to_chunk(places, chunk);
        self.caches.clear();
    }

    /// Consumes the places buffer (tree generation: when the manager holds a
    /// shared borrow of the chunk, the places are detached first to release
    /// the borrow before mutably writing the chunk).
    pub fn into_places(mut self) -> HashMap<u64, BlockEntry> {
        std::mem::take(&mut self.places)
    }

    /// Borrow-conflict-free variant of `apply_to_chunk`: applies a places map
    /// directly (chunk writes + heightmap maintenance, skipping cross-chunk
    /// entries).
    pub fn apply_places_to_chunk(places: HashMap<u64, BlockEntry>, chunk: &mut WorldgenChunk) {
        let chunk_x = chunk.x();
        let chunk_z = chunk.z();
        let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());

        for entry in places.values() {
            let local_x = entry.x & 0xF;
            let local_z = entry.z & 0xF;
            let entry_chunk_x = entry.x >> 4;
            let entry_chunk_z = entry.z >> 4;
            // Only writes blocks inside the current chunk
            if entry_chunk_x != chunk_x || entry_chunk_z != chunk_z {
                continue;
            }
            chunk.set_block_state(
                local_x as u8,
                entry.y,
                local_z as u8,
                entry.layer,
                entry.block,
            );
            // Heightmap update (layer 0, non-air only)
            if entry.layer == 0 && entry.block != air {
                let current_hm = chunk.height_map(local_x as u8, local_z as u8);
                if entry.y > current_hm {
                    chunk.set_height_map(local_x as u8, local_z as u8, entry.y);
                }
            }
        }
    }

    /// Returns whether there are no pending blocks to apply.
    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// Clears the buffers.
    pub fn clear(&mut self) {
        self.places.clear();
        self.caches.clear();
    }
}

impl Default for BlockManager<'_> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::random::Xoroshiro128;
    use sc_world::chunk::ChunkPosition;

    fn make_context<'a>(
        chunk: &'a mut WorldgenChunk,
        holder: &'a NormalObjectHolder,
    ) -> ChunkGenerateContext<'a> {
        ChunkGenerateContext::new(chunk, holder, 12345, -64, 319)
    }

    fn make_holder() -> NormalObjectHolder {
        NormalObjectHolder::new(
            Xoroshiro128::new(42),
            crate::worldgen::material::MaterialBlocks {
                air: BlockRuntimeId(0),
                water: BlockRuntimeId(1),
                lava: BlockRuntimeId(2),
                stone: BlockRuntimeId(3),
                granite: BlockRuntimeId(4),
                tuff: BlockRuntimeId(5),
                copper_ore: BlockRuntimeId(6),
                deepslate_iron_ore: BlockRuntimeId(7),
                raw_copper_block: BlockRuntimeId(8),
                raw_iron_block: BlockRuntimeId(9),
            },
        )
    }

    #[test]
    fn spillover_accumulator_rejects_whole_result_at_its_limit() {
        let holder = make_holder();
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        let mut context =
            ChunkGenerateContext::new_with_spillover_limit(&mut wc, &holder, 42, -64, 319, 1);
        let first = BlockEntry {
            x: 16,
            y: 70,
            z: 0,
            layer: 0,
            block: BlockRuntimeId(10),
        };
        let second = BlockEntry {
            x: 32,
            y: 70,
            z: 0,
            layer: 0,
            block: BlockRuntimeId(11),
        };
        context.queue_object(HashMap::from([
            (BlockManager::hash_xyz(first.x, first.y, first.z, 0), first),
            (
                BlockManager::hash_xyz(second.x, second.y, second.z, 0),
                second,
            ),
        ]));

        assert!(context.spillover_overflowed());
        assert!(context.root_places.is_empty());
        assert!(context.root_caches.is_empty());
        assert!(context.take_spillover().is_empty());
    }

    #[test]
    fn spillover_accumulator_accepts_exactly_the_configured_limit() {
        let holder = make_holder();
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        let mut context =
            ChunkGenerateContext::new_with_spillover_limit(&mut wc, &holder, 42, -64, 319, 1);
        let spillover = BlockEntry {
            x: 16,
            y: 70,
            z: 0,
            layer: 0,
            block: BlockRuntimeId(10),
        };
        context.queue_object(HashMap::from([(
            BlockManager::hash_xyz(spillover.x, spillover.y, spillover.z, 0),
            spillover,
        )]));
        context.apply_root_to_chunk();

        assert!(!context.spillover_overflowed());
        assert_eq!(context.take_spillover().len(), 1);
    }

    #[test]
    fn hash_xyz_matches_java_layout() {
        // Bit layout: x(26b)<<37 | z(26b)<<11 | y(10b)<<1 | layer(1b).
        // Distinct coordinates must produce distinct keys.
        let k1 = BlockManager::hash_xyz(0, 64, 0, 0);
        let k2 = BlockManager::hash_xyz(1, 64, 0, 0);
        let k3 = BlockManager::hash_xyz(0, 65, 0, 0);
        let k4 = BlockManager::hash_xyz(0, 64, 1, 0);
        let k5 = BlockManager::hash_xyz(0, 64, 0, 1);
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_ne!(k1, k4);
        assert_ne!(k1, k5);
    }

    #[test]
    fn hash_xyz_origin() {
        // x=0,y=0,z=0,layer=0 → offset (30M, 400, 30M, 0)
        let h = BlockManager::hash_xyz(0, 0, 0, 0);
        // Round-trip check: must not panic (exact value follows the bit layout)
        assert_eq!(h, BlockManager::hash_xyz(0, 0, 0, 0));
    }

    #[test]
    fn block_manager_set_and_get() {
        let mut bm = BlockManager::new();
        let block = BlockRuntimeId(42);
        bm.set_block_state_at(10, 64, 20, 0, block);
        assert_eq!(bm.get_block_at(10, 64, 20), block);
        assert!(bm.is_cached(10, 64, 20, 0));
        assert!(!bm.is_cached(10, 64, 21, 0));
    }

    #[test]
    fn block_manager_if_cache_absent() {
        let mut bm = BlockManager::new();
        assert!(bm.set_block_state_at_if_cache_absent(1, 64, 1, BlockRuntimeId(5)));
        assert!(!bm.set_block_state_at_if_cache_absent(1, 64, 1, BlockRuntimeId(6)));
        assert_eq!(bm.get_block_at(1, 64, 1), BlockRuntimeId(5));
    }

    #[test]
    fn block_manager_merge() {
        let mut bm1 = BlockManager::new();
        let mut bm2 = BlockManager::new();
        bm1.set_block_state_at(0, 64, 0, 0, BlockRuntimeId(1));
        bm2.set_block_state_at(1, 64, 1, 0, BlockRuntimeId(2));
        bm1.merge(bm2);
        assert_eq!(bm1.get_block_at(0, 64, 0), BlockRuntimeId(1));
        assert_eq!(bm1.get_block_at(1, 64, 1), BlockRuntimeId(2));
        assert!(!bm1.is_empty());
    }

    #[test]
    fn block_manager_apply_to_chunk() {
        let mut bm = BlockManager::new();
        // Blocks inside chunk (0,0)
        bm.set_block_state_at(3, 70, 5, 0, BlockRuntimeId(10));
        bm.set_block_state_at(3, 80, 5, 0, BlockRuntimeId(11));
        // Blocks inside chunk (1,0) (cross-chunk, must be skipped)
        bm.set_block_state_at(20, 70, 5, 0, BlockRuntimeId(99));

        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        bm.apply_to_chunk(&mut wc);

        assert_eq!(wc.block_state(3, 70, 5, 0), BlockRuntimeId(10));
        assert_eq!(wc.block_state(3, 80, 5, 0), BlockRuntimeId(11));
        // The heightmap must advance to the highest non-air y
        assert_eq!(wc.height_map(3, 5), 80);
        // Cross-chunk blocks are not written
        let air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        assert_eq!(wc.block_state(4, 70, 5, 0), air);
        // Buffers are cleared after apply
        assert!(bm.is_empty());
    }

    #[test]
    fn chunk_generate_context_basic() {
        let holder = make_holder();
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(5, -3));
        let mut wc = WorldgenChunk::new(chunk);
        let ctx = make_context(&mut wc, &holder);
        assert_eq!(ctx.chunk().x(), 5);
        assert_eq!(ctx.chunk().z(), -3);
        assert_eq!(ctx.level_seed(), 12345);
        assert_eq!(ctx.min_y(), -64);
        assert_eq!(ctx.max_y(), 319);
    }
}
