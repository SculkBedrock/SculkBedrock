//! Normal 阶段链之上的世界生成器适配器。
//!
//! 构造时组装地形、群系、地表、特征与结构填充各阶段；generate_chunk 按顺序同步运行整条链并返回生成完成的区块。
//!
//! World generator adapter over the Normal stage chain.
//!
//! Construction assembles the terrain, biome, surface, feature, and populator stages; generate_chunk runs the chain synchronously and returns the finished chunk.

use std::sync::Arc;

use crate::blocks_table::{OreBlockTable, WorldgenBlockTable};
use crate::worldgen::chunk::WorldgenChunk;
use crate::worldgen::context::ChunkGenerateContext;
use crate::worldgen::densityfunction::common::CellFunctionContext;
use crate::worldgen::densityfunction::function::ChunkCache;
use crate::worldgen::feature::decoration::DecorationBlockTable;
use crate::worldgen::feature::object::TreeBlockTable;
use crate::worldgen::holder::normal::NormalObjectHolder;
use crate::worldgen::material::MaterialBlocks;
use crate::worldgen::populator::structures::StructureBlockTable;
use crate::worldgen::random::Xoroshiro128;
use crate::worldgen::stages::biome_map::BiomeMapStage;
use crate::worldgen::stages::chunk_feature::{FinishedStage, NormalChunkFeatureStage};
use crate::worldgen::stages::generated::GeneratedStage;
use crate::worldgen::stages::populator::NormalPopulatorStage;
use crate::worldgen::stages::surface_data::NormalSurfaceDataStage;
use crate::worldgen::stages::surface_overwrite::NormalSurfaceOverwriteStage;
use crate::worldgen::stages::terrain::{NormalTerrainStage, SEA_LEVEL};
use crate::worldgen::stages::{run_stages, GenerateStage, GenerateStageBuilder};
use std::collections::VecDeque;
use std::rc::Rc;
use sc_world::chunk::{BlockRuntimeId, Chunk, ChunkPosition, OVERWORLD_MAX_Y, OVERWORLD_MIN_Y};
use sc_world::storage::{
    BlockSpilloverWrite, ChunkGenerationRequest, ChunkKey, GeneratedChunk, WorldGenerator,
    WorldStorageError,
};

/// 实现 [`WorldGenerator`] 的常规世界生成器。
///
/// Normal world generator implementing [`WorldGenerator`].
///
/// 持有每世界的阶段链、方块表与种子。每次 `generate_chunk` 同步运行
/// 整条链（地形 → 生物群系 → 地表 → 矿物特征 → 收尾）。
///
/// Holds the per-world stage chain, block tables, and seed. Each
/// `generate_chunk` call runs the full chain synchronously
/// (terrain → biomes → surface → ore features → finished).
///
/// 流水线：
/// `NormalTerrain → BiomeMap → SurfaceData → SurfaceOverwrite → Generated
///  → Feature → Finished`
///
/// Pipeline:
/// `NormalTerrain → BiomeMap → SurfaceData → SurfaceOverwrite → Generated
///  → Feature → Finished`
pub struct NormalGenerator {
    level_seed: i64,
    material_blocks: MaterialBlocks,
    stages: Vec<Box<dyn GenerateStage>>,
}

impl NormalGenerator {
    /// 由世界种子与方块表构建生成器，组装阶段链。
    ///
    /// Builds the generator from a world seed and block tables, assembling
    /// the stage chain.
    pub fn new(
        level_seed: i64,
        table: WorldgenBlockTable,
        material_blocks: MaterialBlocks,
    ) -> Self {
        Self::new_with_snapshot(level_seed, table, material_blocks, None)
    }

    /// 感知快照的构造函数：存在方块数据快照时，子表（矿物/树木/装饰/结构）
    /// 从 bundle 取默认值；`None` 时回退到核心调色板。
    ///
    /// Snapshot-aware constructor: when a block-data snapshot is present,
    /// the sub-tables (ore/tree/decoration/structure) take their defaults
    /// from the bundle; with `None` they fall back to the core palette.
    pub fn new_with_snapshot(
        level_seed: i64,
        table: WorldgenBlockTable,
        material_blocks: MaterialBlocks,
        snapshot: Option<std::sync::Arc<sc_block::block_json::BlockJsonSnapshot>>,
    ) -> Self {
        let snapshot_ref = snapshot.as_deref();
        let ore_table = match snapshot_ref {
            Some(snap) => OreBlockTable::from_block_snapshot(snap),
            None => OreBlockTable::from_core_palette(),
        };
        let tree_table = Arc::new(match snapshot_ref {
            Some(snap) => TreeBlockTable::from_block_snapshot(snap),
            None => TreeBlockTable::from_core_palette(),
        });
        let decoration_table = Arc::new(match snapshot_ref {
            Some(snap) => DecorationBlockTable::from_block_snapshot(snap),
            None => DecorationBlockTable::from_core_palette(),
        });
        let structure_table = Arc::new(match snapshot_ref {
            Some(snap) => StructureBlockTable::from_block_snapshot(snap),
            None => StructureBlockTable::from_core_palette(),
        });
        let mut builder = GenerateStageBuilder::new();
        builder
            .start(Box::new(NormalTerrainStage::new(
                table.stone,
                table.deepslate,
                table.bedrock,
                table.air,
            )))
            .next(Box::new(BiomeMapStage::new()))
            .next(Box::new(NormalSurfaceDataStage::new(table.clone())))
            .next(Box::new(NormalSurfaceOverwriteStage::new(table.clone())))
            .next(Box::new(GeneratedStage::new()))
            .next(Box::new(NormalChunkFeatureStage::with_decoration_features(
                ore_table,
                tree_table,
                decoration_table,
            )))
            .next(Box::new(NormalPopulatorStage::new(structure_table)))
            .next(Box::new(FinishedStage::new()));
        let stages = builder.build();

        Self {
            level_seed,
            material_blocks,
            stages,
        }
    }
}

impl WorldGenerator for NormalGenerator {
    /// 生成单个 chunk（trait 入口，丢弃跨 chunk 溢出写入）。
    ///
    /// Generates a single chunk (trait entry; drops cross-chunk spillover
    /// writes).
    fn generate_chunk(
        &self,
        request: ChunkGenerationRequest,
    ) -> Result<Option<Chunk>, WorldStorageError> {
        self.generate_chunk_with_spillover(request)
            .map(|generated| generated.map(|g| g.chunk))
    }

    /// 生成单个 chunk 并返回跨 chunk 溢出写入（使用默认上限）。
    ///
    /// Generates a single chunk and returns cross-chunk spillover writes
    /// (with the default bound).
    fn generate_chunk_with_spillover(
        &self,
        request: ChunkGenerationRequest,
    ) -> Result<Option<GeneratedChunk>, WorldStorageError> {
        self.generate_chunk_with_spillover_bounded(
            request,
            sc_world::storage::MAX_GENERATED_SPILLOVER_WRITES,
        )
    }

    /// 生成单个 chunk 并返回跨 chunk 溢出写入（可指定写入上限）。
    ///
    /// Generates a single chunk and returns cross-chunk spillover writes
    /// (with a configurable write bound).
    fn generate_chunk_with_spillover_bounded(
        &self,
        request: ChunkGenerationRequest,
        max_spillover_writes: usize,
    ) -> Result<Option<GeneratedChunk>, WorldStorageError> {
        let key = request.key;
        let dimension = key.dimension;
        let chunk = Chunk::empty(key.position, key.dimension, request.min_y, request.max_y);
        let mut wc = WorldgenChunk::new(chunk);

        // Per-chunk holder: same seed yields same noise (deterministic);
        // the aquifer keeps per-chunk state.
        let random = Xoroshiro128::new(self.level_seed);
        let holder = NormalObjectHolder::new(random, self.material_blocks.clone());

        let mut ctx = ChunkGenerateContext::new_with_spillover_limit(
            &mut wc,
            &holder,
            self.level_seed,
            request.min_y,
            request.max_y,
            max_spillover_writes,
        );

        // Run the full stage chain synchronously up to FinishedStage
        // (terrain + features + populators + finished).
        run_stages(&self.stages, 0, "finished", &mut ctx);

        if ctx.spillover_overflowed() {
            return Err(WorldStorageError::capacity(format!(
                "worldgen spillover exceeded the bounded {}-write result",
                max_spillover_writes
            )));
        }

        // Cross-chunk spillover writes (edge tree canopies, overhanging
        // structures) are handed to the provider for routing.
        let spillover = ctx
            .take_spillover()
            .into_iter()
            .map(|entry| BlockSpilloverWrite {
                key: ChunkKey::new(dimension, ChunkPosition::new(entry.x >> 4, entry.z >> 4)),
                x: entry.x,
                y: entry.y,
                z: entry.z,
                layer: entry.layer,
                block: entry.block,
            })
            .collect();

        Ok(Some(GeneratedChunk {
            chunk: wc.into_inner(),
            spillover,
        }))
    }
}

// ---------------------------------------------------------------------------
// Land-spawn search (stopgap: ignores level.dat, locates the largest landmass)
// ---------------------------------------------------------------------------

/// 粗栅格扫描步长（方块）。大陆横跨数千方块，128 步长足以勾勒陆地轮廓。
///
/// Coarse-grid scan step (blocks). Continents span thousands of blocks, so a
/// 128-step is enough to resolve land outlines.
const LAND_SCAN_STEP: i32 = 128;
/// 粗栅格扫描半径（方块）：±4096 → 65×65 个采样点。
///
/// Coarse-grid scan radius (blocks): ±4096 → 65×65 sample points.
const LAND_SCAN_RADIUS: i32 = 4096;
/// 精扫半径（方块）：质心周围 ±96，取最靠内的点。
///
/// Refinement scan radius (blocks): ±96 around the centroid, picking the
/// most inland point.
const LAND_REFINE_RADIUS: i32 = 96;
/// 精扫步长（方块）。
///
/// Refinement scan step (blocks).
const LAND_REFINE_STEP: i32 = 16;

/// 临时出生点策略：忽略 level.dat 出生点，定位世界中最大的陆地。
///
/// Stopgap spawn strategy: ignores the level.dat spawn and locates the
/// largest landmass in the world.
///
/// 步骤：
/// 1. 在与 [`NormalTerrainStage`] 的 mandatoryTopY 同源的纯 2D noise 函数
///    `preliminary_surface_upper_bound` 上，以 ±4096 方块、128 步长的粗栅格
///    采样；高于海平面记作陆地——无需生成 chunk；
/// 2. BFS 找最大的 4-连通陆地块，取最接近其质心的采样点；
/// 3. 在质心附近精扫（±96，取上界最大的最靠内点）；
/// 4. 在该点生成一个真实 chunk，自顶向下扫描出生点列第一个非空气非水方块，
///    得到精确出生 y。
///
/// Steps:
/// 1. Sample `preliminary_surface_upper_bound` (a pure 2D noise function
///    sharing its source with [`NormalTerrainStage`]'s mandatoryTopY) on a
///    128-step coarse grid over ±4096 blocks; values above sea level count
///    as land — no chunk generation needed;
/// 2. BFS for the largest 4-connected land component, taking the sample
///    point nearest its centroid;
/// 3. Refine around the centroid (±96, most inland = largest upper bound);
/// 4. Generate one real chunk at that point and scan the spawn column
///    top-down for the first non-air, non-water block to get the exact
///    spawn y.
///
/// 范围内找不到陆地时返回 `None`（极端种子）；由调用方回退。
///
/// Returns `None` when no land is found in range (extreme seeds); the
/// caller falls back.
pub fn find_land_spawn(
    generator: &NormalGenerator,
    material_blocks: &MaterialBlocks,
    air: BlockRuntimeId,
    water: BlockRuntimeId,
) -> Option<(i32, i32, i32)> {
    // Holder built the same way as in `generate_chunk`: same seed yields
    // same noise (deterministic).
    let random = Xoroshiro128::new(generator.level_seed);
    let holder = NormalObjectHolder::new(random, material_blocks.clone());
    let cache = Rc::new(ChunkCache::new());
    cache.clear();
    let ctx = CellFunctionContext::new(cache);
    let upper_bound = holder.terrain_holder().preliminary_surface_upper_bound();

    // --- 1. Coarse-grid land classification ---
    let n = ((2 * LAND_SCAN_RADIUS / LAND_SCAN_STEP) + 1) as usize;
    let mut land = vec![false; n * n];
    for iz in 0..n {
        let z = -LAND_SCAN_RADIUS + iz as i32 * LAND_SCAN_STEP;
        for ix in 0..n {
            let x = -LAND_SCAN_RADIUS + ix as i32 * LAND_SCAN_STEP;
            let v = upper_bound.compute(ctx.set(x, 0, z));
            land[iz * n + ix] = v > SEA_LEVEL as f64;
        }
    }

    // --- 2. Largest 4-connected land component (BFS) ---
    let mut visited = vec![false; n * n];
    let mut best_component: Vec<usize> = Vec::new();
    for start in 0..n * n {
        if !land[start] || visited[start] {
            continue;
        }
        let mut component = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(start);
        visited[start] = true;
        while let Some(idx) = queue.pop_front() {
            component.push(idx);
            let (ix, iz) = (idx % n, idx / n);
            // 4-connected neighbors (right/left/down/up)
            for (nx, nz) in [
                (ix + 1, iz),
                (ix.wrapping_sub(1), iz),
                (ix, iz + 1),
                (ix, iz.wrapping_sub(1)),
            ] {
                if nx < n && nz < n {
                    let nidx = nz * n + nx;
                    if land[nidx] && !visited[nidx] {
                        visited[nidx] = true;
                        queue.push_back(nidx);
                    }
                }
            }
        }
        if component.len() > best_component.len() {
            best_component = component;
        }
    }
    if best_component.is_empty() {
        return None;
    }

    // --- 3. Nearest-to-centroid sample → refine to the most inland point ---
    let sum: (i64, i64) = best_component.iter().fold((0, 0), |(sx, sz), &idx| {
        (sx + (idx % n) as i64, sz + (idx / n) as i64)
    });
    let (ccx, ccz) = (
        sum.0 / best_component.len() as i64,
        sum.1 / best_component.len() as i64,
    );
    let center = *best_component
        .iter()
        .min_by_key(|&&idx| {
            let dx = (idx % n) as i64 - ccx;
            let dz = (idx / n) as i64 - ccz;
            dx * dx + dz * dz
        })
        .unwrap();
    let (bx, bz) = (
        -LAND_SCAN_RADIUS + (center % n) as i32 * LAND_SCAN_STEP,
        -LAND_SCAN_RADIUS + (center / n) as i32 * LAND_SCAN_STEP,
    );
    let (mut fx, mut fz, mut fv) = (bx, bz, f64::MIN);
    for z in (bz - LAND_REFINE_RADIUS..=bz + LAND_REFINE_RADIUS).step_by(LAND_REFINE_STEP as usize)
    {
        for x in
            (bx - LAND_REFINE_RADIUS..=bx + LAND_REFINE_RADIUS).step_by(LAND_REFINE_STEP as usize)
        {
            let v = upper_bound.compute(ctx.set(x, 0, z));
            if v > fv {
                fv = v;
                fx = x;
                fz = z;
            }
        }
    }

    // --- 4. Generate one real chunk, scan the spawn column for exact surface y ---
    let request = ChunkGenerationRequest {
        key: ChunkKey {
            dimension: 0,
            position: ChunkPosition::new(fx.div_euclid(16), fz.div_euclid(16)),
        },
        min_y: OVERWORLD_MIN_Y,
        max_y: OVERWORLD_MAX_Y,
    };
    let chunk = generator.generate_chunk(request).ok()??;
    let (lx, lz) = (fx.rem_euclid(16) as u8, fz.rem_euclid(16) as u8);
    let mut top: Option<i32> = None;
    let mut y = OVERWORLD_MAX_Y;
    while y >= OVERWORLD_MIN_Y {
        if let Some(block) = chunk.block_at(lx, y, lz) {
            if block != air && block != water {
                top = Some(y);
                break;
            }
        }
        y -= 1;
    }
    let top = top?; // Whole column is empty/water (unexpected on land) → give up, caller falls back
    Some((fx, top + 1, fz))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks_table::WorldgenBlockTable;
    use crate::worldgen::material::MaterialBlocks;
    use sc_world::chunk::{BlockRuntimeId, ChunkPosition, OVERWORLD_MAX_Y, OVERWORLD_MIN_Y};
    use sc_world::storage::ChunkKey;

    /// Builds a block table with distinct dummy block ids so the water
    /// branch of the surface-data stage cannot loop forever when
    /// water == air without a version pack (same approach as the
    /// terrain.rs unit test).
    fn test_table() -> WorldgenBlockTable {
        let id = |n: u32| BlockRuntimeId(n);
        WorldgenBlockTable {
            air: id(0),
            stone: id(1),
            deepslate: id(2),
            bedrock: id(3),
            water: id(4),
            flowing_water: id(5),
            dirt: id(6),
            coarse_dirt: id(7),
            grass_block: id(8),
            sand: id(9),
            sandstone: id(10),
            gravel: id(11),
            hardened_clay: id(12),
            orange_terracotta: id(13),
            white_terracotta: id(14),
            yellow_terracotta: id(15),
            brown_terracotta: id(16),
            red_terracotta: id(17),
            light_gray_terracotta: id(18),
            snow_layer: id(19),
            snow_block: id(20),
            ice: id(21),
            packed_ice: id(22),
            mycelium: id(23),
            podzol: id(24),
            red_sand: id(25),
            red_sandstone: id(26),
            terracotta: id(27),
        }
    }

    fn test_material_blocks() -> MaterialBlocks {
        let id = |n: u32| BlockRuntimeId(n);
        MaterialBlocks {
            air: id(0),
            water: id(4),
            lava: id(28),
            stone: id(1),
            granite: id(29),
            tuff: id(30),
            copper_ore: id(31),
            deepslate_iron_ore: id(32),
            raw_copper_block: id(33),
            raw_iron_block: id(34),
        }
    }

    fn make_generator(seed: i64) -> NormalGenerator {
        NormalGenerator::new(seed, test_table(), test_material_blocks())
    }

    fn make_request(cx: i32, cz: i32) -> ChunkGenerationRequest {
        ChunkGenerationRequest {
            key: ChunkKey {
                dimension: 0,
                position: ChunkPosition::new(cx, cz),
            },
            min_y: OVERWORLD_MIN_Y,
            max_y: OVERWORLD_MAX_Y,
        }
    }

    #[test]
    fn find_land_spawn_finds_land_above_sea() {
        let table = test_table();
        let gen = make_generator(42);
        let (x, y, z) = find_land_spawn(&gen, &test_material_blocks(), table.air, table.water)
            .expect("land spawn should be found within ±4096");
        // Spawn y must be above sea level (on land)
        assert!(
            y > SEA_LEVEL,
            "spawn y={y} should be above sea level {SEA_LEVEL}"
        );
        // The block directly below spawn must be solid (neither air nor water)
        let chunk = gen
            .generate_chunk(make_request(x.div_euclid(16), z.div_euclid(16)))
            .unwrap()
            .unwrap();
        let below = chunk
            .block_at(x.rem_euclid(16) as u8, y - 1, z.rem_euclid(16) as u8)
            .expect("block below spawn should exist");
        assert_ne!(below, table.air, "block below spawn must not be air");
        assert_ne!(below, table.water, "block below spawn must not be water");
    }

    #[test]
    fn generator_produces_nonempty_chunk() {
        let gen = make_generator(42);
        let result = gen.generate_chunk(make_request(0, 0)).unwrap();
        assert!(result.is_some(), "generator should return a chunk");
        let chunk = result.unwrap();
        // At least one non-air block must exist (bedrock at min_y)
        let air = BlockRuntimeId(0);
        let mut found_solid = false;
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                if chunk.block_at(lx, OVERWORLD_MIN_Y, lz) != Some(air) {
                    found_solid = true;
                    break;
                }
            }
            if found_solid {
                break;
            }
        }
        assert!(
            found_solid,
            "chunk should have solid blocks at bedrock level"
        );
    }

    #[test]
    fn generator_is_deterministic() {
        let gen = make_generator(999);

        let chunk1 = gen.generate_chunk(make_request(3, -2)).unwrap().unwrap();
        let chunk2 = gen.generate_chunk(make_request(3, -2)).unwrap().unwrap();

        // Compare sample points
        for (x, y, z) in [
            (0u8, OVERWORLD_MIN_Y, 0u8),
            (8u8, 0, 8u8),
            (3u8, 63, 7u8),
            (15u8, 100, 15u8),
        ] {
            assert_eq!(
                chunk1.block_at(x, y, z),
                chunk2.block_at(x, y, z),
                "block mismatch at ({x},{y},{z})"
            );
        }
    }

    #[test]
    fn different_chunks_differ() {
        let gen = make_generator(42);

        let chunk1 = gen.generate_chunk(make_request(0, 0)).unwrap().unwrap();
        let chunk2 = gen.generate_chunk(make_request(1, 0)).unwrap().unwrap();

        // Different chunks should have different terrain (overwhelmingly likely)
        let mut differ = false;
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                for y in (-40..120i32).step_by(4) {
                    if chunk1.block_at(lx, y, lz) != chunk2.block_at(lx, y, lz) {
                        differ = true;
                        break;
                    }
                }
                if differ {
                    break;
                }
            }
            if differ {
                break;
            }
        }
        assert!(differ, "adjacent chunks should have different terrain");
    }

    #[test]
    fn different_seeds_differ() {
        let gen1 = make_generator(111);
        let gen2 = make_generator(222);

        let chunk1 = gen1.generate_chunk(make_request(0, 0)).unwrap().unwrap();
        let chunk2 = gen2.generate_chunk(make_request(0, 0)).unwrap().unwrap();

        // Different seeds should produce different terrain (samples the full
        // height range to make sure differences are caught)
        let mut differ = false;
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                for y in (-40..120i32).step_by(4) {
                    if chunk1.block_at(lx, y, lz) != chunk2.block_at(lx, y, lz) {
                        differ = true;
                        break;
                    }
                }
                if differ {
                    break;
                }
            }
            if differ {
                break;
            }
        }
        assert!(differ, "different seeds should produce different terrain");
    }

    // ===================== Single-chunk approximation mechanics =====================
    //
    // The feature layer uses the single-chunk approximation: out-of-bounds
    // block writes are dropped before reaching the root buffer (the
    // "Single-chunk approximation" branch in `worldgen/feature/decoration.rs`),
    // so `GeneratedChunk.spillover` is empty on the production path and edge
    // tree canopies are truncated structurally. The context-level spillover
    // write mechanism is covered by `worldgen::context` tests; its Vec order
    // follows the `root_places` HashMap iteration order
    // (`context.rs::apply_root_to_chunk`), not a stable identity contract.

    /// 生成的 chunk 不产生跨 chunk 溢出写入：特征层会过滤掉越界写入
    /// （单 chunk 近似）。
    ///
    /// Generated chunks produce no spillover writes because the feature
    /// layer filters out-of-chunk writes (single-chunk approximation).
    #[test]
    fn single_chunk_approximation_skips_cross_chunk_feature_writes() {
        let gen = make_generator(42);
        for cx in -1..=1 {
            for cz in -1..=1 {
                let result = gen
                    .generate_chunk_with_spillover(make_request(cx, cz))
                    .unwrap()
                    .expect("chunk should generate");
                assert!(
                    result.spillover.is_empty(),
                    "chunk ({cx},{cz}) unexpectedly produced {} cross-chunk writes; \
                     the single-chunk approximation no longer holds",
                    result.spillover.len()
                );
            }
        }
    }

}
