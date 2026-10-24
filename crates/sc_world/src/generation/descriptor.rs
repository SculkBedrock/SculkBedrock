//! 持久化的生成器描述符与世界准入检查。
//!
//! 描述符是世界显式的生成语义契约。已存世界只有当其记录的描述符与当前内置
//! 生成器完全一致时才允许继续生成；其他任何情况都是
//! [`GenerationContractError::DescriptorMismatch`] 错误并保留存档。
//! 不存在旧版回退，也不存在静默重新生成。
//!
//! Persisted generator descriptor and world-admission checks.
//!
//! The descriptor is the world's explicit generation semantics contract. A
//! stored world may only continue generating when its recorded descriptor
//! matches the current built-in generator exactly; anything else is an
//! [`GenerationContractError::DescriptorMismatch`] that preserves the save.
//! There is no legacy fallback and no silent re-generation.

use crate::generation::error::GenerationContractError;
use serde::{Deserialize, Serialize};

/// 当前内置生成器的 id。
///
/// Current built-in generator id.
pub const BUILTIN_GENERATOR_ID: &str = "builtin_density_three_layer_v1";

/// 从所选 `noise_settings` 解析出的单元格格点设置：它们会改变地形输出，
/// 因此属于语义身份的一部分。取值由地形准备流程填充。
///
/// Cell lattice settings resolved from the selected `noise_settings`: they
/// are part of the semantic identity because they change terrain output.
/// Values are populated by the terrain preparation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellSettings {
    pub cell_size_xz: u32,
    pub cell_height: u32,
    pub min_y: i32,
    pub world_height: u32,
}

impl Default for CellSettings {
    fn default() -> Self {
        // Overworld defaults matching the current implementation constants
        // (`worldgen/stages/terrain.rs`); hardcoded values are replaced by
        // values parsed from the selected noise settings.
        Self { cell_size_xz: 4, cell_height: 8, min_y: -64, world_height: 384 }
    }
}

/// 单个世界/维度的显式生成器语义。运行时旋钮（worker 数量、区域网格大小、
/// 视距）被刻意排除：它们绝不能改变地形摘要。
///
/// Explicit generator semantics for one world/dimension. Runtime knobs
/// (worker counts, region grid size, view distance) are deliberately absent:
/// they must never change the terrain digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratorDescriptor {
    /// 内置生成器 id；唯一被接受的值是 [`BUILTIN_GENERATOR_ID`]。
    ///
    /// Built-in generator id; the only accepted value is
    /// [`BUILTIN_GENERATOR_ID`].
    pub generator_id: String,
    /// 内置算法本身的语义版本号；任何影响输出的修复都会提升它。
    ///
    /// Semantic version of the built-in algorithm itself. Any output-affecting
    /// fix bumps this.
    pub algorithm_version: u32,
    /// 锁定的 Minecraft Java worldgen 数据版本（26.3）。
    ///
    /// Pinned Minecraft Java worldgen data version (26.3).
    pub data_version: String,
    /// 对已解析资源闭包（排序后的资源 id + 内容摘要）计算的 FNV-1a 64 值，
    /// 由资源准备步骤计算。
    ///
    /// FNV-1a 64 over the resolved resource closure (sorted resource ids +
    /// content digests), computed by the resource preparation step.
    pub data_closure_fingerprint: u64,
    /// 供给生成过程的方块/生物群系注册表快照的指纹。
    ///
    /// Fingerprint of the block/biome registry snapshot feeding generation.
    pub registry_fingerprint: u64,
    /// 世界种子（来自 level.dat 的 `RandomSeed`）。
    ///
    /// World seed (from level.dat `RandomSeed`).
    pub seed: i64,
    /// 所选噪声设置的根资源，例如 `minecraft:overworld`。
    ///
    /// Selected noise settings root resource, e.g. `minecraft:overworld`.
    pub noise_settings_root: String,
    /// 从噪声设置解析出的单元格格点。
    ///
    /// Cell lattice resolved from the noise settings.
    pub cell: CellSettings,
    /// 特征排序/范围/冲突规则集的版本号。
    ///
    /// Version of the feature ordering/extent/conflict rule set.
    pub feature_rules_version: u32,
}

impl GeneratorDescriptor {
    /// 当前内置主世界配置的描述符。
    ///
    /// The descriptor for the current built-in overworld profile.
    pub fn builtin_overworld(
        seed: i64,
        data_closure_fingerprint: u64,
        registry_fingerprint: u64,
    ) -> Self {
        Self {
            generator_id: BUILTIN_GENERATOR_ID.to_string(),
            algorithm_version: 1,
            data_version: DATA_VERSION.to_string(),
            data_closure_fingerprint,
            registry_fingerprint,
            seed,
            noise_settings_root: "minecraft:overworld".to_string(),
            cell: CellSettings::default(),
            feature_rules_version: 1,
        }
    }

    /// 用自定义根资源与单元格设置构造内置配置的描述符。
    ///
    /// Constructs a descriptor for a built-in profile with custom root and
    /// cell settings.
    pub fn new_builtin(
        seed: i64,
        noise_settings_root: impl Into<String>,
        data_closure_fingerprint: u64,
        registry_fingerprint: u64,
        cell: CellSettings,
    ) -> Self {
        Self {
            generator_id: BUILTIN_GENERATOR_ID.to_string(),
            algorithm_version: 1,
            data_version: DATA_VERSION.to_string(),
            data_closure_fingerprint,
            registry_fingerprint,
            seed,
            noise_settings_root: noise_settings_root.into(),
            cell,
            feature_rules_version: 1,
        }
    }

    /// 对语义字段计算的 FNV-1a 64 值，用作
    /// [`crate::generation::FeatureId`] 的 `generator_fingerprint` 组成部分。
    ///
    /// FNV-1a 64 over the semantic fields, used as the
    /// `generator_fingerprint` component of [`crate::generation::FeatureId`].
    pub fn fingerprint(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        fn feed(h: &mut u64, bytes: &[u8]) {
            for b in bytes {
                *h ^= u64::from(*b);
                *h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        feed(&mut h, self.generator_id.as_bytes());
        feed(&mut h, &self.algorithm_version.to_le_bytes());
        feed(&mut h, self.data_version.as_bytes());
        feed(&mut h, &self.data_closure_fingerprint.to_le_bytes());
        feed(&mut h, &self.registry_fingerprint.to_le_bytes());
        feed(&mut h, &self.seed.to_le_bytes());
        feed(&mut h, self.noise_settings_root.as_bytes());
        feed(&mut h, &self.cell.cell_size_xz.to_le_bytes());
        feed(&mut h, &self.cell.cell_height.to_le_bytes());
        feed(&mut h, &self.cell.min_y.to_le_bytes());
        feed(&mut h, &self.cell.world_height.to_le_bytes());
        feed(&mut h, &self.feature_rules_version.to_le_bytes());
        h
    }

    /// 已存世界的准入检查：存储的描述符必须逐字段一致。缺失描述符属于
    /// 不匹配，而不是空的特征计划。
    ///
    /// Admission check for a stored world: the stored descriptor must match
    /// field-for-field. Missing descriptors are a mismatch, not an empty
    /// feature plan.
    pub fn ensure_matches_stored(&self, stored: Option<&GeneratorDescriptor>) -> Result<(), GenerationContractError> {
        let Some(stored) = stored else {
            return Err(GenerationContractError::DescriptorMismatch {
                detail: "world has no stored generator descriptor (pre-contract save)".to_string(),
            });
        };
        if self == stored {
            Ok(())
        } else if self.seed == stored.seed && self.generator_id == stored.generator_id {
            Err(GenerationContractError::DescriptorMismatch {
                detail: format!(
                    "semantic fields differ: stored (algorithm v{}, data {}, closure {:#018x}, registry {:#018x}, root {}, cell {:?}) vs current (algorithm v{}, data {}, closure {:#018x}, registry {:#018x}, root {}, cell {:?})",
                    stored.algorithm_version,
                    stored.data_version,
                    stored.data_closure_fingerprint,
                    stored.registry_fingerprint,
                    stored.noise_settings_root,
                    (stored.cell.cell_size_xz, stored.cell.cell_height, stored.cell.min_y, stored.cell.world_height),
                    self.algorithm_version,
                    self.data_version,
                    self.data_closure_fingerprint,
                    self.registry_fingerprint,
                    self.noise_settings_root,
                    (self.cell.cell_size_xz, self.cell.cell_height, self.cell.min_y, self.cell.world_height),
                ),
            })
        } else {
            Err(GenerationContractError::DescriptorMismatch {
                detail: "stored descriptor belongs to a different generator or seed".to_string(),
            })
        }
    }
}

/// 锁定的 Minecraft Java worldgen 数据版本（26.3）。
///
/// Pinned Minecraft Java worldgen data version (26.3).
pub const DATA_VERSION: &str = "26.3";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_preserves_all_semantic_fields() {
        let descriptor = GeneratorDescriptor::builtin_overworld(-12345, 0xfe, 0xdc);
        let bytes = serde_json::to_vec(&descriptor).unwrap();
        let parsed: GeneratorDescriptor = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, descriptor);
        assert_eq!(parsed.fingerprint(), descriptor.fingerprint());
    }

    #[test]
    fn fingerprint_changes_with_every_semantic_field() {
        let base = GeneratorDescriptor::builtin_overworld(1, 2, 3);
        let mut variant = base.clone();
        variant.algorithm_version += 1;
        assert_ne!(base.fingerprint(), variant.fingerprint());
        let mut variant = base.clone();
        variant.data_closure_fingerprint += 1;
        assert_ne!(base.fingerprint(), variant.fingerprint());
        let mut variant = base.clone();
        variant.registry_fingerprint += 1;
        assert_ne!(base.fingerprint(), variant.fingerprint());
        let mut variant = base.clone();
        variant.seed += 1;
        assert_ne!(base.fingerprint(), variant.fingerprint());
        let mut variant = base.clone();
        variant.cell.cell_size_xz += 1;
        assert_ne!(base.fingerprint(), variant.fingerprint());
        let mut variant = base.clone();
        variant.noise_settings_root = "minecraft:nether".into();
        assert_ne!(base.fingerprint(), variant.fingerprint());
        // Runtime-only knobs are not representable: no field can encode them.
    }

    #[test]
    fn admission_rejects_missing_and_mismatched_descriptors() {
        let current = GeneratorDescriptor::builtin_overworld(42, 7, 9);
        // Missing descriptor: pre-contract saves are refused, not regenerated.
        assert!(matches!(
            current.ensure_matches_stored(None),
            Err(GenerationContractError::DescriptorMismatch { .. })
        ));
        // Identical stored descriptor: admitted.
        assert_eq!(current.ensure_matches_stored(Some(&current)), Ok(()));
        // Same seed, changed semantics: refused with a field-level detail.
        let drifted = GeneratorDescriptor { algorithm_version: 2, ..current.clone() };
        assert!(matches!(
            current.ensure_matches_stored(Some(&drifted)),
            Err(GenerationContractError::DescriptorMismatch { .. })
        ));
        // Different generator/seed entirely.
        let foreign = GeneratorDescriptor::builtin_overworld(43, 7, 9);
        assert!(matches!(
            current.ensure_matches_stored(Some(&foreign)),
            Err(GenerationContractError::DescriptorMismatch { .. })
        ));
    }

    #[test]
    fn builtin_descriptor_pins_decided_data_version() {
        let descriptor = GeneratorDescriptor::builtin_overworld(0, 0, 0);
        assert_eq!(descriptor.generator_id, BUILTIN_GENERATOR_ID);
        assert_eq!(descriptor.data_version, "26.3");
        assert_eq!(descriptor.noise_settings_root, "minecraft:overworld");
    }
}
