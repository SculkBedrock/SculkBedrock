//! 交互式容器/处理方块声明（箱子、熔炉、工作台），由方块组件编译为逐状态稠密列。
//!
//! Interactive container/processing block declarations (chest, furnace, crafting_table)
//! compiled from block components into per-state dense columns.

use std::collections::BTreeMap;

use sc_packloader::macros::ComponentKey;
use sc_packloader::block::{BlockBundleBudgets, BlockJsonError};

/// 容器/处理槽位的槽位角色。
///
/// Slot role for container/processing slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SlotRole {
    Input,
    Fuel,
    Result,
    Extra,
}

/// 单个槽位声明：玩法角色 + Bedrock 容器枚举字节。
///
/// One slot declaration: gameplay role plus Bedrock container enum byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkstationSlotDecl {
    pub role: SlotRole,
    pub container: u8,
}

/// 把包加载器 schema 的槽位角色映射为本 crate 的角色枚举。
///
/// Maps the pack-loader schema slot role onto this crate's role enum.
///
/// - `role`: schema 侧槽位角色
fn slot_role_of(
    role: &sc_packloader::block::component::WorkstationSlotRole,
) -> SlotRole {
    use sc_packloader::block::component::WorkstationSlotRole as Schema;
    match role {
        Schema::Input => SlotRole::Input,
        Schema::Fuel => SlotRole::Fuel,
        Schema::Result => SlotRole::Result,
        Schema::Extra => SlotRole::Extra,
    }
}

/// 编译产物 `sc:chest`：存储容器声明。
///
/// Compiled `sc:chest`: storage container declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChestDecl {
    pub slots: u16,
    pub container_type: i8,
    pub pairable: bool,
    pub check_obstruction: bool,
    pub open_sound: Box<str>,
    pub close_sound: Box<str>,
}

/// 编译产物 `sc:furnace`：时长处理声明。
///
/// Compiled `sc:furnace`: duration processing declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FurnaceDecl {
    pub kind: Box<str>,
    pub cook_ticks: u16,
    pub fuel: bool,
    pub lit: bool,
    pub lit_variant: Option<Box<str>>,
    pub container_type: i8,
    pub slots: Vec<WorkstationSlotDecl>,
}

/// 编译产物 `sc:crafting_table`：即时网格合成台声明。
///
/// Compiled `sc:crafting_table`: instant grid crafting table declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftingTableDecl {
    pub grid: u8,
    pub station: Box<str>,
    pub container_type: i8,
}

/// 编译单个状态的 `sc:chest`（`None` = 未声明）。
///
/// Compile one state's `sc:chest` (`None` = undeclared).
///
/// - `merged`: 合并后的组件 JSON 映射
/// - `pack_id` / `file` / `key`: 出错时定位用的包 id、文件名与状态键
/// - `budgets`: 方块 bundle 预算（限制单状态槽位数）
pub fn compile_chest(
    merged: &BTreeMap<String, serde_json::Value>,
    pack_id: &str,
    file: &str,
    key: &str,
    budgets: &BlockBundleBudgets,
) -> Result<Option<ChestDecl>, BlockJsonError> {
    use sc_packloader::block::component::Chest as Schema;
    let field = |name: &str| format!("$.minecraft:block.components.{name}（状态 {key:?}）");
    let bad = |detail: String| BlockJsonError::new(pack_id, file, field(Schema::NAME), detail);
    let Some(raw) = merged.get(Schema::NAME) else {
        return Ok(None);
    };
    let parsed: Schema = serde_json::from_value(raw.clone())
        .map_err(|e| bad(format!("内部不一致：{e}")))?;
    if parsed.slots as usize > budgets.max_chest_slots_per_state {
        return Err(bad(format!(
            "slots {} 超出预算 {}",
            parsed.slots,
            budgets.max_chest_slots_per_state
        )));
    }
    Ok(Some(ChestDecl {
        slots: parsed.slots,
        container_type: parsed.container_type,
        pairable: parsed.pairable,
        check_obstruction: parsed.check_obstruction,
        open_sound: parsed.open_sound.into_boxed_str(),
        close_sound: parsed.close_sound.into_boxed_str(),
    }))
}

/// 编译单个状态的 `sc:furnace`（`None` = 未声明）。
///
/// Compile one state's `sc:furnace` (`None` = undeclared).
///
/// fuel 熔炉必须声明至少一个 fuel 槽，否则报错。
///
/// Errors: a fuel furnace must declare at least one fuel slot.
///
/// - `merged`: 合并后的组件 JSON 映射
/// - `pack_id` / `file` / `key`: 出错时定位用的包 id、文件名与状态键
/// - `budgets`: 方块 bundle 预算（限制单状态槽位数）
pub fn compile_furnace(
    merged: &BTreeMap<String, serde_json::Value>,
    pack_id: &str,
    file: &str,
    key: &str,
    budgets: &BlockBundleBudgets,
) -> Result<Option<FurnaceDecl>, BlockJsonError> {
    use sc_packloader::block::component::Furnace as Schema;
    let field = |name: &str| format!("$.minecraft:block.components.{name}（状态 {key:?}）");
    let bad = |detail: String| BlockJsonError::new(pack_id, file, field(Schema::NAME), detail);
    let Some(raw) = merged.get(Schema::NAME) else {
        return Ok(None);
    };
    let parsed: Schema = serde_json::from_value(raw.clone())
        .map_err(|e| bad(format!("内部不一致：{e}")))?;
    if parsed.slots.len() > budgets.max_furnace_slots_per_state {
        return Err(bad(format!(
            "slots {} 项超出预算 {}",
            parsed.slots.len(),
            budgets.max_furnace_slots_per_state
        )));
    }
    let slots: Vec<WorkstationSlotDecl> = parsed
        .slots
        .iter()
        .map(|slot| WorkstationSlotDecl {
            role: slot_role_of(&slot.role),
            container: slot.container,
        })
        .collect();
    if parsed.fuel && !slots.iter().any(|slot| slot.role == SlotRole::Fuel) {
        return Err(bad("fuel 熔炉必须包含 fuel 槽".to_string()));
    }
    Ok(Some(FurnaceDecl {
        kind: parsed.kind.into_boxed_str(),
        cook_ticks: parsed.cook_ticks,
        fuel: parsed.fuel,
        lit: parsed.lit,
        lit_variant: parsed.lit_variant.map(String::into_boxed_str),
        container_type: parsed.container_type,
        slots,
    }))
}

/// 编译单个状态的 `sc:crafting_table`（`None` = 未声明）。
///
/// Compile one state's `sc:crafting_table` (`None` = undeclared).
///
/// - `merged`: 合并后的组件 JSON 映射
/// - `pack_id` / `file` / `key`: 出错时定位用的包 id、文件名与状态键
pub fn compile_crafting_table(
    merged: &BTreeMap<String, serde_json::Value>,
    pack_id: &str,
    file: &str,
    key: &str,
) -> Result<Option<CraftingTableDecl>, BlockJsonError> {
    use sc_packloader::block::component::CraftingTable as Schema;
    let field = |name: &str| format!("$.minecraft:block.components.{name}（状态 {key:?}）");
    let bad = |detail: String| BlockJsonError::new(pack_id, file, field(Schema::NAME), detail);
    let Some(raw) = merged.get(Schema::NAME) else {
        return Ok(None);
    };
    let parsed: Schema = serde_json::from_value(raw.clone())
        .map_err(|e| bad(format!("内部不一致：{e}")))?;
    Ok(Some(CraftingTableDecl {
        grid: parsed.grid,
        station: parsed.station.into_boxed_str(),
        container_type: parsed.container_type,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_packloader::block::BlockBundleBudgets;
    use serde_json::json;

    fn budgets() -> BlockBundleBudgets {
        BlockBundleBudgets::default()
    }

    #[test]
    fn chest_compiles_defaults_and_budgets() {
        let mut map = BTreeMap::new();
        map.insert(
            "sc:chest".to_string(),
            json!({
                "slots": 27,
                "pairable": true
            }),
        );
        let decl = compile_chest(&map, "p", "f", "k", &budgets())
            .expect("应编译")
            .expect("应存在");
        assert_eq!(decl.slots, 27);
        assert_eq!(decl.container_type, 0);
        assert!(decl.pairable);
        assert!(decl.check_obstruction);
        assert_eq!(decl.open_sound.as_ref(), "random.chestopen");

        let mut tight = budgets();
        tight.max_chest_slots_per_state = 10;
        assert!(compile_chest(&map, "p", "f", "k", &tight).is_err());
    }

    #[test]
    fn chest_switches_pairable_and_obstruction_can_be_toggled_independently() {
        // Barrel case: pairable=false, check_obstruction=false
        let mut map_barrel = BTreeMap::new();
        map_barrel.insert(
            "sc:chest".to_string(),
            json!({
                "slots": 27,
                "pairable": false,
                "check_obstruction": false,
                "open_sound": "block.barrel.open",
                "close_sound": "block.barrel.close"
            }),
        );
        let barrel_decl = compile_chest(&map_barrel, "p", "f", "k", &budgets())
            .expect("应编译")
            .expect("应存在");
        assert!(!barrel_decl.pairable);
        assert!(!barrel_decl.check_obstruction);
        assert_eq!(barrel_decl.open_sound.as_ref(), "block.barrel.open");

        // Custom case: pairable=false, check_obstruction=true
        let mut map_unpairable_chest = BTreeMap::new();
        map_unpairable_chest.insert(
            "sc:chest".to_string(),
            json!({
                "slots": 54,
                "pairable": false,
                "check_obstruction": true
            }),
        );
        let unpairable_decl = compile_chest(&map_unpairable_chest, "p", "f", "k", &budgets())
            .expect("应编译")
            .expect("应存在");
        assert_eq!(unpairable_decl.slots, 54);
        assert!(!unpairable_decl.pairable);
        assert!(unpairable_decl.check_obstruction);
    }

    #[test]
    fn furnace_compiles_defaults_and_checks_fuel() {
        let mut map = BTreeMap::new();
        map.insert(
            "sc:furnace".to_string(),
            json!({
                "kind": "blast_furnace",
                "cook_ticks": 100,
                "fuel": true,
                "lit_variant": "minecraft:lit_blast_furnace"
            }),
        );
        let decl = compile_furnace(&map, "p", "f", "k", &budgets())
            .expect("应编译")
            .expect("应存在");
        assert_eq!(decl.kind.as_ref(), "blast_furnace");
        assert_eq!(decl.cook_ticks, 100);
        assert_eq!(decl.container_type, 2);
        assert_eq!(decl.slots.len(), 3);
        assert_eq!(decl.lit_variant.as_deref(), Some("minecraft:lit_blast_furnace"));
    }

    #[test]
    fn crafting_table_compiles_defaults() {
        let mut map = BTreeMap::new();
        map.insert(
            "sc:crafting_table".to_string(),
            json!({
                "grid": 3,
                "station": "crafting_table"
            }),
        );
        let decl = compile_crafting_table(&map, "p", "f", "k")
            .expect("应编译")
            .expect("应存在");
        assert_eq!(decl.grid, 3);
        assert_eq!(decl.station.as_ref(), "crafting_table");
        assert_eq!(decl.container_type, 1);
    }
}
