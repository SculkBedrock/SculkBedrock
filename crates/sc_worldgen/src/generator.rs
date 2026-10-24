//! 内置世界生成器实现：提供 [`BuiltinWorldGenerator`]，它基于编译后的密度
//! 计划、噪声路由与三层特性流水线实现 [`sc_world::storage::WorldGenerator`]。
//!
//! Built-in world generator implementation: provides [`BuiltinWorldGenerator`],
//! which implements [`sc_world::storage::WorldGenerator`] using the compiled
//! density plan, noise router, and three-layer feature pipeline.

use std::sync::Arc;

use sc_world::block_dictionary::{air_runtime_id, resolve_or_register_default};
use sc_world::chunk::{BlockRuntimeId, Chunk, ChunkPosition, PalettedBiomeStorage};
use sc_world::generation::descriptor::GeneratorDescriptor;
use sc_world::generation::identity::{FeatureType, OriginCell};
use sc_world::generation::view::{BaseSnapshot, GenerationView};
use sc_world::storage::{
    BlockSpilloverWrite, ChunkGenerationRequest, ChunkKey, GeneratedChunk, WorldGenerator,
    WorldStorageError,
};

use crate::compiled::CompiledWorldgen;
use crate::feature::{
    project_feature_plan, CandidateSampler, DesertWellSolver, SwampHutSolver, TreeSolver,
};
use crate::runtime::WorldgenRuntime;
use crate::terrain::base::{BaseBlockPalette, BaseTerrainGenerator, DEFAULT_SEA_LEVEL};

/// 用于地表修饰与装饰的方块运行时 ID 调色板。
///
/// Palette of block runtime IDs used for surface dressing and decoration.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceBlockPalette {
    pub stone: BlockRuntimeId,
    pub deepslate: BlockRuntimeId,
    pub bedrock: BlockRuntimeId,
    pub water: BlockRuntimeId,
    pub air: BlockRuntimeId,
    pub grass_block: BlockRuntimeId,
    pub dirt: BlockRuntimeId,
    pub sand: BlockRuntimeId,
}

impl Default for SurfaceBlockPalette {
    fn default() -> Self {
        Self {
            stone: BlockRuntimeId(resolve_or_register_default("minecraft:stone")),
            deepslate: BlockRuntimeId(resolve_or_register_default("minecraft:deepslate")),
            bedrock: BlockRuntimeId(resolve_or_register_default("minecraft:bedrock")),
            water: BlockRuntimeId(resolve_or_register_default("minecraft:water")),
            air: BlockRuntimeId(air_runtime_id()),
            grass_block: BlockRuntimeId(resolve_or_register_default("minecraft:grass_block")),
            dirt: BlockRuntimeId(resolve_or_register_default("minecraft:dirt")),
            sand: BlockRuntimeId(resolve_or_register_default("minecraft:sand")),
        }
    }
}

/// 实现 [`WorldGenerator`] 的内置三层世界生成器。
///
/// The built-in three-layer world generator implementing [`WorldGenerator`].
pub struct BuiltinWorldGenerator {
    runtime: Arc<WorldgenRuntime>,
    base_generator: BaseTerrainGenerator,
    tree_solver: TreeSolver,
    #[allow(dead_code)]
    desert_well_solver: DesertWellSolver,
    #[allow(dead_code)]
    swamp_hut_solver: SwampHutSolver,
    descriptor: GeneratorDescriptor,
    palette: SurfaceBlockPalette,
    sea_level: i32,
}

impl BuiltinWorldGenerator {
    /// 创建绑定到给定编译资产与种子的新内置生成器。
    ///
    /// Creates a new built-in generator bound to the given compiled assets and seed.
    pub fn new(compiled: &CompiledWorldgen, seed: i64) -> Self {
        let runtime = Arc::new(compiled.instantiate_runtime(seed));
        let descriptor = compiled.descriptor(seed, 0);
        let base_palette = BaseBlockPalette {
            stone: BlockRuntimeId(resolve_or_register_default("minecraft:stone")),
            deepslate: BlockRuntimeId(resolve_or_register_default("minecraft:deepslate")),
            bedrock: BlockRuntimeId(resolve_or_register_default("minecraft:bedrock")),
            water: BlockRuntimeId(resolve_or_register_default("minecraft:water")),
            air: BlockRuntimeId(air_runtime_id()),
        };
        let base_generator = BaseTerrainGenerator::new(
            Arc::clone(&runtime),
            0,
            1,
            descriptor.fingerprint(),
            base_palette,
        );
        Self {
            runtime,
            base_generator,
            tree_solver: TreeSolver::default(),
            desert_well_solver: DesertWellSolver::default(),
            swamp_hut_solver: SwampHutSolver::default(),
            descriptor,
            palette: SurfaceBlockPalette::default(),
            sea_level: DEFAULT_SEA_LEVEL,
        }
    }

    /// 绑定的生成器描述符。
    ///
    /// Bound generator descriptor.
    pub fn descriptor(&self) -> &GeneratorDescriptor {
        &self.descriptor
    }

    /// 绑定的运行时。
    ///
    /// Bound runtime.
    pub fn runtime(&self) -> &Arc<WorldgenRuntime> {
        &self.runtime
    }

    /// 粗扫附近原点、寻找干燥内陆出生点。
    ///
    /// Coarse scan to locate a dry inland spawn position near the origin.
    pub fn find_land_spawn(&self) -> Option<(i32, i32, i32)> {
        // Search spiral coordinates around origin at step of 16 blocks
        for radius in 0i32..8i32 {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx.abs() != radius && dz.abs() != radius {
                        continue;
                    }
                    let cx = dx;
                    let cz = dz;
                    if let Ok(snap) = self.base_generator.generate_base_chunk(ChunkPosition::new(cx, cz)) {
                        let surface_y = snap.surface_height_at(8, 8);
                        if surface_y >= self.sea_level + 2 && surface_y <= 120 {
                            let world_x = cx * 16 + 8;
                            let world_z = cz * 16 + 8;
                            return Some((world_x, surface_y + 1, world_z));
                        }
                    }
                }
            }
        }
        None
    }
}

impl WorldGenerator for BuiltinWorldGenerator {
    fn generate_chunk(
        &self,
        request: ChunkGenerationRequest,
    ) -> Result<Option<Chunk>, WorldStorageError> {
        self.generate_chunk_with_spillover(request)
            .map(|opt| opt.map(|gen| gen.chunk))
    }

    fn generate_chunk_with_spillover(
        &self,
        request: ChunkGenerationRequest,
    ) -> Result<Option<GeneratedChunk>, WorldStorageError> {
        self.generate_chunk_with_spillover_bounded(
            request,
            sc_world::storage::MAX_GENERATED_SPILLOVER_WRITES,
        )
    }

    fn generate_chunk_with_spillover_bounded(
        &self,
        request: ChunkGenerationRequest,
        max_spillover_writes: usize,
    ) -> Result<Option<GeneratedChunk>, WorldStorageError> {
        let pos = request.key.position;
        let base_snapshot = self
            .base_generator
            .generate_base_chunk(pos)
            .map_err(|err| WorldStorageError::backend(format!("density eval failed: {err}")))?;

        let mut chunk = (*base_snapshot.chunk).clone();
        let mut height_map = base_snapshot.height_map;

        // 1. Fill 3D biomes (Plains: 1) for all subchunks
        let subchunk_count = chunk.subchunks.len();
        if chunk.biomes.is_empty() {
            chunk.biomes = (0..subchunk_count)
                .map(|_| PalettedBiomeStorage::Single(1))
                .collect();
        }

        // 2. Surface dressing: grass/dirt on land, sand in shallow water/beaches
        for lx in 0u8..16 {
            for lz in 0u8..16 {
                let hm_idx = ((lx as usize) << 4) | (lz as usize);
                let top_y = height_map[hm_idx];
                if top_y > request.min_y {
                    let top_block = chunk.block_at(lx, top_y, lz);
                    let is_solid_surface = top_block == Some(self.palette.stone)
                        || top_block == Some(self.palette.deepslate);

                    if is_solid_surface {
                        if top_y >= self.sea_level {
                            // Land surface: top block is grass block, below is dirt
                            chunk.set_block_at(0, lx, top_y, lz, self.palette.grass_block);
                            let dirt_depth = 3;
                            let bottom_dirt = (top_y - dirt_depth).max(request.min_y);
                            for y in bottom_dirt..top_y {
                                if chunk.block_at(lx, y, lz) == Some(self.palette.stone) {
                                    chunk.set_block_at(0, lx, y, lz, self.palette.dirt);
                                }
                            }
                        } else if top_y >= self.sea_level - 2 {
                            // Shallow water floor / beach: sand
                            for y in (top_y - 2).max(request.min_y)..=top_y {
                                chunk.set_block_at(0, lx, y, lz, self.palette.sand);
                            }
                        }
                    }
                }
            }
        }

        // 3. Feature decoration (trees)
        let dressed_snapshot = Arc::new(BaseSnapshot::new(
            request.key.dimension,
            pos,
            base_snapshot.world_instance_id,
            base_snapshot.generator_fingerprint,
            request.min_y,
            request.max_y,
            Arc::new(chunk.clone()),
            height_map,
        ));

        let view = GenerationView::with_snapshots(
            request.key.dimension,
            base_snapshot.world_instance_id,
            base_snapshot.generator_fingerprint,
            request.min_y,
            request.max_y,
            self.palette.air,
            vec![dressed_snapshot],
        )
        .ok();

        let mut spillover = Vec::new();
        if let Some(view) = &view {
            let origin = OriginCell::new(pos.x, pos.z);
            let candidates = CandidateSampler::sample_candidates_for_cell(
                self.descriptor.fingerprint(),
                request.key.dimension,
                origin,
                FeatureType::Tree,
                8,
            );

            for cand in candidates {
                // Focus on candidates whose crown remains mostly within this chunk
                let local_x = cand.anchor_x.rem_euclid(16);
                let local_z = cand.anchor_z.rem_euclid(16);
                if local_x < 2 || local_x > 13 || local_z < 2 || local_z > 13 {
                    continue;
                }

                if let Ok(plan) = self.tree_solver.solve(&cand, view) {
                    if plan.is_accepted() {
                        let (fragments, _manifest) = project_feature_plan(&plan);
                        for frag in fragments {
                            if frag.fragment_id.target_x == pos.x
                                && frag.fragment_id.target_z == pos.z
                            {
                                for w in frag.writes {
                                    if w.local_x < 16
                                        && w.local_z < 16
                                        && w.y >= request.min_y
                                        && w.y <= request.max_y
                                    {
                                        chunk.set_block_at(
                                            w.layer as usize,
                                            w.local_x,
                                            w.y,
                                            w.local_z,
                                            BlockRuntimeId(w.runtime_id),
                                        );
                                        if w.layer == 0 {
                                            let idx = ((w.local_x as usize) << 4)
                                                | (w.local_z as usize);
                                            if w.y > height_map[idx] {
                                                height_map[idx] = w.y;
                                            }
                                        }
                                    }
                                }
                            } else {
                                for w in frag.writes {
                                    if spillover.len() < max_spillover_writes {
                                        spillover.push(BlockSpilloverWrite {
                                            key: ChunkKey::new(
                                                request.key.dimension,
                                                ChunkPosition::new(
                                                    frag.fragment_id.target_x,
                                                    frag.fragment_id.target_z,
                                                ),
                                            ),
                                            x: frag.fragment_id.target_x * 16 + w.local_x as i32,
                                            y: w.y,
                                            z: frag.fragment_id.target_z * 16 + w.local_z as i32,
                                            layer: w.layer as usize,
                                            block: BlockRuntimeId(w.runtime_id),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(Some(GeneratedChunk { chunk, spillover }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use crate::data::{Axis, Node};
    use crate::link::LinkedNoiseSettings;

    #[test]
    fn generator_surface_dressing_preserves_ocean_water_and_does_not_place_grass_on_sea() {
        // Build an ocean runtime with seabed at Y=30
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
        let descriptor = GeneratorDescriptor::builtin_overworld(12345, 0x1000, 0x2000);
        let palette = SurfaceBlockPalette::default();
        let base_palette = BaseBlockPalette {
            stone: palette.stone,
            deepslate: palette.deepslate,
            bedrock: palette.bedrock,
            water: palette.water,
            air: palette.air,
        };
        let base_generator = BaseTerrainGenerator::new(
            Arc::clone(&runtime),
            0,
            1,
            descriptor.fingerprint(),
            base_palette,
        );
        let generator = BuiltinWorldGenerator {
            runtime,
            base_generator,
            tree_solver: TreeSolver::default(),
            desert_well_solver: DesertWellSolver::default(),
            swamp_hut_solver: SwampHutSolver::default(),
            descriptor,
            palette,
            sea_level: DEFAULT_SEA_LEVEL,
        };

        let request = ChunkGenerationRequest {
            key: ChunkKey::new(0, ChunkPosition::new(0, 0)),
            min_y: -64,
            max_y: 319,
        };

        let result = generator.generate_chunk(request).expect("generation must succeed");
        let chunk = result.expect("chunk must be produced");

        // Sea level is 63: the sea surface must be water, NOT grass block!
        for lx in 0u8..16 {
            for lz in 0u8..16 {
                let surface_block = chunk.block_at(lx, 63, lz);
                assert_eq!(
                    surface_block,
                    Some(palette.water),
                    "ocean surface at Y=63 must be water, but found {:?}",
                    surface_block
                );

                let air_block = chunk.block_at(lx, 64, lz);
                assert_eq!(
                    air_block,
                    Some(palette.air),
                    "block above ocean surface at Y=64 must be air, but found {:?}",
                    air_block
                );

                // Ensure NO grass blocks anywhere in the ocean column
                for y in 31..=63 {
                    assert_ne!(
                        chunk.block_at(lx, y, lz),
                        Some(palette.grass_block),
                        "grass block must not appear on or inside water at Y={y}"
                    );
                }
            }
        }
    }
}

