use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MinecraftRangeType<T> {
    Number(T),
    Vec([T; 2]),
    Range(MinecraftRange<T>),
}

impl<T: Default> Default for MinecraftRangeType<T> {
    fn default() -> Self {
        Self::Number(T::default())
    }
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftRange<T> {
    pub range_min: T,
    pub range_max: T,
}

impl<T> MinecraftRange<T> {
    pub fn new(range_min: T, range_max: T) -> Self {
        Self {
            range_min,
            range_max,
        }
    }
}
