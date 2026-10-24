use serde::{Deserialize, Deserializer};

/// 噪声地形类型（生物群系生成参数之一）。
///
/// Noise terrain type (a biome generation parameter).
#[derive(Debug, Clone)]
pub enum NoiseType {
    /// 默认地形噪声。
    ///
    /// Default terrain noise.
    Default,
    /// 变形后的默认地形噪声。
    ///
    /// Mutated default terrain noise.
    DefaultMutated,
    /// 河流地形噪声。
    ///
    /// River terrain noise.
    River,
    /// 海洋地形噪声。
    ///
    /// Ocean terrain noise.
    Ocean,
    /// 深海地形噪声。
    ///
    /// Deep ocean terrain noise.
    DeepOcean,
    /// 低地地形噪声。
    ///
    /// Lowlands terrain noise.
    Lowlands,
    /// 针叶林地形噪声。
    ///
    /// Taiga terrain noise.
    Taiga,
    /// 山地地形噪声。
    ///
    /// Mountains terrain noise.
    Mountains,
    /// 高地地形噪声。
    ///
    /// Highlands terrain noise.
    Highlands,
    /// 极端地形噪声。
    ///
    /// Extreme terrain noise.
    Extreme,
    /// 次极端地形噪声。
    ///
    /// Less extreme terrain noise.
    LessExtreme,
    /// 沙滩地形噪声。
    ///
    /// Beach terrain noise.
    Beach,
    /// 石质沙滩地形噪声。
    ///
    /// Stone beach terrain noise.
    StoneBeach,
    /// 蘑菇岛地形噪声。
    ///
    /// Mushroom island terrain noise.
    Mushroom,
    /// 沼泽地形噪声。
    ///
    /// Swamp terrain noise.
    Swamp,
}

/// 从字符串反序列化噪声类型，未知值回退为 [`NoiseType::Default`]。
///
/// Deserializes a noise type from a string, falling back to
/// [`NoiseType::Default`] for unknown values.
impl<'de> Deserialize<'de> for NoiseType {
    /// 从字符串反序列化噪声类型；未知值回退为 [`NoiseType::Default`]。
    ///
    /// Deserializes a noise type from a string; unknown values fall back to
    /// [`NoiseType::Default`].
    ///
    /// - `deserializer`: serde 反序列化器
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ty = String::deserialize(deserializer)?;
        Ok(match ty.as_ref() {
            "default" => NoiseType::Default,
            "default_mutated" => NoiseType::DefaultMutated,
            "river" => NoiseType::River,
            "ocean" => NoiseType::Ocean,
            "deep_ocean" => NoiseType::DeepOcean,
            "lowlands" => NoiseType::Lowlands,
            "taiga" => NoiseType::Taiga,
            "mountains" => NoiseType::Mountains,
            "highlands" => NoiseType::Highlands,
            "extreme" => NoiseType::Extreme,
            "less_extreme" => NoiseType::LessExtreme,
            "beach" => NoiseType::Beach,
            "stone_beach" => NoiseType::StoneBeach,
            "mushroom" => NoiseType::Mushroom,
            "swamp" => NoiseType::Swamp,
            _ => NoiseType::Default,
        })
    }
}
