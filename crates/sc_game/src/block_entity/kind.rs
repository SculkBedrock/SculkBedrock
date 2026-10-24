//! 有状态方块种类（BlockEntity 类型）。
//!
//! Stateful block kinds (BlockEntity types).

use sc_recipe::StationKind;

/// 有状态方块种类。
///
/// Stateful block kind.
///
/// 除熔炉家族外，这个可扩展枚举对所有承载状态的方块实体
/// （箱子、告示牌等）进行分类。
/// In addition to the furnace family, this extensible enumeration categorizes
/// all state-bearing block entities (chests, signs, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockEntityKind {
    /// 熔炉家族（高炉/烟熏炉/普通熔炉）。
    ///
    /// Furnace family (blast furnace / smoker / furnace).
    Furnace(StationKind),
    /// 箱子。
    ///
    /// Chest.
    Chest,
    /// 告示牌。
    ///
    /// Sign.
    Sign,
}

impl BlockEntityKind {
    /// 返回关联的工作站 StationKind（如适用）。
    ///
    /// Returns the associated workstation StationKind if applicable.
    ///
    /// Furnace 变体返回具体 StationKind；其余返回 `StationKind::Unknown`。
    /// For Furnace variants, returns the concrete StationKind; for others
    /// returns StationKind::Unknown.
    pub fn station(self) -> StationKind {
        match self {
            Self::Furnace(station) => station,
            _ => StationKind::Unknown,
        }
    }

    /// 规范的 Bedrock NBT 标识标签（如 "Furnace"、"Chest"、"Sign"）。
    ///
    /// Canonical Bedrock NBT identifier tag (e.g. "Furnace", "Chest", "Sign").
    pub fn nbt_id(self) -> &'static str {
        match self {
            Self::Furnace(StationKind::BlastFurnace) => "BlastFurnace",
            Self::Furnace(StationKind::Smoker) => "Smoker",
            Self::Furnace(_) => "Furnace",
            Self::Chest => "Chest",
            Self::Sign => "Sign",
        }
    }

    /// 从 Bedrock NBT 标识标签解析。
    ///
    /// Parse from Bedrock NBT identifier tag.
    ///
    /// - `id`: NBT 标识标签
    ///
    /// # Returns
    /// 解析成功返回对应种类，未知标签返回 `None`。
    /// The matching kind on success; `None` for an unknown tag.
    pub fn from_nbt_id(id: &str) -> Option<Self> {
        match id {
            "Furnace" => Some(Self::Furnace(StationKind::Furnace)),
            "BlastFurnace" => Some(Self::Furnace(StationKind::BlastFurnace)),
            "Smoker" => Some(Self::Furnace(StationKind::Smoker)),
            "Chest" => Some(Self::Chest),
            "Sign" => Some(Self::Sign),
            _ => None,
        }
    }
}
