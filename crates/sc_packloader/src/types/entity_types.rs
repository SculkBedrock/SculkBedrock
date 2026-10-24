use crate::types::filter::MinecraftFilterType;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

/// 实体类型条件：空、单个条件或条件列表。
///
/// Entity type condition: none, a single condition, or a list of conditions.
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(untagged)]
pub enum MinecraftEntityTypes {
    /// 无条件（空）。
    ///
    /// No condition (null).
    #[default]
    Null,
    /// 条件列表。
    ///
    /// A list of conditions.
    Vec(Vec<MinecraftEntityType>),
    /// 单个条件。
    ///
    /// A single condition.
    Single(MinecraftEntityType),
}

/// 实体生成/过滤条件参数（距离、可见性、速度倍率等）。
///
/// Entity spawning/filter condition parameters (distance, visibility, speed multipliers, etc.).
#[serde_inline_default]
#[derive(Deserialize, Debug, Default, Clone)]
pub struct MinecraftEntityType {
    /// 附加过滤条件。
    ///
    /// Additional filter conditions.
    #[serde(default)]
    pub filters: MinecraftFilterType,
    /// 与玩家的最大距离。
    ///
    /// Maximum distance to players.
    #[serde_inline_default(16.0)]
    pub max_dist: f64,
    /// 是否要求实体在玩家视线内。
    ///
    /// Whether the entity must be visible to players.
    #[serde_inline_default(false)]
    pub must_see: bool,
    /// 保持可见状态的记忆时长（秒）。
    ///
    /// How long (in seconds) visibility is remembered after being lost.
    #[serde_inline_default(3.0)]
    pub must_see_forget_duration: f64,
    /// 是否重新评估生成描述。
    ///
    /// Whether the spawn description is re-evaluated.
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    /// 冲刺速度倍率。
    ///
    /// Sprint speed multiplier.
    #[serde_inline_default(1.0)]
    pub sprint_speed_multiplier: f64,
    /// 行走速度倍率。
    ///
    /// Walk speed multiplier.
    #[serde_inline_default(1.0)]
    pub walk_speed_multiplier: f64,
    /// 条件优先级。
    ///
    /// Condition priority.
    #[serde(default)]
    pub priority: i32,
}
