//! 配方类型与工作台类型（对应 Microsoft Recipe Reference）。
//!
//! `RecipeKind` 是 JSON 根键（`minecraft:recipe_shaped` 等）；`StationKind`
//! 是由 `tags` 推导的合成场景（crafting_table、stonecutter、furnace 等）。
//! 两者互不推导：配方类型不隐含工作台，工作台标签也不隐含配方类型。
//!
//! Recipe and station kinds (Microsoft Recipe Reference).
//!
//! `RecipeKind` is the JSON root key (`minecraft:recipe_shaped`, ...).
//! `StationKind` is the crafting context derived from `tags`
//! (crafting_table, stonecutter, furnace, ...). Neither implies the other:
//! a recipe type never implies a station and a station tag never implies
//! a recipe type.

use serde::{Deserialize, Serialize};

/// 全部配方根键：Microsoft Recipe Reference 记录的键，加上原版包中出现的
/// 燃料/材料变体。
///
/// 线上名称保留 `minecraft:` 前缀，便于诊断信息与包文件一致。
///
/// All recipe root keys documented in the Microsoft Recipe Reference plus
/// the fuel/material variants observed in vanilla packs.
///
/// Wire names keep the `minecraft:` prefix so diagnostics match pack files.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum RecipeKind {
    /// `minecraft:recipe_shaped`
    Shaped,
    /// `minecraft:recipe_shapeless`
    Shapeless,
    /// `minecraft:recipe_furnace`
    Furnace,
    /// `minecraft:recipe_furnace_material` (fuel entry; same shape as furnace).
    FurnaceMaterial,
    /// `minecraft:recipe_brewing_mix`
    BrewingMix,
    /// `minecraft:recipe_brewing_container`
    BrewingContainer,
    /// `minecraft:recipe_smithing_transform`
    SmithingTransform,
    /// `minecraft:recipe_smithing_trim`
    SmithingTrim,
    /// `minecraft:recipe_material_reduction` (chemistry material reducer).
    /// The historic alias `minecraft:recipe_material_reducer` is accepted.
    MaterialReducer,
}

impl RecipeKind {
    /// 解析 JSON 根键。未知键返回 `None`。
    ///
    /// Parse a JSON root key. Returns `None` for unknown keys.
    pub fn from_root_key(key: &str) -> Option<Self> {
        match key {
            "minecraft:recipe_shaped" => Some(Self::Shaped),
            "minecraft:recipe_shapeless" => Some(Self::Shapeless),
            "minecraft:recipe_furnace" => Some(Self::Furnace),
            "minecraft:recipe_furnace_material" => Some(Self::FurnaceMaterial),
            "minecraft:recipe_brewing_mix" => Some(Self::BrewingMix),
            "minecraft:recipe_brewing_container" => Some(Self::BrewingContainer),
            "minecraft:recipe_smithing_transform" => Some(Self::SmithingTransform),
            "minecraft:recipe_smithing_trim" => Some(Self::SmithingTrim),
            "minecraft:recipe_material_reduction" | "minecraft:recipe_material_reducer" => {
                Some(Self::MaterialReducer)
            }
            _ => None,
        }
    }

    /// 规范化的 JSON 根键。
    ///
    /// Canonical JSON root key.
    pub fn root_key(self) -> &'static str {
        match self {
            Self::Shaped => "minecraft:recipe_shaped",
            Self::Shapeless => "minecraft:recipe_shapeless",
            Self::Furnace => "minecraft:recipe_furnace",
            Self::FurnaceMaterial => "minecraft:recipe_furnace_material",
            Self::BrewingMix => "minecraft:recipe_brewing_mix",
            Self::BrewingContainer => "minecraft:recipe_brewing_container",
            Self::SmithingTransform => "minecraft:recipe_smithing_transform",
            Self::SmithingTrim => "minecraft:recipe_smithing_trim",
            Self::MaterialReducer => "minecraft:recipe_material_reduction",
        }
    }

    /// 即时（网格）配方在一次事务内求解；其余为
    ///
    /// Instant (grid) recipes resolve in one transaction; the rest are
    /// duration-based block-entity processes and must not be treated as
    /// instant crafting.
    pub fn is_instant(self) -> bool {
        matches!(self, Self::Shaped | Self::Shapeless)
    }

    /// 基于时长的过程配方（熔炉 / 酿造 / 锻造 /
    ///
    /// Duration-based process recipes (furnace / brewing / smithing /
    /// material reducer). They compile to the unified model but execute
    /// through `BlockEntity` processing state, not instant crafting.
    pub fn is_process(self) -> bool {
        !self.is_instant()
    }
}

/// 由配方 `tags` 数组推导的合成场景。
///
/// 标签只描述工作台条件，不会改变配方类型。
///
/// Crafting context derived from the recipe `tags` array.
///
/// Tags are workstation conditions only; they never change the recipe kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum StationKind {
    CraftingTable,
    /// Player-inventory 2x2 crafting (no station block). Never parsed from
    /// recipe tags; attached at compile time to grid recipes that fit 2x2.
    InventoryCrafting,
    Stonecutter,
    Furnace,
    BlastFurnace,
    Smoker,
    Campfire,
    SoulCampfire,
    BrewingStand,
    SmithingTable,
    MaterialReducer,
    CartographyTable,
    Loom,
    Grindstone,
    Unknown,
}

impl Default for StationKind {
    fn default() -> Self {
        Self::Unknown
    }
}

impl StationKind {
    /// 解析 `tags` 数组的一个条目（或旧版 `tags` 字符串）。
    ///
    /// Parse one entry of the `tags` array (or the legacy `tags` string).
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "crafting_table" => Self::CraftingTable,
            "stonecutter" => Self::Stonecutter,
            "furnace" => Self::Furnace,
            "blast_furnace" => Self::BlastFurnace,
            "smoker" => Self::Smoker,
            "campfire" => Self::Campfire,
            "soul_campfire" => Self::SoulCampfire,
            "brewing_stand" => Self::BrewingStand,
            "smithing_table" => Self::SmithingTable,
            "material_reducer" => Self::MaterialReducer,
            "cartography_table" => Self::CartographyTable,
            "loom" => Self::Loom,
            "grindstone" => Self::Grindstone,
            _ => Self::Unknown,
        }
    }

    /// 规范化的标签字符串（`"crafting_table"` 等）。`Unknown` 映射为
    ///
    /// Canonical tag string (`"crafting_table"`, ...). `Unknown` maps to
    /// `"unknown"` and is only used for diagnostics.
    pub fn tag(self) -> &'static str {
        match self {
            Self::CraftingTable => "crafting_table",
            Self::InventoryCrafting => "inventory",
            Self::Stonecutter => "stonecutter",
            Self::Furnace => "furnace",
            Self::BlastFurnace => "blast_furnace",
            Self::Smoker => "smoker",
            Self::Campfire => "campfire",
            Self::SoulCampfire => "soul_campfire",
            Self::BrewingStand => "brewing_stand",
            Self::SmithingTable => "smithing_table",
            Self::MaterialReducer => "material_reducer",
            Self::CartographyTable => "cartography_table",
            Self::Loom => "loom",
            Self::Grindstone => "grindstone",
            Self::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_keys_round_trip() {
        for (key, kind) in [
            ("minecraft:recipe_shaped", RecipeKind::Shaped),
            ("minecraft:recipe_shapeless", RecipeKind::Shapeless),
            ("minecraft:recipe_furnace", RecipeKind::Furnace),
            (
                "minecraft:recipe_furnace_material",
                RecipeKind::FurnaceMaterial,
            ),
            ("minecraft:recipe_brewing_mix", RecipeKind::BrewingMix),
            (
                "minecraft:recipe_brewing_container",
                RecipeKind::BrewingContainer,
            ),
            (
                "minecraft:recipe_smithing_transform",
                RecipeKind::SmithingTransform,
            ),
            ("minecraft:recipe_smithing_trim", RecipeKind::SmithingTrim),
            (
                "minecraft:recipe_material_reduction",
                RecipeKind::MaterialReducer,
            ),
            (
                "minecraft:recipe_material_reducer",
                RecipeKind::MaterialReducer,
            ),
        ] {
            assert_eq!(RecipeKind::from_root_key(key), Some(kind));
            if key != "minecraft:recipe_material_reducer" {
                assert_eq!(kind.root_key(), key);
            }
        }
        assert_eq!(RecipeKind::from_root_key("minecraft:nope"), None);
    }

    #[test]
    fn station_and_kind_are_independent() {
        // A shapeless recipe with a stonecutter tag is still shapeless;
        // the tag only selects the station.
        assert!(RecipeKind::Shapeless.is_instant());
        assert_eq!(
            StationKind::from_tag("stonecutter"),
            StationKind::Stonecutter
        );
        assert_eq!(StationKind::from_tag("nope"), StationKind::Unknown);
    }
}
