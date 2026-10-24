//! 特征覆盖范围上界与源闭包查询。
//!
//! 每个启用的特征类型都为其写入覆盖、读取覆盖与冲突检查视野注册保守的
//! 上界。注册表把目标区块转换成可能与之相交的全部源单元格；闭包超出配置
//! 预算时显式失败，不会被截断。
//!
//! Feature footprint bounds and the source-closure query.
//!
//! Every enabled feature type registers conservative upper bounds for its
//! write footprint, read footprint and conflict-check horizon. The registry
//! turns a target chunk into every origin cell whose features can intersect it;
//! closures beyond the configured budget fail loudly and are never truncated.

use std::collections::BTreeSet;

use crate::generation::error::GenerationContractError;
use crate::generation::identity::{FeatureType, OriginCell};
use serde::{Deserialize, Serialize};

/// 单个特征类型的保守覆盖范围上界。所有区块半径都是以区块为单位的
/// 切比雪夫距离；高度边界为世界 Y。
///
/// Conservative footprint bounds for one feature type. All chunk radii are
/// Chebyshev distances in chunk units; height bounds are world Y.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureExtent {
    pub feature_type: FeatureType,
    /// 水平写入覆盖：锚定在该切比雪夫区块半径内的单元格的特征可以向目标
    /// 区块写入。
    ///
    /// Horizontal write footprint: features anchored in a cell within this
    /// Chebyshev chunk radius may write into the target chunk.
    pub write_radius_chunks: u8,
    /// 水平读取覆盖：读取可能需要锚定单元格之外该半径范围内的地形数据。
    ///
    /// Horizontal read footprint: reads may need terrain data within this
    /// radius beyond the anchor cell itself.
    pub read_radius_chunks: u8,
    /// 冲突视野：候选验证可能需要咨询锚定在该半径内的其他候选
    /// （冲突光环）。
    ///
    /// Conflict horizon: candidate validation may consult other candidates
    /// anchored within this radius (conflict halo).
    pub conflict_radius_chunks: u8,
    /// 锚定表面高度上下的最大垂直跨度。
    ///
    /// Maximum vertical span above/below the anchor surface height.
    pub vertical_span: i32,
    /// 每个源单元格被接受的候选数上界。
    ///
    /// Upper bound on accepted candidates per origin cell.
    pub max_candidates_per_cell: u32,
    /// 每个被接受候选的方块写入总数上界。
    ///
    /// Upper bound on total block writes per accepted candidate.
    pub max_writes_per_candidate: u32,
}

impl FeatureExtent {
    /// 本范围的所有消费方都必须等待的最远切比雪夫区块半径
    /// （写入 ∪ 读取 ∪ 冲突）。
    ///
    /// The furthest Chebyshev chunk radius any consumer of this extent must
    /// wait on (write ∪ read ∪ conflict).
    pub fn max_halo_chunks(&self) -> u8 {
        self.write_radius_chunks
            .max(self.read_radius_chunks)
            .max(self.conflict_radius_chunks)
    }
}

/// 所有已启用特征类型的范围注册表。注册在 [`FeatureType`] 上是完备的：
/// 构造函数为每个变体各取一个范围。
///
/// Registry of extents for all enabled feature types. Registration is total
/// over [`FeatureType`]: the constructor takes one extent per variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtentRegistry {
    entries: Vec<FeatureExtent>,
    /// 每个目标区块的依赖闭包预算。
    ///
    /// Dependency-closure budget per target chunk.
    max_dependency_cells_per_target: usize,
}

impl ExtentRegistry {
    /// 校验完备性（每个特征类型一个条目、无重复）并安装闭包预算。
    ///
    /// Validates completeness (one entry per feature type, no duplicates) and
    /// installs the closure budget.
    pub fn new(
        entries: Vec<FeatureExtent>,
        max_dependency_cells_per_target: usize,
    ) -> Result<Self, GenerationContractError> {
        if entries.len() != FEATURE_TYPE_COUNT {
            return Err(GenerationContractError::InvalidConfiguration(format!(
                "extent registry expects one entry per feature type ({FEATURE_TYPE_COUNT}), got {}",
                entries.len()
            )));
        }
        let mut seen = [false; FEATURE_TYPE_COUNT];
        for extent in &entries {
            let disc = extent.feature_type.discriminator() as usize;
            if disc == 0 || disc > FEATURE_TYPE_COUNT || seen[disc - 1] {
                return Err(GenerationContractError::InvalidConfiguration(format!(
                    "duplicate or unknown feature type discriminator {disc} in extent registry"
                )));
            }
            seen[disc - 1] = true;
        }
        if max_dependency_cells_per_target == 0 {
            return Err(GenerationContractError::InvalidConfiguration(
                "max_dependency_cells_per_target must be positive".to_string(),
            ));
        }
        Ok(Self { entries, max_dependency_cells_per_target })
    }

    /// 返回某特征类型的已注册范围。
    ///
    /// Returns the registered extent of a feature type.
    pub fn extent_of(&self, feature_type: FeatureType) -> &FeatureExtent {
        let disc = feature_type.discriminator() as usize;
        &self.entries[disc - 1]
    }

    /// 目标区块的完备源单元格闭包：位于所有已注册类型的合并视野范围内的
    /// 每个单元格。没有特征的源仍然会收到封包，因此即使集合中包含空单元格，
    /// 该集合也是权威的。
    ///
    /// Complete origin-cell closure for a target chunk: every cell within the
    /// union halo of all registered types. Sources with no features still
    /// receive seals, so this set is authoritative even when it contains
    /// empty cells.
    pub fn needs_for(
        &self,
        target: OriginCell,
    ) -> Result<BTreeSet<OriginCell>, GenerationContractError> {
        let halo = self
            .entries
            .iter()
            .map(FeatureExtent::max_halo_chunks)
            .max()
            .unwrap_or(0);
        let radius = i32::from(halo);
        let span = 2 * radius + 1;
        let cells = span.saturating_mul(span) as usize;
        if cells > self.max_dependency_cells_per_target {
            return Err(GenerationContractError::DependencyBudgetExceeded {
                target_x: target.x,
                target_z: target.z,
                required_cells: cells,
                budget: self.max_dependency_cells_per_target,
            });
        }
        let mut set = BTreeSet::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                set.insert(OriginCell::new(target.x + dx, target.z + dz));
            }
        }
        Ok(set)
    }

    /// 目标区块的按类型闭包：该类型的哪些源的特征可以与目标相交
    /// （仅写入覆盖；读取与冲突视野的依赖展开由各阶段运行器负责）。
    ///
    /// Per-type closure for a target chunk: which origins' features of this
    /// type can intersect the target (write footprint only; read and conflict
    /// halos are dependency-expansion work for the stage runners).
    pub fn origins_for_type(
        &self,
        target: OriginCell,
        feature_type: FeatureType,
    ) -> Result<Vec<OriginCell>, GenerationContractError> {
        let extent = self.extent_of(feature_type);
        let radius = i32::from(extent.write_radius_chunks);
        let span = 2 * radius + 1;
        let cells = span.saturating_mul(span) as usize;
        if cells > self.max_dependency_cells_per_target {
            return Err(GenerationContractError::DependencyBudgetExceeded {
                target_x: target.x,
                target_z: target.z,
                required_cells: cells,
                budget: self.max_dependency_cells_per_target,
            });
        }
        let mut origins = Vec::with_capacity(cells);
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                origins.push(OriginCell::new(target.x + dx, target.z + dz));
            }
        }
        Ok(origins)
    }

    /// 根据已注册上界校验一次观测到的特征结果。超出上界属于拒绝该次尝试的
    /// 生成错误——写入绝不为了迁就边界而被裁剪。
    ///
    /// Validates one observed feature result against its registered bounds:
    /// exceeding a bound is a generation error that rejects the attempt —
    /// writes are never clipped to fit.
    pub fn validate_result(
        &self,
        feature_type: FeatureType,
        origin: OriginCell,
        target: OriginCell,
        write_distance_chunks: u8,
        vertical_span: i32,
        total_writes: u32,
    ) -> Result<(), GenerationContractError> {
        let extent = self.extent_of(feature_type);
        let name = format!("{feature_type:?}");
        if u32::from(write_distance_chunks) > u32::from(extent.write_radius_chunks) {
            return Err(GenerationContractError::ExtentExceeded {
                feature: name,
                origin_x: origin.x,
                origin_z: origin.z,
                observed: "write radius (chunks)",
                bound: i64::from(extent.write_radius_chunks),
            });
        }
        let chunk_distance = (origin.x - target.x).abs().max((origin.z - target.z).abs());
        if chunk_distance > i32::from(extent.write_radius_chunks) {
            return Err(GenerationContractError::ExtentExceeded {
                feature: name,
                origin_x: origin.x,
                origin_z: origin.z,
                observed: "target chunk distance",
                bound: i64::from(extent.write_radius_chunks),
            });
        }
        if vertical_span.abs() > extent.vertical_span {
            return Err(GenerationContractError::ExtentExceeded {
                feature: name,
                origin_x: origin.x,
                origin_z: origin.z,
                observed: "vertical span",
                bound: i64::from(extent.vertical_span),
            });
        }
        if total_writes > extent.max_writes_per_candidate {
            return Err(GenerationContractError::ExtentExceeded {
                feature: name,
                origin_x: origin.x,
                origin_z: origin.z,
                observed: "total writes",
                bound: i64::from(extent.max_writes_per_candidate),
            });
        }
        Ok(())
    }
}

/// 已注册的 [`FeatureType`] 变体数量。
///
/// Number of registered [`FeatureType`] variants.
pub const FEATURE_TYPE_COUNT: usize = 5;

/// 内置配置的初始范围集合（覆盖矿石、树木、地面装饰、沙漠水井、沼泽小屋）。
/// 取值是保守的初始上界，属于描述符 `feature_rules_version` 身份的一部分。
///
/// The initial extent set for the built-in profile (ores, trees, ground
/// decoration, desert well, swamp hut). Values are conservative starting
/// bounds and part of the descriptor's `feature_rules_version` identity.
pub fn builtin_registry() -> ExtentRegistry {
    let entries = vec![
        // Ores are currently in-chunk (write radius 0); if that ever changes,
        // re-runs must go through the same closure pipeline.
        FeatureExtent {
            feature_type: FeatureType::Ore,
            write_radius_chunks: 0,
            read_radius_chunks: 0,
            conflict_radius_chunks: 0,
            vertical_span: 384,
            max_candidates_per_cell: 512,
            max_writes_per_candidate: 4096,
        },
        FeatureExtent {
            feature_type: FeatureType::Tree,
            write_radius_chunks: 1,
            read_radius_chunks: 1,
            conflict_radius_chunks: 1,
            vertical_span: 64,
            max_candidates_per_cell: 64,
            max_writes_per_candidate: 2048,
        },
        FeatureExtent {
            feature_type: FeatureType::GroundDecoration,
            write_radius_chunks: 1,
            read_radius_chunks: 0,
            conflict_radius_chunks: 0,
            vertical_span: 8,
            max_candidates_per_cell: 256,
            max_writes_per_candidate: 512,
        },
        FeatureExtent {
            feature_type: FeatureType::DesertWell,
            write_radius_chunks: 2,
            read_radius_chunks: 2,
            conflict_radius_chunks: 2,
            vertical_span: 32,
            max_candidates_per_cell: 2,
            max_writes_per_candidate: 8192,
        },
        FeatureExtent {
            feature_type: FeatureType::SwampHut,
            write_radius_chunks: 2,
            read_radius_chunks: 2,
            conflict_radius_chunks: 2,
            vertical_span: 48,
            max_candidates_per_cell: 1,
            max_writes_per_candidate: 16384,
        },
    ];
    ExtentRegistry::new(entries, 1024).expect("builtin extents are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_closure_covers_every_registered_halo() {
        let registry = builtin_registry();
        // The widest halo (structures, radius 2) drives the closure.
        let target = OriginCell::new(10, -3);
        let needs = registry.needs_for(target).unwrap();
        assert_eq!(needs.len(), 25); // (2*2+1)^2
        assert!(needs.contains(&OriginCell::new(8, -5)));
        assert!(needs.contains(&OriginCell::new(12, -1)));
        assert!(!needs.contains(&OriginCell::new(7, -3)));
        // Every per-type origin list is a subset of the full closure.
        for feature_type in [
            FeatureType::Ore,
            FeatureType::Tree,
            FeatureType::GroundDecoration,
            FeatureType::DesertWell,
            FeatureType::SwampHut,
        ] {
            for origin in registry.origins_for_type(target, feature_type).unwrap() {
                assert!(needs.contains(&origin), "{feature_type:?} origin {origin:?} outside closure");
            }
        }
    }

    #[test]
    fn closure_budget_failure_is_loud_not_truncated() {
        let registry = ExtentRegistry::new(
            builtin_registry().entries,
            9, // below the 25-cell requirement
        )
        .unwrap();
        match registry.needs_for(OriginCell::new(0, 0)) {
            Err(GenerationContractError::DependencyBudgetExceeded {
                required_cells, budget, ..
            }) => {
                assert_eq!(required_cells, 25);
                assert_eq!(budget, 9);
            }
            other => panic!("expected budget failure, got {other:?}"),
        }
    }

    #[test]
    fn registry_rejects_incomplete_or_duplicate_entries() {
        let mut entries = builtin_registry().entries;
        entries.pop(); // incomplete
        assert!(ExtentRegistry::new(entries, 1024).is_err());
        let mut entries = builtin_registry().entries;
        entries[0].feature_type = entries[1].feature_type; // duplicate
        assert!(ExtentRegistry::new(entries, 1024).is_err());
    }

    #[test]
    fn result_validation_rejects_extent_violations() {
        let registry = builtin_registry();
        let origin = OriginCell::new(0, 0);
        let target = OriginCell::new(1, 0); // within tree write radius 1
        assert_eq!(
            registry.validate_result(FeatureType::Tree, origin, target, 1, 30, 512),
            Ok(())
        );
        // Tree anchored 2 chunks away from the target: outside its radius.
        assert!(matches!(
            registry.validate_result(FeatureType::Tree, origin, OriginCell::new(2, 0), 1, 30, 512),
            Err(GenerationContractError::ExtentExceeded { .. })
        ));
        // Vertical span beyond the tree bound.
        assert!(matches!(
            registry.validate_result(FeatureType::Tree, origin, target, 1, 65, 512),
            Err(GenerationContractError::ExtentExceeded { observed: "vertical span", .. })
        ));
        // Write count beyond the bound.
        assert!(matches!(
            registry.validate_result(FeatureType::Tree, origin, target, 1, 30, 2049),
            Err(GenerationContractError::ExtentExceeded { observed: "total writes", .. })
        ));
    }

    #[test]
    fn builtin_extents_cover_the_decided_initial_feature_set() {
        let registry = builtin_registry();
        assert_eq!(registry.entries.len(), FEATURE_TYPE_COUNT);
        // Structures start with desert well + swamp hut only.
        assert_eq!(registry.extent_of(FeatureType::DesertWell).max_halo_chunks(), 2);
        assert_eq!(registry.extent_of(FeatureType::SwampHut).max_halo_chunks(), 2);
        assert_eq!(registry.extent_of(FeatureType::Ore).write_radius_chunks, 0);
    }
}
