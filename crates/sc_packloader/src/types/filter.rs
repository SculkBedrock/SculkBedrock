use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MinecraftFilterType {
    Filter(MinecraftFilter),
    Vec(Vec<MinecraftFilter>),
}

impl Default for MinecraftFilterType {
    fn default() -> Self {
        MinecraftFilterType::Filter(MinecraftFilter {
            all_of: vec![],
            any_of: vec![],
            test: "".to_string(),
            subject: "".to_string(),
            operator: "".to_string(),
            value: serde_json::Value::Null,
        })
    }
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftFilter {
    #[serde(default)]
    pub all_of: Vec<Self>,
    #[serde(default)]
    pub any_of: Vec<Self>,
    #[serde(default)]
    pub test: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub operator: String,
    #[serde(default)]
    pub value: serde_json::Value,
}
