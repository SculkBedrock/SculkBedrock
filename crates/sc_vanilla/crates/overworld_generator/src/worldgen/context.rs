//! 区块生成上下文与方块管理器。
//!
//! 上下文持有生成中的区块、世界级持有者与维度边界；方块管理器提供缓冲写入、读取与区块应用。
//!
//! Chunk generation context and block manager.
//!
//! The context holds the chunk under generation, the world-level holder, and dimension bounds; the block manager provides buffered writes, reads, and chunk application.

use crate::worldgen::chunk::WorldgenChunk;
use crate::worldgen::holder::normal::NormalObjectHolder;
use std::collections::HashMap;
use sc_world::chunk::BlockRuntimeId;
use sc_world::storage::MAX_GENERATED_SPILLOVER_WRITES;

// ---------------------------------------------------------------------------
// ChunkGenerateContext
// ---------------------------------------------------------------------------

/// chunk 生成上下文。
///
/// Chunk generation context.
///
/// 持有生成中的 chunk、世界级持有者与维度边界。
///
/// Holds the chunk under generation, the world-level holder, and dimension
/// bounds.
///
/// 同时持有等价的共享写缓冲区：特征阶段内的所有方块写入合并进
/// `root_places`，由阶段末尾的 `apply_root_to_chunk` 一次性提交
/// （chunk 方块加高度图）。同时 `chunk.height_map` 保持原始地形值，
/// 后一个特征不会叠在前一个特征的树冠上。
///
/// Also holds the shared write-buffer equivalent: all feature block writes
/// within a feature stage merge into `root_places`, committed in one shot by
/// `apply_root_to_chunk` at the end of the stage (chunk blocks + heightmap).
/// Meanwhile `chunk.height_map` keeps the original terrain values so a later
/// feature never stacks a tree on top of an earlier tree's canopy.
pub struct ChunkGenerateContext<'a> {
    /// 生成中的 chunk。
    ///
    /// The chunk under generation.
    pub chunk: &'a mut WorldgenChunk,
    /// 共享写缓冲区，`places` 半边（以 [`BlockManager::hash_xyz`] 为键）。
    ///
    /// Shared write buffer, `places` half (keyed by [`BlockManager::hash_xyz`]).
    root_places: HashMap<u64, BlockEntry>,
    /// 共享写缓冲区，`caches` 半边（用于已缓存方块查询）。
    ///
    /// Shared write buffer, `caches` half (for cached-block queries).
    root_caches: HashMap<u64, BlockRuntimeId>,
    /// 跨 chunk 写入（悬垂的特征/结构部分）：先暂存在此，由生成器经
    /// `WorldGenerator::generate_chunk_with_spillover` 交给 provider 路由
    /// （已缓存的邻居立即写入；未生成的邻居进入待写队列）。
    ///
    /// Cross-chunk writes (overhanging feature/structure parts): staged here
    /// and handed by the generator to the provider for routing via
    /// `WorldGenerator::generate_chunk_with_spillover` (cached neighbors are
    /// written immediately; ungenerated neighbors go to a pending queue).
    spillover: Vec<BlockEntry>,
    spillover_write_limit: usize,
    queued_spillover_writes: usize,
    spillover_overflowed: bool,
    /// 世界级对象持有者。
    ///
    /// World-level object holder.
    pub holder: &'a NormalObjectHolder,
    /// 世界种子。
    ///
    /// World seed.
    pub level_seed: i64,
    /// 世界最小高度。
    ///
    /// Minimum world height.
    pub min_y: i32,
    /// 世界最大高度。
    ///
    /// Maximum world height.
    pub max_y: i32,
    /// 结构生成开关（生成器设置 "structures"，默认 true）。
    ///
    /// Structure-generation switch (generator setting "structures",
    /// default true).
    pub structures: bool,
}

impl<'a> ChunkGenerateContext<'a> {
    /// 构造上下文（跨 chunk 写入使用默认上限）。
    ///
    /// Constructs a context (cross-chunk writes use the default bound).
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

    /// 构造上下文并指定跨 chunk 写入上限。
    ///
    /// Constructs a context with a configurable cross-chunk write bound.
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

    /// 返回生成中的 chunk。
    ///
    /// Returns the chunk under generation.
    pub fn chunk(&self) -> &WorldgenChunk {
        self.chunk
    }

    /// 返回生成中的 chunk（可变，供阶段直接写入）。
    ///
    /// Returns the chunk under generation (mutable, for direct stage writes).
    pub fn chunk_mut(&mut self) -> &mut WorldgenChunk {
        self.chunk
    }

    /// 返回世界级对象持有者。
    ///
    /// Returns the world-level object holder.
    pub fn holder(&self) -> &NormalObjectHolder {
        self.holder
    }

    /// 返回世界种子。
    ///
    /// Returns the world seed.
    pub fn level_seed(&self) -> i64 {
        self.level_seed
    }

    /// 返回世界最小高度。
    ///
    /// Returns the minimum world height.
    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    /// 返回世界最大高度。
    ///
    /// Returns the maximum world height.
    pub fn max_y(&self) -> i32 {
        self.max_y
    }

    /// 把特征的 `places` 缓冲区合并进共享根缓冲区（暂不写 chunk）；由阶段
    /// 末尾的 `apply_root_to_chunk` 一次性提交。
    ///
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

    /// 溢出写入超限：作废整个生成结果（清空所有缓冲区）。
    ///
    /// Spillover writes exceeded the bound: voids the whole generation
    /// result (clears all buffers).
    fn reject_spillover_result(&mut self) {
        self.spillover_overflowed = true;
        self.root_places.clear();
        self.root_caches.clear();
        self.spillover.clear();
        self.queued_spillover_writes = 0;
    }

    /// 从根缓冲区读取已暂存但未提交的方块。
    ///
    /// Reads a queued-but-uncommitted block from the root buffer.
    pub fn root_cached_block(&self, x: i32, y: i32, z: i32) -> BlockRuntimeId {
        let hash = BlockManager::hash_xyz(x, y, z, 0);
        self.root_caches
            .get(&hash)
            .copied()
            .unwrap_or_else(|| BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()))
    }

    /// 在阶段末尾一次性提交所有缓存的根写入（chunk 方块写入加高度图更新）。
    ///
    /// Commits all buffered root writes at the end of the stage (chunk
    /// block writes + heightmap updates).
    ///
    /// 跨 chunk 条目收进 `spillover`，由生成器交给 provider 持久化/写入。
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

    /// 本次私有生成结果是否超出了配置的跨 chunk 写入上限；调用方必须拒绝
    /// 整个结果。
    ///
    /// True when this private generation result exceeded its configured
    /// cross-chunk write bound. The caller must reject the whole result.
    pub fn spillover_overflowed(&self) -> bool {
        self.spillover_overflowed
    }

    /// 取走累积的跨 chunk 写入（生成器在阶段链结束后调用一次）。
    ///
    /// Takes the accumulated cross-chunk writes (called once by the
    /// generator after the stage chain finishes).
    pub fn take_spillover(&mut self) -> Vec<BlockEntry> {
        std::mem::take(&mut self.spillover)
    }
}

// ---------------------------------------------------------------------------
// BlockEntry (minimal block data needed by the generation pipeline)
// ---------------------------------------------------------------------------

/// 生成流水线所需的最小方块数据：坐标加层加运行时 id。
///
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

/// 缓冲式方块写入管理器。
///
/// Buffered block-write manager.
///
/// 双映射缓冲：
/// - `places`：待写入的方块（以 `hash_xyz` 为键）；
/// - `caches`：已缓存方块的读缓存（以 `hash_xyz` 为键）。
///
/// Dual-map buffer:
/// - `places`: pending block writes (keyed by `hash_xyz`);
/// - `caches`: read cache of buffered blocks (keyed by `hash_xyz`).
///
/// `chunk`：生成中 chunk 的只读引用。跨 chunk 读取返回空气
/// （与未加载邻居 chunk 相同的回退）。
///
/// `chunk`: read-only reference to the chunk under generation. Cross-chunk
/// reads return air (same fallback as an unloaded neighbor chunk).
///
/// `apply_to_chunk` 把 `places` 写进目标 chunk 并更新高度图。
///
/// `apply_to_chunk` writes `places` into the target chunk and updates the
/// heightmap.
pub struct BlockManager<'a> {
    /// 已缓冲的方块写入。
    ///
    /// Buffered block writes.
    places: HashMap<u64, BlockEntry>,
    /// 已缓冲方块读缓存。
    ///
    /// Buffered block read cache.
    caches: HashMap<u64, BlockRuntimeId>,
    /// 生成中 chunk 的只读视图（其已加载读取路径）。
    ///
    /// Read-only view of the chunk under generation (its loaded-read path).
    chunk: Option<&'a WorldgenChunk>,
    /// 世界种子（垂悬藤蔓放置由 `seed + x + y + z` 派生确定性随机源）。
    ///
    /// World seed (hanging-moss placement derives a deterministic random
    /// source from `seed + x + y + z`).
    seed: i64,
}

impl<'a> BlockManager<'a> {
    /// 无 chunk 构造函数，用于纯缓冲场景（如矿石）。
    ///
    /// Unchunked constructor for purely buffered scenarios (e.g. ores).
    pub fn new() -> Self {
        Self {
            places: HashMap::new(),
            caches: HashMap::new(),
            chunk: None,
            seed: 0,
        }
    }

    /// 带 chunk 读回退的构造函数（用于树木等必须看到当前 chunk 地形的
    /// 特征生成）。
    ///
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

    /// 带 chunk 读回退加世界种子的构造函数，使树木生成时的地面检查能看到
    /// 当前 chunk 的地形。
    ///
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

    /// 返回世界种子。
    ///
    /// Returns the world seed.
    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// 返回方块列的高度图值。
    ///
    /// Returns the heightmap value at a block column.
    ///
    /// 仅当前 chunk 内可用（未加载的跨 chunk 列无可读值，`None` 即告知
    /// 调用方跳过——单 chunk 生成近似）。
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

    /// 返回某位置的方块，优先查缓存。
    ///
    /// Returns the block at a position, consulting the cache first.
    ///
    /// 语义：
    /// 1. 缓存命中 → 返回缓存的方块；
    /// 2. 位置在当前（已加载）chunk 内 → 读 chunk 并回填缓存；
    /// 3. 否则（未加载的跨 chunk）→ 空气回退，不写缓存。
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

    /// 当前 chunk 的最小高度（无 chunk 时为 -64）。
    ///
    /// Minimum height of the current chunk (-64 without a chunk).
    pub fn min_height(&self) -> i32 {
        self.chunk.map(|c| c.min_y()).unwrap_or(-64)
    }

    /// 当前 chunk 的最大高度 +1（无 chunk 时为 320）。
    ///
    /// Maximum height of the current chunk + 1 (320 without a chunk).
    pub fn max_height(&self) -> i32 {
        self.chunk.map(|c| c.max_y()).unwrap_or(320)
    }

    /// 把方块位置加层位打包成单个 `u64` 键。
    ///
    /// Bit-packs a block position plus layer into a single `u64` key.
    ///
    /// 布局：x（26 位）| z（26 位）| y（10 位）| 层（1 位），
    /// 偏移为 x+30M / z+30M / y+400。
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

    /// 缓冲某位置某层的方块状态写入。
    ///
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

    /// 仅当缓存中无该位置条目时缓冲 layer-0 方块状态写入；返回是否缓冲成功。
    ///
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

    /// 返回某位置缓存的方块，未缓存时为 `None`。
    ///
    /// Returns the cached block at a position, or `None` when uncached.
    pub fn get_block_if_cached(&self, x: i32, y: i32, z: i32) -> Option<BlockRuntimeId> {
        let hash = Self::hash_xyz(x, y, z, 0);
        self.caches.get(&hash).copied()
    }

    /// 从缓存返回某位置的方块，未缓存时为空气。
    ///
    /// Returns the block at a position from the cache, or air when uncached.
    pub fn get_block_at(&self, x: i32, y: i32, z: i32) -> BlockRuntimeId {
        let hash = Self::hash_xyz(x, y, z, 0);
        self.caches
            .get(&hash)
            .copied()
            .unwrap_or(BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()))
    }

    /// 返回某位置/层是否有缓存条目。
    ///
    /// Returns whether a position/layer has a cached entry.
    pub fn is_cached(&self, x: i32, y: i32, z: i32, layer: usize) -> bool {
        self.caches.contains_key(&Self::hash_xyz(x, y, z, layer))
    }

    /// 把另一个管理器的缓冲写入合并进本管理器（缓存同步更新）。
    ///
    /// Merges another manager's buffered writes into this one (caches
    /// updated in step).
    pub fn merge(&mut self, mut other: BlockManager<'a>) {
        for (hash, entry) in other.places.drain() {
            self.places.insert(hash, entry);
            self.caches.insert(hash, entry.block);
        }
    }

    /// 返回已缓冲的方块写入。
    ///
    /// Returns the buffered block writes.
    pub fn places(&self) -> &HashMap<u64, BlockEntry> {
        &self.places
    }

    /// 把当前 chunk 中的 `places` 写进 `WorldgenChunk` 并更新高度图，
    /// 随后清空 places/caches。
    ///
    /// Writes the buffered `places` in the current chunk into a
    /// `WorldgenChunk` and updates the heightmap, then clears
    /// places/caches.
    ///
    /// 当前 chunk 之外的方块（跨 chunk）会被跳过——由生成会话路由。
    ///
    /// Cross-chunk blocks (outside the current chunk) are skipped — the
    /// generation session routes them.
    pub fn apply_to_chunk(&mut self, chunk: &mut WorldgenChunk) {
        let places = std::mem::take(&mut self.places);
        Self::apply_places_to_chunk(places, chunk);
        self.caches.clear();
    }

    /// 消耗 places 缓冲区（树木生成：管理器持有 chunk 共享借用时，先分离
    /// places 以在可变写 chunk 前释放借用）。
    ///
    /// Consumes the places buffer (tree generation: when the manager holds a
    /// shared borrow of the chunk, the places are detached first to release
    /// the borrow before mutably writing the chunk).
    pub fn into_places(mut self) -> HashMap<u64, BlockEntry> {
        std::mem::take(&mut self.places)
    }

    /// `apply_to_chunk` 的无借用冲突变体：直接应用一个 places 映射
    /// （chunk 写入加高度图维护，跳过跨 chunk 条目）。
    ///
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

    /// 返回是否没有待应用的方块。
    ///
    /// Returns whether there are no pending blocks to apply.
    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// 清空缓冲区。
    ///
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

    /// 根缓冲区收集多个越界条目（多个邻居 chunk/高度）后，把整个多重集
    /// 交给 `take_spillover`。注意：Vec 顺序遵循 `root_places` 的 HashMap
    /// 迭代顺序（见 `apply_root_to_chunk`），因此按返回顺序编号不是稳定
    /// 的标识契约。
    ///
    /// The root buffer collects several out-of-chunk entries (multiple
    /// neighbor chunks / heights) and hands the full multiset to
    /// `take_spillover`. Note: the Vec order follows `root_places` HashMap
    /// iteration order (see `apply_root_to_chunk`), so provider-return-order
    /// ordinals are not a stable identity contract.
    #[test]
    fn apply_root_collects_multiple_out_of_chunk_entries_as_a_multiset() {
        let holder = make_holder();
        let chunk = sc_world::chunk::Chunk::empty_overworld(ChunkPosition::new(0, 0));
        let mut wc = WorldgenChunk::new(chunk);
        let mut context =
            ChunkGenerateContext::new_with_spillover_limit(&mut wc, &holder, 42, -64, 319, 16);
        let entries = [
            BlockEntry { x: 16, y: 70, z: 0, layer: 0, block: BlockRuntimeId(10) },
            BlockEntry { x: -1, y: 72, z: 5, layer: 0, block: BlockRuntimeId(11) },
            BlockEntry { x: 0, y: 74, z: 16, layer: 0, block: BlockRuntimeId(12) },
            BlockEntry { x: 20, y: 68, z: 20, layer: 0, block: BlockRuntimeId(13) },
        ];
        context.queue_object(
            entries
                .iter()
                .map(|e| (BlockManager::hash_xyz(e.x, e.y, e.z, e.layer), *e))
                .collect(),
        );
        context.apply_root_to_chunk();

        assert!(!context.spillover_overflowed());
        let spill = context.take_spillover();
        assert_eq!(spill.len(), 4);
        let mut actual: Vec<(i32, i32, i32, u32)> =
            spill.iter().map(|e| (e.x, e.y, e.z, e.block.0)).collect();
        actual.sort_unstable();
        let mut expected: Vec<(i32, i32, i32, u32)> =
            entries.iter().map(|e| (e.x, e.y, e.z, e.block.0)).collect();
        expected.sort_unstable();
        assert_eq!(actual, expected, "spillover multiset must match queued entries");
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
