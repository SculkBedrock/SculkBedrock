//! BaseSnapshot 与 GenerationView 契约。
//!
//! [`BaseSnapshot`] 是区块基础地形的只读表示（地形密度、含水层、基岩、
//! 生物群系与初步表面高度图），不含树木、植被或结构装饰。跨区块特征
//! （树冠、支撑检查、土壤检查、悬崖）经 [`GenerationView`] 读取邻居快照：
//! 相邻区块缺失时返回 [`GenerationContractError::MissingDependency`]；
//! 低于 `min_y` 或高于 `max_y` 时返回定义的边界方块（基岩底面或天空空气）。
//!
//! [`BaseSnapshot`] is the read-only representation of a chunk's base terrain
//! (terrain density, aquifers, bedrock, biomes and preliminary surface
//! heightmap), without tree, vegetation or structure decorations. Cross-chunk
//! features (canopies, support checks, soil checks, cliffs) read neighbor
//! snapshots through [`GenerationView`]: a missing neighbor returns
//! [`GenerationContractError::MissingDependency`], and reads below `min_y` or
//! above `max_y` return the defined boundary blocks (bedrock floor or sky air).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::chunk::{BlockRuntimeId, Chunk, ChunkPosition};
use crate::generation::error::GenerationContractError;

/// 为 [`BaseSnapshot`] 或组装后的区块列计算 FNV-1a 64 摘要。
///
/// - `pos`: 区块位置
/// - `gen_fp`: 生成器指纹
/// - `world_instance_id`: 世界实例 id
/// - `min_y`/`max_y`: 垂直边界
/// - `height_map`: 表面高度图
/// - `chunk`: 参与摘要的区块列
///
/// Computes an FNV-1a 64 digest for a [`BaseSnapshot`] or assembled chunk
/// column.
///
/// - `pos`: chunk position
/// - `gen_fp`: generator fingerprint
/// - `world_instance_id`: world instance id
/// - `min_y`/`max_y`: vertical bounds
/// - `height_map`: surface heightmap
/// - `chunk`: chunk column feeding the digest
pub fn compute_base_digest(
    pos: ChunkPosition,
    gen_fp: u64,
    world_instance_id: u64,
    min_y: i32,
    max_y: i32,
    height_map: &[i32; 256],
    chunk: &Chunk,
) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    fn feed(h: &mut u64, bytes: &[u8]) {
        for b in bytes {
            *h ^= u64::from(*b);
            *h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    feed(&mut h, &pos.x.to_le_bytes());
    feed(&mut h, &pos.z.to_le_bytes());
    feed(&mut h, &gen_fp.to_le_bytes());
    feed(&mut h, &world_instance_id.to_le_bytes());
    feed(&mut h, &min_y.to_le_bytes());
    feed(&mut h, &max_y.to_le_bytes());
    for y in height_map {
        feed(&mut h, &y.to_le_bytes());
    }
    for subchunk in &chunk.subchunks {
        feed(&mut h, &[subchunk.index.y as u8]);
        for layer in &subchunk.layers {
            match layer {
                crate::chunk::PalettedBlockStorage::Single(id) => {
                    feed(&mut h, &[0]);
                    feed(&mut h, &id.0.to_le_bytes());
                }
                crate::chunk::PalettedBlockStorage::Paletted { palette, words } => {
                    feed(&mut h, &[1]);
                    feed(&mut h, &(palette.len() as u32).to_le_bytes());
                    for p in palette {
                        feed(&mut h, &p.0.to_le_bytes());
                    }
                    for w in words.as_ref() {
                        feed(&mut h, &w.to_le_bytes());
                    }
                }
            }
        }
    }
    for biome in &chunk.biomes {
        match biome {
            crate::chunk::PalettedBiomeStorage::Single(id) => {
                feed(&mut h, &[0]);
                feed(&mut h, &id.to_le_bytes());
            }
            crate::chunk::PalettedBiomeStorage::Paletted { palette, words } => {
                feed(&mut h, &[1]);
                feed(&mut h, &(palette.len() as u32).to_le_bytes());
                for p in palette {
                    feed(&mut h, &p.to_le_bytes());
                }
                for w in words.as_ref() {
                    feed(&mut h, &w.to_le_bytes());
                }
            }
        }
    }
    h
}

/// 单个区块列的不可变、已验证的基础地形快照。
///
/// 包含地形密度方块、水/含水层、基岩底面、生物群系与高度图，不含任何
/// 结构或装饰特征。
///
/// An immutable, verified base terrain snapshot for a single chunk column.
///
/// Contains terrain density blocks, water/aquifers, bedrock floor, biomes,
/// and heightmap without any structure or decoration features.
#[derive(Debug, Clone)]
pub struct BaseSnapshot {
    pub dimension: i32,
    pub chunk_position: ChunkPosition,
    pub world_instance_id: u64,
    pub generator_fingerprint: u64,
    pub min_y: i32,
    pub max_y: i32,
    pub chunk: Arc<Chunk>,
    pub height_map: [i32; 256],
    pub digest: u64,
}

impl BaseSnapshot {
    /// 构造一个已验证的 [`BaseSnapshot`] 并计算其确定性摘要。
    ///
    /// Constructs a verified [`BaseSnapshot`] and computes its deterministic
    /// digest.
    pub fn new(
        dimension: i32,
        chunk_position: ChunkPosition,
        world_instance_id: u64,
        generator_fingerprint: u64,
        min_y: i32,
        max_y: i32,
        chunk: Arc<Chunk>,
        height_map: [i32; 256],
    ) -> Self {
        let digest = compute_base_digest(
            chunk_position,
            generator_fingerprint,
            world_instance_id,
            min_y,
            max_y,
            &height_map,
            &chunk,
        );
        Self {
            dimension,
            chunk_position,
            world_instance_id,
            generator_fingerprint,
            min_y,
            max_y,
            chunk,
            height_map,
            digest,
        }
    }

    /// 读取局部坐标处的方块（第 0 层）。
    ///
    /// Read block at local coordinates (layer 0).
    pub fn block_at(&self, local_x: u8, world_y: i32, local_z: u8) -> BlockRuntimeId {
        self.chunk
            .block_at(local_x, world_y, local_z)
            .unwrap_or(BlockRuntimeId(0))
    }

    /// 读取局部坐标处的生物群系。
    ///
    /// Read biome at local coordinates.
    pub fn biome_at(&self, local_x: u8, world_y: i32, local_z: u8) -> u32 {
        let section = match self.chunk.section_of(world_y) {
            Some(s) => s,
            None => return 0,
        };
        let sub_y = (world_y.rem_euclid(16)) as u8;
        let index = ((sub_y as usize) << 8) | ((local_z as usize) << 4) | (local_x as usize);
        self.chunk
            .biomes
            .get(section)
            .and_then(|b| b.get(index))
            .unwrap_or(0)
    }

    /// 读取局部坐标处的基础表面高度。
    ///
    /// Read preliminary base surface height at local coordinates.
    pub fn surface_height_at(&self, local_x: u8, local_z: u8) -> i32 {
        let idx = ((local_x as usize) << 4) | (local_z as usize);
        self.height_map[idx]
    }

    /// 确定性的内容摘要。
    ///
    /// Deterministic content digest.
    pub fn digest(&self) -> u64 {
        self.digest
    }
}

/// 供后续生成阶段（结构、装饰、特征）使用的只读多区块邻域视图。
///
/// 保证：
/// 1. 只读取邻域中显式声明并准备好的快照；
/// 2. 缺失的相邻区块返回 [`GenerationContractError::MissingDependency`]，
///    绝不静默按空气处理；
/// 3. 垂直越界读取安全地返回 `out_of_bounds_block`。
///
/// A read-only multi-chunk neighborhood view feeding subsequent generation
/// stages (structures, decorations, features).
///
/// Guarantees:
/// 1. Only reads explicitly declared and prepared snapshots in the neighborhood;
/// 2. Missing adjacent chunks return [`GenerationContractError::MissingDependency`],
///    never silent air;
/// 3. Vertically out-of-bounds reads return `out_of_bounds_block` safely.
#[derive(Debug, Clone)]
pub struct GenerationView {
    dimension: i32,
    world_instance_id: u64,
    generator_fingerprint: u64,
    min_y: i32,
    max_y: i32,
    out_of_bounds_block: BlockRuntimeId,
    snapshots: BTreeMap<(i32, i32), Arc<BaseSnapshot>>,
}

impl GenerationView {
    /// 为某维度与实例创建空的生成视图。
    ///
    /// Creates an empty generation view for a dimension and instance.
    pub fn new(
        dimension: i32,
        world_instance_id: u64,
        generator_fingerprint: u64,
        min_y: i32,
        max_y: i32,
        out_of_bounds_block: BlockRuntimeId,
    ) -> Self {
        Self {
            dimension,
            world_instance_id,
            generator_fingerprint,
            min_y,
            max_y,
            out_of_bounds_block,
            snapshots: BTreeMap::new(),
        }
    }

    /// 创建带初始基础快照集合的生成视图。
    ///
    /// Creates a generation view populated with an initial set of base
    /// snapshots.
    pub fn with_snapshots(
        dimension: i32,
        world_instance_id: u64,
        generator_fingerprint: u64,
        min_y: i32,
        max_y: i32,
        out_of_bounds_block: BlockRuntimeId,
        snapshots: Vec<Arc<BaseSnapshot>>,
    ) -> Result<Self, GenerationContractError> {
        let mut view = Self::new(
            dimension,
            world_instance_id,
            generator_fingerprint,
            min_y,
            max_y,
            out_of_bounds_block,
        );
        for snap in snapshots {
            view.insert_snapshot(snap)?;
        }
        Ok(view)
    }

    /// 校验维度、实例、指纹与垂直边界后，将快照插入视图。
    ///
    /// Inserts a snapshot into the view after validating dimension,
    /// instance, fingerprint, and vertical bounds.
    pub fn insert_snapshot(&mut self, snapshot: Arc<BaseSnapshot>) -> Result<(), GenerationContractError> {
        if snapshot.dimension != self.dimension {
            return Err(GenerationContractError::InvalidConfiguration(format!(
                "snapshot dimension {} does not match view dimension {}",
                snapshot.dimension, self.dimension
            )));
        }
        if snapshot.world_instance_id != self.world_instance_id {
            return Err(GenerationContractError::InvalidConfiguration(format!(
                "snapshot world instance {:#018x} does not match view instance {:#018x}",
                snapshot.world_instance_id, self.world_instance_id
            )));
        }
        if snapshot.generator_fingerprint != self.generator_fingerprint {
            return Err(GenerationContractError::InvalidConfiguration(format!(
                "snapshot generator fingerprint {:#018x} does not match view fingerprint {:#018x}",
                snapshot.generator_fingerprint, self.generator_fingerprint
            )));
        }
        if snapshot.min_y != self.min_y || snapshot.max_y != self.max_y {
            return Err(GenerationContractError::InvalidConfiguration(format!(
                "snapshot vertical bounds [{}, {}] do not match view [{}, {}]",
                snapshot.min_y, snapshot.max_y, self.min_y, self.max_y
            )));
        }

        let key = (snapshot.chunk_position.x, snapshot.chunk_position.z);
        self.snapshots.insert(key, snapshot);
        Ok(())
    }

    /// 视图的维度。
    ///
    /// Dimension of the view.
    pub fn dimension(&self) -> i32 {
        self.dimension
    }

    /// 世界实例 ID。
    ///
    /// World instance ID.
    pub fn world_instance_id(&self) -> u64 {
        self.world_instance_id
    }

    /// 生成器描述符指纹。
    ///
    /// Generator descriptor fingerprint.
    pub fn generator_fingerprint(&self) -> u64 {
        self.generator_fingerprint
    }

    /// 视图是否包含区块 (cx, cz) 的已准备快照。
    ///
    /// Whether the view contains a prepared snapshot for chunk (cx, cz).
    pub fn contains_chunk(&self, chunk_x: i32, chunk_z: i32) -> bool {
        self.snapshots.contains_key(&(chunk_x, chunk_z))
    }

    /// 查找区块 (cx, cz) 的快照。
    ///
    /// Looks up a snapshot for chunk (cx, cz).
    pub fn snapshot(&self, chunk_x: i32, chunk_z: i32) -> Option<&Arc<BaseSnapshot>> {
        self.snapshots.get(&(chunk_x, chunk_z))
    }

    /// 当前视图中的全部快照。
    ///
    /// All snapshots currently present in this view.
    pub fn snapshots(&self) -> &BTreeMap<(i32, i32), Arc<BaseSnapshot>> {
        &self.snapshots
    }

    /// 读取任意世界坐标处的方块。
    ///
    /// - `world_y` 在 `[min_y, max_y]` 之外时返回 `Ok(out_of_bounds_block)`；
    /// - 目标区块不在快照集合中时返回
    ///   [`GenerationContractError::MissingDependency`]；
    /// - 存在时返回真实快照的 `Ok(block_runtime_id)`。
    ///
    /// Reads a block at arbitrary world coordinates.
    ///
    /// - If `world_y` is outside `[min_y, max_y]`, returns `Ok(out_of_bounds_block)`.
    /// - If the target chunk is absent from the snapshot set, returns
    ///   [`GenerationContractError::MissingDependency`].
    /// - If present, returns `Ok(block_runtime_id)` from the real snapshot.
    pub fn block_at(
        &self,
        world_x: i32,
        world_y: i32,
        world_z: i32,
    ) -> Result<BlockRuntimeId, GenerationContractError> {
        if world_y < self.min_y || world_y > self.max_y {
            return Ok(self.out_of_bounds_block);
        }

        let chunk_x = world_x.div_euclid(16);
        let chunk_z = world_z.div_euclid(16);

        let snapshot = self.snapshots.get(&(chunk_x, chunk_z)).ok_or_else(|| {
            GenerationContractError::MissingDependency {
                dimension: self.dimension,
                chunk_x,
                chunk_z,
                stage: "base",
            }
        })?;

        let local_x = world_x.rem_euclid(16) as u8;
        let local_z = world_z.rem_euclid(16) as u8;
        Ok(snapshot.block_at(local_x, world_y, local_z))
    }

    /// 读取任意世界坐标处的生物群系。
    ///
    /// Reads a biome at arbitrary world coordinates.
    pub fn biome_at(
        &self,
        world_x: i32,
        world_y: i32,
        world_z: i32,
    ) -> Result<u32, GenerationContractError> {
        let chunk_x = world_x.div_euclid(16);
        let chunk_z = world_z.div_euclid(16);

        let snapshot = self.snapshots.get(&(chunk_x, chunk_z)).ok_or_else(|| {
            GenerationContractError::MissingDependency {
                dimension: self.dimension,
                chunk_x,
                chunk_z,
                stage: "base",
            }
        })?;

        let local_x = world_x.rem_euclid(16) as u8;
        let local_z = world_z.rem_euclid(16) as u8;
        Ok(snapshot.biome_at(local_x, world_y, local_z))
    }

    /// 读取任意世界 (x, z) 坐标处的基础表面高度。
    ///
    /// Reads preliminary surface height at arbitrary world (x, z) coordinates.
    pub fn surface_height_at(
        &self,
        world_x: i32,
        world_z: i32,
    ) -> Result<i32, GenerationContractError> {
        let chunk_x = world_x.div_euclid(16);
        let chunk_z = world_z.div_euclid(16);

        let snapshot = self.snapshots.get(&(chunk_x, chunk_z)).ok_or_else(|| {
            GenerationContractError::MissingDependency {
                dimension: self.dimension,
                chunk_x,
                chunk_z,
                stage: "base",
            }
        })?;

        let local_x = world_x.rem_euclid(16) as u8;
        let local_z = world_z.rem_euclid(16) as u8;
        Ok(snapshot.surface_height_at(local_x, local_z))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_chunk(pos: ChunkPosition, fill_y_range: std::ops::RangeInclusive<i32>, block: BlockRuntimeId) -> (Arc<Chunk>, [i32; 256]) {
        let mut chunk = Chunk::empty_overworld(pos);
        let mut height_map = [-64i32; 256];

        for local_x in 0u8..16 {
            for local_z in 0u8..16 {
                for y in fill_y_range.clone() {
                    chunk.set_block_at(0, local_x, y, local_z, block);
                    let idx = ((local_x as usize) << 4) | (local_z as usize);
                    if y > height_map[idx] {
                        height_map[idx] = y;
                    }
                }
            }
        }
        (Arc::new(chunk), height_map)
    }

    #[test]
    fn generation_view_reads_real_neighborhood_and_rejects_missing_as_missing_dependency() {
        let air = BlockRuntimeId(crate::block_dictionary::air_runtime_id());
        const STONE: BlockRuntimeId = BlockRuntimeId(1);
        const WATER: BlockRuntimeId = BlockRuntimeId(2);

        let instance = 0x11223344;
        let gen_fp = 0x55667788;

        // Center chunk (0, 0): stone up to Y=64
        let (c00, hm00) = make_test_chunk(ChunkPosition::new(0, 0), -64..=64, STONE);
        let s00 = Arc::new(BaseSnapshot::new(0, ChunkPosition::new(0, 0), instance, gen_fp, -64, 319, c00, hm00));

        // East neighbor chunk (1, 0): cliff stone up to Y=120
        let (c10, hm10) = make_test_chunk(ChunkPosition::new(1, 0), -64..=120, STONE);
        let s10 = Arc::new(BaseSnapshot::new(0, ChunkPosition::new(1, 0), instance, gen_fp, -64, 319, c10, hm10));

        // South neighbor chunk (0, 1): ocean water up to Y=63
        let (c01, hm01) = make_test_chunk(ChunkPosition::new(0, 1), -64..=63, WATER);
        let s01 = Arc::new(BaseSnapshot::new(0, ChunkPosition::new(0, 1), instance, gen_fp, -64, 319, c01, hm01));

        // Create view with (0,0), (1,0), (0,1).
        // West (-1,0) and North (0,-1) are intentionally absent.
        let view = GenerationView::with_snapshots(0, instance, gen_fp, -64, 319, air, vec![s00, s10, s01])
            .expect("view construction succeeds");

        // 1. Reading inside (0, 0)
        assert_eq!(view.block_at(8, 64, 8).unwrap(), STONE);
        assert_eq!(view.block_at(8, 65, 8).unwrap(), air);
        assert_eq!(view.surface_height_at(8, 8).unwrap(), 64);

        // 2. Reading inside (1, 0): cliff is present, NOT air!
        assert_eq!(view.block_at(16, 100, 8).unwrap(), STONE, "cliff stone read across chunk border");
        assert_eq!(view.surface_height_at(16, 8).unwrap(), 120);

        // 3. Reading inside (0, 1): water read across chunk border
        assert_eq!(view.block_at(8, 60, 16).unwrap(), WATER, "ocean water read across chunk border");
        assert_eq!(view.surface_height_at(8, 16).unwrap(), 63);

        // 4. Reading inside absent west chunk (-1, 0): must be MissingDependency, NEVER AIR!
        let missing_err = view.block_at(-1, 64, 8).unwrap_err();
        assert_eq!(
            missing_err,
            GenerationContractError::MissingDependency {
                dimension: 0,
                chunk_x: -1,
                chunk_z: 0,
                stage: "base"
            },
            "absent neighbor must loudly report MissingDependency instead of disguised air"
        );
        assert!(matches!(view.surface_height_at(-1, 8), Err(GenerationContractError::MissingDependency { .. })));

        // 5. Vertical bounds clamp safely to boundary block
        assert_eq!(view.block_at(8, -100, 8).unwrap(), air);
        assert_eq!(view.block_at(8, 400, 8).unwrap(), air);
    }

    #[test]
    fn base_snapshot_digest_is_deterministic_and_changes_with_content() {
        const STONE: BlockRuntimeId = BlockRuntimeId(1);
        const DIRT: BlockRuntimeId = BlockRuntimeId(3);

        let (c1, hm1) = make_test_chunk(ChunkPosition::new(0, 0), -64..=64, STONE);
        let s1 = BaseSnapshot::new(0, ChunkPosition::new(0, 0), 1, 2, -64, 319, c1, hm1);

        let (c2, hm2) = make_test_chunk(ChunkPosition::new(0, 0), -64..=64, STONE);
        let s2 = BaseSnapshot::new(0, ChunkPosition::new(0, 0), 1, 2, -64, 319, c2, hm2);

        assert_eq!(s1.digest(), s2.digest(), "identical content produces identical digest");

        let (c3, hm3) = make_test_chunk(ChunkPosition::new(0, 0), -64..=64, DIRT);
        let s3 = BaseSnapshot::new(0, ChunkPosition::new(0, 0), 1, 2, -64, 319, c3, hm3);

        assert_ne!(s1.digest(), s3.digest(), "different block content produces different digest");
    }
}
