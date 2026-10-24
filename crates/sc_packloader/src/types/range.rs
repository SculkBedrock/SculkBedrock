use serde::Deserialize;

/// 数值范围表示：单值、`[min, max]` 二元组或显式范围结构体。
///
/// Range representation: a single value, a `[min, max]` pair, or an explicit range struct.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MinecraftRangeType<T> {
    /// 单个数值。
    ///
    /// A single number.
    Number(T),
    /// 二元组 `[min, max]`。
    ///
    /// A `[min, max]` pair.
    Vec([T; 2]),
    /// 显式范围结构体。
    ///
    /// An explicit range struct.
    Range(MinecraftRange<T>),
}

impl<T: Default> Default for MinecraftRangeType<T> {
    /// 默认表示：取元素类型的默认值作为单值。
    ///
    /// Default representation: the element type's default as a single value.
    fn default() -> Self {
        Self::Number(T::default())
    }
}

impl<T: Copy> MinecraftRangeType<T> {
    /// 展开为 `(下界, 上界)`；单值表示上下界相同。
    ///
    /// Collapses to `(lower, upper)`; a single value yields equal bounds.
    pub fn bounds(&self) -> (T, T) {
        match self {
            Self::Number(value) => (*value, *value),
            Self::Vec([min, max]) => (*min, *max),
            Self::Range(range) => (range.range_min, range.range_max),
        }
    }
}

/// 显式数值范围 `[range_min, range_max]`。
///
/// Explicit numeric range `[range_min, range_max]`.
///
/// `min` / `max` 是 Bedrock 官方写法（如 `minecraft:durability` 的
/// `damage_chance`），与 `range_min` / `range_max` 等价。
///
/// `min` / `max` are the official Bedrock spelling (e.g. the `damage_chance`
/// of `minecraft:durability`) and are equivalent to `range_min` / `range_max`.
#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftRange<T> {
    /// 范围下界。
    ///
    /// Lower bound of the range.
    #[serde(alias = "min")]
    pub range_min: T,
    /// 范围上界。
    ///
    /// Upper bound of the range.
    #[serde(alias = "max")]
    pub range_max: T,
}

impl<T> MinecraftRange<T> {
    /// 用给定的上下界构造范围。
    ///
    /// Constructs a range with the given bounds.
    ///
    /// - `range_min`: 范围下界
    /// - `range_max`: 范围上界
    pub fn new(range_min: T, range_max: T) -> Self {
        Self {
            range_min,
            range_max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_min_max_spelling_deserializes() {
        // Bedrock item JSON writes `{"min":..,"max":..}` (durability damage_chance).
        let range: MinecraftRangeType<f64> =
            serde_json::from_str(r#"{"min":0,"max":100}"#).unwrap();
        assert_eq!(range.bounds(), (0.0, 100.0));
    }

    #[test]
    fn legacy_forms_still_deserialize() {
        let single: MinecraftRangeType<f64> = serde_json::from_str("100").unwrap();
        assert_eq!(single.bounds(), (100.0, 100.0));
        let pair: MinecraftRangeType<f64> = serde_json::from_str("[1,2]").unwrap();
        assert_eq!(pair.bounds(), (1.0, 2.0));
        let explicit: MinecraftRangeType<f64> =
            serde_json::from_str(r#"{"range_min":3,"range_max":4}"#).unwrap();
        assert_eq!(explicit.bounds(), (3.0, 4.0));
    }
}
