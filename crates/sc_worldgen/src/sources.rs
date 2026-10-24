//! 自持有的世界生成源 bundle：键为 `<namespace>/<path>`（不带 `.json`），
//! 与包布局 `definitions/worldgen/<category>/<namespace>/<path>.json` 对应。
//! 各映射使用 `BTreeMap`，因此无论 bundle 如何组装，迭代顺序都是确定的。
//!
//! Owned worldgen source bundle: keys are `<namespace>/<path>` (no `.json`),
//! matching the pack layout `definitions/worldgen/<category>/<namespace>/<path>.json`.
//! Maps are `BTreeMap`s so iteration order is deterministic regardless of how
//! the bundle was assembled.

use std::collections::BTreeMap;

/// 单个世界生成资源类别。
///
/// One worldgen resource category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum WorldgenCategory {
    DensityFunction,
    Noise,
    NoiseSettings,
}

impl WorldgenCategory {
    /// Java 数据布局使用的资源名（`worldgen/<name>/...`）。
    ///
    /// Resource name used by the Java data layout (`worldgen/<name>/...`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::DensityFunction => "density_function",
            Self::Noise => "noise",
            Self::NoiseSettings => "noise_settings",
        }
    }

    /// 全部类别，按稳定顺序排列。
    ///
    /// All categories in stable order.
    pub const ALL: [WorldgenCategory; 3] = [
        WorldgenCategory::DensityFunction,
        WorldgenCategory::Noise,
        WorldgenCategory::NoiseSettings,
    ];
}

/// 自持有的、确定性的世界生成 JSON bundle。
///
/// Owned, deterministic worldgen JSON bundle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldgenSources {
    pub density_functions: BTreeMap<String, Vec<u8>>,
    pub noises: BTreeMap<String, Vec<u8>>,
    pub noise_settings: BTreeMap<String, Vec<u8>>,
}

impl WorldgenSources {
    /// 跨类别的文件总数。
    ///
    /// Total file count across categories.
    pub fn len(&self) -> usize {
        self.density_functions.len() + self.noises.len() + self.noise_settings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 某个类别的文件，键为 `<namespace>/<path>`。
    ///
    /// Files of one category, keyed by `<namespace>/<path>`.
    pub fn category(&self, category: WorldgenCategory) -> &BTreeMap<String, Vec<u8>> {
        match category {
            WorldgenCategory::DensityFunction => &self.density_functions,
            WorldgenCategory::Noise => &self.noises,
            WorldgenCategory::NoiseSettings => &self.noise_settings,
        }
    }

    /// 在某个类别中查找形如 `minecraft:overworld` 的资源引用，返回 bundle
    /// 键（`minecraft/overworld`）对应的内容。
    ///
    /// Looks up a resource reference like `minecraft:overworld` in a
    /// category. Returns the bundle key (`minecraft/overworld`).
    pub fn resolve(
        &self,
        category: WorldgenCategory,
        resource: &str,
    ) -> Option<&Vec<u8>> {
        let (namespace, path) = resource.split_once(':')?;
        self.category(category).get(&format!("{namespace}/{path}"))
    }

    /// 对全部文件计算确定性 FNV-1a 64：按 `category|key|fnv64(content)`
    /// 行排序后汇总。必须与抽取来源清单中的 `closure_fingerprint` 一致。
    ///
    /// Deterministic FNV-1a 64 over every file, ordered by
    /// `category|key|fnv64(content)` lines. Must equal the extraction
    /// provenance manifest's `closure_fingerprint`.
    pub fn content_fingerprint(&self) -> u64 {
        let mut lines: Vec<Vec<u8>> = Vec::with_capacity(self.len());
        for category in WorldgenCategory::ALL {
            for (key, bytes) in self.category(category) {
                lines.push(
                    format!(
                        "{}|{}|{:016x}",
                        category.name(),
                        key,
                        crate::fnv1a64(bytes)
                    )
                    .into_bytes(),
                );
            }
        }
        lines.sort();
        crate::fnv1a64(&lines.join(&b'\n'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> WorldgenSources {
        WorldgenSources {
            density_functions: BTreeMap::from([(
                "minecraft/overworld/continents".to_string(),
                b"{\"type\":\"minecraft:shifted_noise\"}".to_vec(),
            )]),
            noises: BTreeMap::from([(
                "minecraft/overworld/temperature".to_string(),
                b"{\"firstOctave\":-9,\"amplitudes\":[]}".to_vec(),
            )]),
            noise_settings: BTreeMap::from([(
                "minecraft/overworld".to_string(),
                b"{\"sea_level\":63}".to_vec(),
            )]),
        }
    }

    #[test]
    fn resource_resolution_maps_namespace_refs_onto_bundle_keys() {
        let sources = sample();
        assert!(sources
            .resolve(WorldgenCategory::NoiseSettings, "minecraft:overworld")
            .is_some());
        assert!(sources
            .resolve(WorldgenCategory::DensityFunction, "minecraft:overworld/continents")
            .is_some());
        assert!(sources
            .resolve(WorldgenCategory::Noise, "minecraft:missing")
            .is_none());
        // A ref without a namespace cannot resolve.
        assert!(sources.resolve(WorldgenCategory::Noise, "overworld").is_none());
    }

    #[test]
    fn content_fingerprint_is_insertion_order_independent_and_content_sensitive() {
        let a = sample();
        // Same files, different insertion order into the maps.
        let mut b = WorldgenSources::default();
        b.noise_settings.insert("minecraft/overworld".into(), b"{\"sea_level\":63}".to_vec());
        b.noises.insert("minecraft/overworld/temperature".into(), b"{\"firstOctave\":-9,\"amplitudes\":[]}".to_vec());
        b.density_functions.insert("minecraft/overworld/continents".into(), b"{\"type\":\"minecraft:shifted_noise\"}".to_vec());
        assert_eq!(a.content_fingerprint(), b.content_fingerprint());

        // Any content byte change changes the fingerprint.
        let mut c = sample();
        c.noise_settings.insert("minecraft/overworld".into(), b"{\"sea_level\":64}".to_vec());
        assert_ne!(a.content_fingerprint(), c.content_fingerprint());

        // Adding a file changes it too.
        let mut d = sample();
        d.noises.insert("minecraft/extra".into(), b"{}".to_vec());
        assert_ne!(a.content_fingerprint(), d.content_fingerprint());
    }
}
