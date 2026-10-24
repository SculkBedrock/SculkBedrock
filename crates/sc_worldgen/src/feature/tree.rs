//! 针对 GenerationView 的树木特性规划与求解。
//!
//! Tree feature planning and solving against GenerationView.

use sc_world::block_dictionary::{air_runtime_id, resolve_or_register_default};
use sc_world::generation::error::GenerationContractError;
use sc_world::generation::view::GenerationView;

use crate::feature::candidate::FeatureCandidate;
use crate::feature::plan::{FeaturePlan, FeatureRejectReason, WorldBlockWrite};

/// 树木生成使用的方块运行时 ID。
///
/// Block runtime IDs for tree generation.
#[derive(Debug, Clone, Copy)]
pub struct TreeBlockPalette {
    pub air: u32,
    pub dirt: u32,
    pub grass_block: u32,
    pub podzol: u32,
    pub coarse_dirt: u32,
    pub oak_log: u32,
    pub oak_leaves: u32,
}

impl Default for TreeBlockPalette {
    fn default() -> Self {
        Self {
            air: air_runtime_id(),
            dirt: resolve_or_register_default("minecraft:dirt"),
            grass_block: resolve_or_register_default("minecraft:grass_block"),
            podzol: resolve_or_register_default("minecraft:podzol"),
            coarse_dirt: resolve_or_register_default("minecraft:coarse_dirt"),
            oak_log: resolve_or_register_default("minecraft:oak_log"),
            oak_leaves: resolve_or_register_default("minecraft:oak_leaves"),
        }
    }
}

/// 针对只读地形 GenerationView 求解树木候选。
///
/// Solves a tree candidate against a read-only terrain GenerationView.
///
/// 若所需的相邻区块不在视图中，会向上传播
/// [`GenerationContractError::MissingDependency`]——缺失的相邻区块绝不
/// 被伪装成空气！
///
/// Propagates [`GenerationContractError::MissingDependency`] if required neighbor
/// chunks are not present in the view — missing neighbors are never disguised as air!
pub struct TreeSolver {
    pub palette: TreeBlockPalette,
}

impl Default for TreeSolver {
    fn default() -> Self {
        Self {
            palette: TreeBlockPalette::default(),
        }
    }
}

impl TreeSolver {
    pub fn new(palette: TreeBlockPalette) -> Self {
        Self { palette }
    }

    /// 求解候选树木，校验支撑地基与树冠清空情况。
    ///
    /// Solves the candidate tree, verifying support foundation and crown clearance.
    pub fn solve(
        &self,
        candidate: &FeatureCandidate,
        view: &GenerationView,
    ) -> Result<FeaturePlan, GenerationContractError> {
        let ax = candidate.anchor_x;
        let az = candidate.anchor_z;

        // 1. Get surface height at anchor position
        let surface_y = view.surface_height_at(ax, az)?;

        // 2. Foundation check: ground block must be dirt or grass
        let ground_block = view.block_at(ax, surface_y, az)?;
        let is_valid_ground = ground_block.0 == self.palette.dirt
            || ground_block.0 == self.palette.grass_block
            || ground_block.0 == self.palette.podzol
            || ground_block.0 == self.palette.coarse_dirt;

        if !is_valid_ground {
            return Ok(FeaturePlan::rejected(
                candidate.feature_id,
                FeatureRejectReason::InvalidFoundation(format!(
                    "block id {} at surface Y={} is not dirt/grass",
                    ground_block.0, surface_y
                )),
            ));
        }

        // 3. Tree dimensions: height 4 to 6 blocks
        let trunk_height = 4 + (candidate.variant % 3) as i32;
        let top_y = surface_y + trunk_height;

        if top_y + 1 > 319 {
            return Ok(FeaturePlan::rejected(
                candidate.feature_id,
                FeatureRejectReason::OutOfBounds { y: top_y + 1 },
            ));
        }

        // 4. Footprint clearance check:
        // Examine every voxel that trunk or leaves would occupy.
        // If any voxel has a solid non-air block, reject as Obstructed.
        // If a voxel crosses into an unprovided neighbor chunk, block_at will
        // return MissingDependency and propagate up.
        let mut writes = Vec::with_capacity((trunk_height + 40) as usize);

        // Clearance & leaf positions collection
        for dy in 1..=(trunk_height + 1) {
            let y = surface_y + dy;

            // Crown shape:
            // dy == trunk_height - 1 or trunk_height: radius 2 (5x5 without corners)
            // dy == trunk_height: radius 1 (3x3)
            // dy == trunk_height + 1: cross (radius 1 cross)
            let radius = if dy >= trunk_height - 2 && dy <= trunk_height - 1 {
                2i32
            } else if dy == trunk_height {
                1i32
            } else if dy == trunk_height + 1 {
                1i32
            } else {
                0i32
            };

            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let wx = ax + dx;
                    let wz = az + dz;

                    // Corner omission for radius 2
                    if radius == 2 && dx.abs() == 2 && dz.abs() == 2 {
                        continue;
                    }
                    // Cross shape for top layer (dy == trunk_height + 1)
                    if dy == trunk_height + 1 && dx.abs() == 1 && dz.abs() == 1 {
                        continue;
                    }

                    let block = view.block_at(wx, y, wz)?;
                    if block.0 != self.palette.air && block.0 != self.palette.oak_leaves {
                        return Ok(FeaturePlan::rejected(
                            candidate.feature_id,
                            FeatureRejectReason::Obstructed(format!(
                                "obstacle block id {} at ({wx},{y},{wz})",
                                block.0
                            )),
                        ));
                    }

                    // Collect write
                    let is_trunk = dx == 0 && dz == 0 && dy <= trunk_height;
                    let runtime_id = if is_trunk {
                        self.palette.oak_log
                    } else {
                        self.palette.oak_leaves
                    };

                    writes.push(WorldBlockWrite {
                        world_x: wx,
                        world_y: y,
                        world_z: wz,
                        runtime_id,
                        layer: 0,
                    });
                }
            }
        }

        Ok(FeaturePlan::accepted(candidate.feature_id, writes))
    }
}
