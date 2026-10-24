//! 基地形生成阶段：在 3D 单元格格点（默认 4x4x8 方块）上从 [`WorldgenRuntime`]
//! 求值地形密度，在单元格内做三线性插值，填充基岩、深板岩过渡、石头与
//! 海水，更新初步高度图，并将结果打包为不可变的 [`BaseSnapshot`]。
//!
//! Base terrain generation stage: evaluates terrain density from
//! [`WorldgenRuntime`] on a 3D cell lattice (default 4x4x8 blocks), applies
//! trilinear interpolation within each cell, fills bedrock, deepslate
//! transition, stone and sea water, updates the preliminary heightmap, and
//! packages the result into an immutable [`BaseSnapshot`].
//!
//! 生成器只做纯 CPU 计算，返回带完整身份、指纹与内容摘要的 [`BaseSnapshot`]，
//! 不修改世界状态。单元格角点求值复用 [`WorldgenRuntime`] 的池化缓冲。
//! 未生成区域在 [`GenerationView`] 中表示为缺失依赖，不伪装成空气。
//!
//! The generator performs pure CPU computation and returns a [`BaseSnapshot`]
//! carrying its full identity, fingerprint and content digest; it mutates no
//! world state. Cell corner evaluations reuse pooled buffers from
//! [`WorldgenRuntime`]. Ungenerated areas are represented as missing
//! dependencies in [`GenerationView`], never as air.

use std::sync::Arc;

use sc_world::chunk::{BlockRuntimeId, Chunk, ChunkPosition};
use sc_world::generation::view::BaseSnapshot;

use crate::eval::{EvalError, SampleContext};
use crate::noise::LegacyRandom;
use crate::runtime::WorldgenRuntime;

/// 原版主世界海平面（Y=63）。
///
/// Sea level for vanilla Overworld (Y=63).
pub const DEFAULT_SEA_LEVEL: i32 = 63;

/// 水平单元格大小（4x4 方块）。
///
/// Horizontal cell size in blocks (4x4).
pub const CELL_SIZE_XZ: usize = 4;

/// 垂直单元格大小（8 方块）。
///
/// Vertical cell size in blocks (8 blocks).
pub const CELL_SIZE_Y: usize = 8;

/// 基地形生成使用的方块运行时调色板配置。
///
/// Block runtime palette configuration for base terrain generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaseBlockPalette {
    pub stone: BlockRuntimeId,
    pub deepslate: BlockRuntimeId,
    pub bedrock: BlockRuntimeId,
    pub water: BlockRuntimeId,
    pub air: BlockRuntimeId,
}

/// 将密度计划执行并落成 [`BaseSnapshot`] 的基地形生成器。
///
/// Base terrain generator executing density plans into [`BaseSnapshot`]s.
pub struct BaseTerrainGenerator {
    runtime: Arc<WorldgenRuntime>,
    dimension: i32,
    world_instance_id: u64,
    generator_fingerprint: u64,
    sea_level: i32,
    palette: BaseBlockPalette,
}

impl BaseTerrainGenerator {
    /// 创建绑定到运行时与世界身份的基地形生成器。
    ///
    /// Creates a base terrain generator bound to a runtime and world identity.
    pub fn new(
        runtime: Arc<WorldgenRuntime>,
        dimension: i32,
        world_instance_id: u64,
        generator_fingerprint: u64,
        palette: BaseBlockPalette,
    ) -> Self {
        Self {
            runtime,
            dimension,
            world_instance_id,
            generator_fingerprint,
            sea_level: DEFAULT_SEA_LEVEL,
            palette,
        }
    }

    /// 设置自定义海平面（默认 63）。
    ///
    /// Sets a custom sea level (default 63).
    pub fn with_sea_level(mut self, sea_level: i32) -> Self {
        self.sea_level = sea_level;
        self
    }

    /// 本生成器的海平面。
    ///
    /// Sea level for this generator.
    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    /// 绑定的运行时。
    ///
    /// Bound runtime.
    pub fn runtime(&self) -> &Arc<WorldgenRuntime> {
        &self.runtime
    }

    /// 为单个区块位置生成基地形，产出自持有的 [`BaseSnapshot`]。
    ///
    /// Generates base terrain for one chunk position, producing an owned
    /// [`BaseSnapshot`].
    pub fn generate_base_chunk(&self, pos: ChunkPosition) -> Result<BaseSnapshot, EvalError> {
        let min_y = self.runtime.min_y() as i32;
        let height = self.runtime.height() as usize;
        let max_y = min_y + height as i32 - 1;

        let base_x = pos.x * 16;
        let base_z = pos.z * 16;

        let cells_x = 16 / CELL_SIZE_XZ; // 4
        let cells_z = 16 / CELL_SIZE_XZ; // 4
        let cells_y = height / CELL_SIZE_Y; // e.g. 384 / 8 = 48

        // Corner grid size: (cells_x + 1) * (cells_y + 1) * (cells_z + 1)
        let stride_x = (cells_y + 1) * (cells_z + 1);
        let stride_y = cells_z + 1;
        let mut corners = vec![0.0f64; (cells_x + 1) * (cells_y + 1) * (cells_z + 1)];

        // Sample density at each cell corner using a chunk-scoped evaluator
        let mut evaluator = self.runtime.chunk_evaluator();
        for cx in 0..=cells_x {
            let wx = (base_x + (cx * CELL_SIZE_XZ) as i32) as f64;
            for cz in 0..=cells_z {
                let wz = (base_z + (cz * CELL_SIZE_XZ) as i32) as f64;
                for cy in 0..=cells_y {
                    let wy = (min_y + (cy * CELL_SIZE_Y) as i32) as f64;
                    let ctx = SampleContext { x: wx, y: wy, z: wz };
                    let density = evaluator.sample_root("final_density", &ctx)?;
                    let idx = cx * stride_x + cy * stride_y + cz;
                    corners[idx] = density;
                }
            }
        }
        drop(evaluator);

        // Initialize empty chunk
        let mut chunk = Chunk::empty(pos, self.dimension, min_y, max_y);
        let mut height_map = [min_y; 256];

        // Seed per-chunk pseudo-random stream for deepslate transition & bedrock floor
        let seed_mixer = ((pos.x as i64) << 32) ^ (pos.z as i64 & 0xFFFF_FFFF);
        let mut rng = LegacyRandom::new(self.runtime.seed() ^ seed_mixer);

        // Trilinear interpolation helper
        let trilerp = |c000: f64, c100: f64, c010: f64, c110: f64,
                       c001: f64, c101: f64, c011: f64, c111: f64,
                       tx: f64, ty: f64, tz: f64| -> f64 {
            let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
            lerp(
                lerp(lerp(c000, c100, tx), lerp(c010, c110, tx), ty),
                lerp(lerp(c001, c101, tx), lerp(c011, c111, tx), ty),
                tz,
            )
        };

        let canonical_air = BlockRuntimeId(sc_world::block_dictionary::air_runtime_id());
        let can_skip_air = self.palette.air == canonical_air;

        // Fill cells with trilinear interpolation
        for cx in 0..cells_x {
            for cz in 0..cells_z {
                for cy in 0..cells_y {
                    // Extract 8 corner densities
                    let c000 = corners[cx * stride_x + cy * stride_y + cz];
                    let c100 = corners[(cx + 1) * stride_x + cy * stride_y + cz];
                    let c010 = corners[cx * stride_x + (cy + 1) * stride_y + cz];
                    let c110 = corners[(cx + 1) * stride_x + (cy + 1) * stride_y + cz];

                    let c001 = corners[cx * stride_x + cy * stride_y + (cz + 1)];
                    let c101 = corners[(cx + 1) * stride_x + cy * stride_y + (cz + 1)];
                    let c011 = corners[cx * stride_x + (cy + 1) * stride_y + (cz + 1)];
                    let c111 = corners[(cx + 1) * stride_x + (cy + 1) * stride_y + (cz + 1)];

                    let cell_min_wy = min_y + (cy * CELL_SIZE_Y) as i32;
                    let max_c = c000.max(c100).max(c010).max(c110)
                        .max(c001).max(c101).max(c011).max(c111);

                    // Fast-skip: if all 8 corners are non-positive and cell bottom
                    // is above sea level, every voxel in this cell is air.
                    if can_skip_air && max_c <= 0.0 && cell_min_wy > self.sea_level {
                        continue;
                    }

                    for dx in 0..CELL_SIZE_XZ {
                        let tx = dx as f64 / CELL_SIZE_XZ as f64;
                        let lx = (cx * CELL_SIZE_XZ + dx) as u8;
                        for dz in 0..CELL_SIZE_XZ {
                            let tz = dz as f64 / CELL_SIZE_XZ as f64;
                            let lz = (cz * CELL_SIZE_XZ + dz) as u8;
                            let hm_idx = ((lx as usize) << 4) | (lz as usize);

                            for dy in (0..CELL_SIZE_Y).rev() {
                                let ty = dy as f64 / CELL_SIZE_Y as f64;
                                let wy = min_y + (cy * CELL_SIZE_Y + dy) as i32;

                                let density = trilerp(c000, c100, c010, c110, c001, c101, c011, c111, tx, ty, tz);

                                let block = if density > 0.0 {
                                    // Solid block: deepslate below Y=0, stochastic
                                    // transition in [0, 8]
                                    if wy < 0 {
                                        self.palette.deepslate
                                    } else if wy <= 8 && (rng.next_int_bounded(9) as i32) >= wy {
                                        self.palette.deepslate
                                    } else {
                                        self.palette.stone
                                    }
                                } else if wy <= self.sea_level {
                                    self.palette.water
                                } else {
                                    self.palette.air
                                };

                                if !can_skip_air || block != self.palette.air {
                                    chunk.set_block_at(0, lx, wy, lz, block);
                                    if density > 0.0 && wy > height_map[hm_idx] {
                                        height_map[hm_idx] = wy;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Place bedrock floor at min_y and random rough bedrock up to min_y + 4
        for lx in 0u8..16 {
            for lz in 0u8..16 {
                // Bottom-most block is solid unbreakable bedrock
                chunk.set_block_at(0, lx, min_y, lz, self.palette.bedrock);

                let extra_bedrock = rng.next_int_bounded(5) as i32;
                for b_offset in 1..extra_bedrock {
                    let by = min_y + b_offset;
                    chunk.set_block_at(0, lx, by, lz, self.palette.bedrock);
                }
            }
        }

        Ok(BaseSnapshot::new(
            self.dimension,
            pos,
            self.world_instance_id,
            self.generator_fingerprint,
            min_y,
            max_y,
            Arc::new(chunk),
            height_map,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use crate::data::{Axis, Node};
    use crate::link::LinkedNoiseSettings;
    use sc_world::generation::error::GenerationContractError;
    use sc_world::generation::view::GenerationView;

    const STONE: BlockRuntimeId = BlockRuntimeId(1);
    const DEEPSLATE: BlockRuntimeId = BlockRuntimeId(2);
    const BEDROCK: BlockRuntimeId = BlockRuntimeId(3);
    const WATER: BlockRuntimeId = BlockRuntimeId(4);
    const AIR: BlockRuntimeId = BlockRuntimeId(0);

    const TEST_PALETTE: BaseBlockPalette = BaseBlockPalette {
        stone: STONE,
        deepslate: DEEPSLATE,
        bedrock: BEDROCK,
        water: WATER,
        air: AIR,
    };

    fn make_test_runtime() -> Arc<WorldgenRuntime> {
        // final_density = 64.0 - Y
        let mut router = BTreeMap::new();
        router.insert(
            "final_density".to_string(),
            Node::Add(
                Box::new(Node::Constant(64.0)),
                Box::new(Node::Mul(
                    Box::new(Node::Coord(Axis::Y)),
                    Box::new(Node::Constant(-1.0)),
                )),
            ),
        );
        let settings = LinkedNoiseSettings {
            legacy_random_source: false,
            min_y: -64,
            height: 384,
            router,
            noises: BTreeMap::new(),
            frozen: BTreeMap::new(),
        };
        Arc::new(WorldgenRuntime::new(12345, &settings))
    }

    #[test]
    fn base_terrain_generator_produces_snapshot_with_accurate_height_and_materials() {
        let runtime = make_test_runtime();
        let generator = BaseTerrainGenerator::new(
            Arc::clone(&runtime),
            0,
            0x1000,
            0x2000,
            TEST_PALETTE,
        );

        let snapshot = generator.generate_base_chunk(ChunkPosition::new(0, 0)).unwrap();

        assert_eq!(snapshot.dimension, 0);
        assert_eq!(snapshot.chunk_position, ChunkPosition::new(0, 0));
        assert_eq!(snapshot.min_y, -64);
        assert_eq!(snapshot.max_y, 319);
        assert_eq!(runtime.outstanding_scratch(), 0, "scratch pool must be fully returned");

        // Bedrock floor at Y=-64
        assert_eq!(snapshot.block_at(0, -64, 0), BEDROCK);
        assert_eq!(snapshot.block_at(15, -64, 15), BEDROCK);

        // Deepslate below Y=0
        assert_eq!(snapshot.block_at(8, -32, 8), DEEPSLATE);

        // Stone between Y=9 and Y=63
        assert_eq!(snapshot.block_at(8, 32, 8), STONE);
        assert_eq!(snapshot.block_at(8, 63, 8), STONE);

        // Air at and above Y=64 (where density 64 - Y <= 0)
        assert_eq!(snapshot.block_at(8, 64, 8), AIR);
        assert_eq!(snapshot.block_at(8, 100, 8), AIR);

        // Heightmap is 63 everywhere
        for lx in 0u8..16 {
            for lz in 0u8..16 {
                assert_eq!(snapshot.surface_height_at(lx, lz), 63);
            }
        }
    }

    #[test]
    fn base_terrain_generator_ocean_column_preserves_water_and_accurate_seabed_height() {
        // final_density = 30.0 - Y (seabed at Y=30, ocean water from 31 to 63)
        let mut router = BTreeMap::new();
        router.insert(
            "final_density".to_string(),
            Node::Add(
                Box::new(Node::Constant(30.0)),
                Box::new(Node::Mul(
                    Box::new(Node::Coord(Axis::Y)),
                    Box::new(Node::Constant(-1.0)),
                )),
            ),
        );
        let settings = LinkedNoiseSettings {
            legacy_random_source: false,
            min_y: -64,
            height: 384,
            router,
            noises: BTreeMap::new(),
            frozen: BTreeMap::new(),
        };
        let runtime = Arc::new(WorldgenRuntime::new(12345, &settings));
        let generator = BaseTerrainGenerator::new(
            Arc::clone(&runtime),
            0,
            0x1000,
            0x2000,
            TEST_PALETTE,
        );

        let snapshot = generator.generate_base_chunk(ChunkPosition::new(0, 0)).unwrap();

        // 1. Solid seabed at Y=29 (where 30.0 - Y > 0.0)
        assert_eq!(snapshot.block_at(8, 29, 8), STONE);

        // 2. Water from Y=30 to Y=63 (sea level)
        assert_eq!(snapshot.block_at(8, 30, 8), WATER);
        assert_eq!(snapshot.block_at(8, 63, 8), WATER);

        // 3. Air above sea level (Y >= 64)
        assert_eq!(snapshot.block_at(8, 64, 8), AIR);

        // 4. Heightmap MUST be solid seabed height (29), NOT water surface (63)
        for lx in 0u8..16 {
            for lz in 0u8..16 {
                assert_eq!(
                    snapshot.surface_height_at(lx, lz),
                    29,
                    "heightmap for ocean column must be seabed height, not sea level"
                );
            }
        }
    }

    #[test]
    fn base_terrain_generator_digest_is_deterministic_across_runs() {
        let runtime = make_test_runtime();
        let generator = BaseTerrainGenerator::new(
            Arc::clone(&runtime),
            0,
            0x1000,
            0x2000,
            TEST_PALETTE,
        );

        let snap1 = generator.generate_base_chunk(ChunkPosition::new(3, -5)).unwrap();
        let snap2 = generator.generate_base_chunk(ChunkPosition::new(3, -5)).unwrap();
        assert_eq!(snap1.digest(), snap2.digest());

        let snap3 = generator.generate_base_chunk(ChunkPosition::new(3, -4)).unwrap();
        assert_ne!(snap1.digest(), snap3.digest(), "different coordinates have different digests");
    }

    #[test]
    fn base_terrain_generator_feeds_generation_view_correctly() {
        let runtime = make_test_runtime();
        let generator = BaseTerrainGenerator::new(
            Arc::clone(&runtime),
            0,
            0x1000,
            0x2000,
            TEST_PALETTE,
        );

        let snap00 = Arc::new(generator.generate_base_chunk(ChunkPosition::new(0, 0)).unwrap());
        let snap10 = Arc::new(generator.generate_base_chunk(ChunkPosition::new(1, 0)).unwrap());

        let view = GenerationView::with_snapshots(0, 0x1000, 0x2000, -64, 319, AIR, vec![snap00, snap10]).unwrap();

        // Reading across border into (1, 0)
        assert_eq!(view.block_at(16, 50, 0).unwrap(), STONE);
        assert_eq!(view.surface_height_at(16, 0).unwrap(), 63);

        // Reading into ungenerated chunk (0, 1)
        let err = view.block_at(8, 50, 16).unwrap_err();
        assert_eq!(
            err,
            GenerationContractError::MissingDependency {
                dimension: 0,
                chunk_x: 0,
                chunk_z: 1,
                stage: "base"
            }
        );
    }
}

