//! 针对 GenerationView 的结构求解器（沙漠水井、沼泽小屋）。
//!
//! Structure solvers (Desert Well, Swamp Hut) against GenerationView.

use sc_world::block_dictionary::{air_runtime_id, resolve_or_register_default};
use sc_world::generation::error::GenerationContractError;
use sc_world::generation::view::GenerationView;

use crate::feature::candidate::FeatureCandidate;
use crate::feature::plan::{FeaturePlan, FeatureRejectReason, WorldBlockWrite};

/// 沙漠水井生成使用的方块运行时 ID。
///
/// Block runtime IDs for desert well generation.
#[derive(Debug, Clone, Copy)]
pub struct DesertWellPalette {
    pub air: u32,
    pub sand: u32,
    pub red_sand: u32,
    pub sandstone: u32,
    pub water: u32,
}

impl Default for DesertWellPalette {
    fn default() -> Self {
        Self {
            air: air_runtime_id(),
            sand: resolve_or_register_default("minecraft:sand"),
            red_sand: resolve_or_register_default("minecraft:red_sand"),
            sandstone: resolve_or_register_default("minecraft:sandstone"),
            water: resolve_or_register_default("minecraft:water"),
        }
    }
}

/// 针对只读 GenerationView 求解沙漠水井候选。
///
/// Solves a Desert Well candidate against a read-only GenerationView.
pub struct DesertWellSolver {
    pub palette: DesertWellPalette,
}

impl Default for DesertWellSolver {
    fn default() -> Self {
        Self {
            palette: DesertWellPalette::default(),
        }
    }
}

impl DesertWellSolver {
    pub fn new(palette: DesertWellPalette) -> Self {
        Self { palette }
    }

    pub fn solve(
        &self,
        candidate: &FeatureCandidate,
        view: &GenerationView,
    ) -> Result<FeaturePlan, GenerationContractError> {
        let ax = candidate.anchor_x;
        let az = candidate.anchor_z;

        let surface_y = view.surface_height_at(ax, az)?;

        // Foundation check: ground at anchor must be sand
        let ground = view.block_at(ax, surface_y, az)?;
        if ground.0 != self.palette.sand && ground.0 != self.palette.red_sand {
            return Ok(FeaturePlan::rejected(
                candidate.feature_id,
                FeatureRejectReason::InvalidFoundation("ground block is not sand".into()),
            ));
        }

        // 5x5 footprint clearance check (dy: 1..=4)
        for dx in -2i32..=2i32 {
            for dz in -2i32..=2i32 {
                for dy in 1i32..=4i32 {
                    let wx = ax + dx;
                    let wy = surface_y + dy;
                    let wz = az + dz;
                    let b = view.block_at(wx, wy, wz)?;
                    if b.0 != self.palette.air {
                        return Ok(FeaturePlan::rejected(
                            candidate.feature_id,
                            FeatureRejectReason::Obstructed(format!("obstacle at ({wx},{wy},{wz})")),
                        ));
                    }
                }
            }
        }

        let mut writes = Vec::new();

        // 5x5 rim and water floor at surface_y
        for dx in -2..=2 {
            for dz in -2..=2 {
                let wx = ax + dx;
                let wz = az + dz;
                let is_rim = dx.abs() == 2 || dz.abs() == 2;
                let runtime_id = if is_rim {
                    self.palette.sandstone
                } else if dx == 0 && dz == 0 {
                    self.palette.water
                } else {
                    self.palette.sandstone
                };
                writes.push(WorldBlockWrite {
                    world_x: wx,
                    world_y: surface_y,
                    world_z: wz,
                    runtime_id,
                    layer: 0,
                });
            }
        }

        // 4 corner pillars (dy: 1..=3)
        let pillar_offsets = [(-1, -1), (-1, 1), (1, -1), (1, 1)];
        for (px, pz) in pillar_offsets {
            for dy in 1..=3 {
                writes.push(WorldBlockWrite {
                    world_x: ax + px,
                    world_y: surface_y + dy,
                    world_z: az + pz,
                    runtime_id: self.palette.sandstone,
                    layer: 0,
                });
            }
        }

        // Roof at dy: 4 over the 3x3 center
        for dx in -1..=1 {
            for dz in -1..=1 {
                writes.push(WorldBlockWrite {
                    world_x: ax + dx,
                    world_y: surface_y + 4,
                    world_z: az + dz,
                    runtime_id: self.palette.sandstone,
                    layer: 0,
                });
            }
        }

        Ok(FeaturePlan::accepted(candidate.feature_id, writes))
    }
}

/// 沼泽小屋生成使用的方块运行时 ID。
///
/// Block runtime IDs for swamp hut generation.
#[derive(Debug, Clone, Copy)]
pub struct SwampHutPalette {
    pub air: u32,
    pub spruce_planks: u32,
    pub oak_log: u32,
}

impl Default for SwampHutPalette {
    fn default() -> Self {
        Self {
            air: air_runtime_id(),
            spruce_planks: resolve_or_register_default("minecraft:spruce_planks"),
            oak_log: resolve_or_register_default("minecraft:oak_log"),
        }
    }
}

/// 针对只读 GenerationView 求解沼泽小屋候选。
///
/// Solves a Swamp Hut candidate against a read-only GenerationView.
pub struct SwampHutSolver {
    pub palette: SwampHutPalette,
}

impl Default for SwampHutSolver {
    fn default() -> Self {
        Self {
            palette: SwampHutPalette::default(),
        }
    }
}

impl SwampHutSolver {
    pub fn new(palette: SwampHutPalette) -> Self {
        Self { palette }
    }

    pub fn solve(
        &self,
        candidate: &FeatureCandidate,
        view: &GenerationView,
    ) -> Result<FeaturePlan, GenerationContractError> {
        let ax = candidate.anchor_x;
        let az = candidate.anchor_z;

        let surface_y = view.surface_height_at(ax, az)?;

        // Footprint: 7x7 hut (dx: -3..=3, dz: -3..=3), clearance dy: 2..=5
        for dx in -3i32..=3i32 {
            for dz in -3i32..=3i32 {
                for dy in 2i32..=5i32 {
                    let wx = ax + dx;
                    let wy = surface_y + dy;
                    let wz = az + dz;
                    let b = view.block_at(wx, wy, wz)?;
                    if b.0 != self.palette.air {
                        return Ok(FeaturePlan::rejected(
                            candidate.feature_id,
                            FeatureRejectReason::Obstructed(format!("obstacle at ({wx},{wy},{wz})")),
                        ));
                    }
                }
            }
        }

        let mut writes = Vec::new();

        // 4 stilts (logs) from surface_y down into ground, and up to dy=1
        let stilt_offsets = [(-2i32, -2i32), (-2i32, 2i32), (2i32, -2i32), (2i32, 2i32)];
        for (sx, sz) in stilt_offsets {
            for dy in 0i32..=1i32 {
                writes.push(WorldBlockWrite {
                    world_x: ax + sx,
                    world_y: surface_y + dy,
                    world_z: az + sz,
                    runtime_id: self.palette.oak_log,
                    layer: 0,
                });
            }
        }

        // Floor at dy: 2 (7x7)
        for dx in -3i32..=3i32 {
            for dz in -3i32..=3i32 {
                writes.push(WorldBlockWrite {
                    world_x: ax + dx,
                    world_y: surface_y + 2,
                    world_z: az + dz,
                    runtime_id: self.palette.spruce_planks,
                    layer: 0,
                });
            }
        }

        // Walls at dy: 3..=4 (rim of 7x7)
        for dx in -3i32..=3i32 {
            for dz in -3i32..=3i32 {
                if dx.abs() == 3 || dz.abs() == 3 {
                    // Door opening at (0, -3)
                    if dx == 0 && dz == -3 {
                        continue;
                    }
                    for dy in 3i32..=4i32 {
                        writes.push(WorldBlockWrite {
                            world_x: ax + dx,
                            world_y: surface_y + dy,
                            world_z: az + dz,
                            runtime_id: self.palette.spruce_planks,
                            layer: 0,
                        });
                    }
                }
            }
        }

        // Roof at dy: 5 (7x7)
        for dx in -3i32..=3i32 {
            for dz in -3i32..=3i32 {
                writes.push(WorldBlockWrite {
                    world_x: ax + dx,
                    world_y: surface_y + 5,
                    world_z: az + dz,
                    runtime_id: self.palette.spruce_planks,
                    layer: 0,
                });
            }
        }

        Ok(FeaturePlan::accepted(candidate.feature_id, writes))
    }
}
