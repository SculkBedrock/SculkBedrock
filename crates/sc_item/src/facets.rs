//! 物品常用 NBT 切片定义。
//!
//! Common item NBT facet definitions.

use sc_nbt::NbtFacet;

/// 耐久切片（直接平铺在根 NBT）。
#[derive(Clone, Debug, PartialEq, Eq, NbtFacet)]
pub struct DamageFacet {
    #[nbt(rename = "Damage")]
    pub damage: i32,
}

/// 展示切片（嵌套在 display 子标签中）。
#[derive(Clone, Debug, Default, PartialEq, Eq, NbtFacet)]
#[nbt(tag = "display")]
pub struct DisplayFacet {
    #[nbt(rename = "Name")]
    pub name: Option<String>,
    #[nbt(rename = "Lore")]
    pub lore: Vec<String>,
}

/// 修补惩罚切片。
#[derive(Clone, Debug, PartialEq, Eq, NbtFacet)]
pub struct RepairCostFacet {
    #[nbt(rename = "RepairCost")]
    pub cost: i32,
}
