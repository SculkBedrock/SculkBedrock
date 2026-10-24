use serde::Deserialize;

/// 过滤条件：单个过滤器或过滤器列表。
///
/// Filter condition: a single filter or a list of filters.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum MinecraftFilterType {
    /// 单个过滤器。
    ///
    /// A single filter.
    Filter(MinecraftFilter),
    /// 过滤器列表。
    ///
    /// A list of filters.
    Vec(Vec<MinecraftFilter>),
}

/// 默认实现：构造一个全空的过滤器（等价于无过滤条件）。
///
/// Default implementation: builds an all-empty filter (equivalent to no filtering).
impl Default for MinecraftFilterType {
    /// 构造空过滤器。
    ///
    /// Builds an empty filter.
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

/// 单个过滤器：逻辑组合（all_of/any_of）或具体比较（test/subject/operator/value）。
///
/// A single filter: logical composition (`all_of`/`any_of`) or a concrete
/// comparison (`test`/`subject`/`operator`/`value`).
#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftFilter {
    /// 全部满足的子条件列表（逻辑与）。
    ///
    /// Sub-conditions that must all pass (logical AND).
    #[serde(default)]
    pub all_of: Vec<Self>,
    /// 任一满足的子条件列表（逻辑或）。
    ///
    /// Sub-conditions of which any may pass (logical OR).
    #[serde(default)]
    pub any_of: Vec<Self>,
    /// 测试名称。
    ///
    /// Test name.
    #[serde(default)]
    pub test: String,
    /// 测试主体。
    ///
    /// Test subject.
    #[serde(default)]
    pub subject: String,
    /// 比较运算符。
    ///
    /// Comparison operator.
    #[serde(default)]
    pub operator: String,
    /// 比较基准值。
    ///
    /// Comparison reference value.
    #[serde(default)]
    pub value: serde_json::Value,
}
