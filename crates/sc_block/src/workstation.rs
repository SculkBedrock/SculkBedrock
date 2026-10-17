//! Workstation/process declarations compiled from `sc:workstation` /
//! `sc:process` block components into per-state dense columns.
//!
//! Parse shapes live in `sc_packloader::block::component`; this module owns
//! the compiled game-facing form plus per-state compilation (structural
//! checks, budgets). Station-set membership (`StationKind`) and lit-variant
//! existence resolve in `sc_game` at snapshot validation (see
//! `validate_workstations`), keeping `sc_block` free of recipe-domain
//! knowledge.

use std::collections::BTreeMap;

use sc_packloader::macros::ComponentKey;
use sc_packloader::block::{BlockBundleBudgets, BlockJsonError};

/// Workstation slot role (server container addressing). `Extra` is
/// reserved for 4+-slot processes (campfire); no shipped decl uses it yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SlotRole {
    Input,
    Fuel,
    Result,
    Extra,
}

/// One workstation slot: gameplay role plus Bedrock container enum byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkstationSlotDecl {
    pub role: SlotRole,
    pub container: u8,
}

/// Compiled `sc:workstation`: which recipe station a block state serves
/// (`station` stays a raw tag; `sc_game` resolves it to `StationKind`),
/// which container window opens, and how slots address on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkstationDecl {
    pub station: Box<str>,
    pub container: i8,
    /// Player-openable window (the five stations; others address but not open).
    pub window: bool,
    /// NxN crafting grid size when present (workbench 3).
    pub grid: Option<u8>,
    pub slots: Vec<WorkstationSlotDecl>,
}

/// Duration-process kind (furnace family shares one kind with params).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessKind {
    BurnCook,
}

/// Compiled `sc:process`: burn/cook process attached to a block state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessDecl {
    pub kind: ProcessKind,
    pub cook_ticks: u16,
    pub fuel: bool,
    /// Lit member of a lit pair (swap direction guard).
    pub lit: bool,
    /// Swap target block on lit edges (facing preserved); `None` = no swap.
    pub lit_variant: Option<Box<str>>,
}

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

/// Compile one state's `sc:workstation` (`None` = undeclared).
/// Structural checks plus budgets; station-set membership resolves later
/// (see `validate_workstations` in `sc_game`).
pub fn compile_workstation(
    merged: &BTreeMap<String, serde_json::Value>,
    pack_id: &str,
    file: &str,
    key: &str,
    budgets: &BlockBundleBudgets,
) -> Result<Option<WorkstationDecl>, BlockJsonError> {
    use sc_packloader::block::component::Workstation as Schema;
    let field = |name: &str| format!("$.minecraft:block.components.{name}（状态 {key:?}）");
    let bad = |detail: String| BlockJsonError::new(pack_id, file, field(Schema::NAME), detail);
    let Some(raw) = merged.get(Schema::NAME) else {
        return Ok(None);
    };
    let parsed: Schema = serde_json::from_value(raw.clone())
        .map_err(|e| bad(format!("内部不一致：{e}")))?;
    if parsed.slots.len() > budgets.max_workstation_slots_per_state {
        return Err(bad(format!(
            "slots {} 项超出预算 {}",
            parsed.slots.len(),
            budgets.max_workstation_slots_per_state
        )));
    }
    Ok(Some(WorkstationDecl {
        station: parsed.station.into_boxed_str(),
        container: parsed.container,
        window: parsed.window,
        grid: parsed.grid,
        slots: parsed
            .slots
            .iter()
            .map(|slot| WorkstationSlotDecl {
                role: slot_role_of(&slot.role),
                container: slot.container,
            })
            .collect(),
    }))
}

/// Compile one state's `sc:process` (`None` = undeclared).
/// A fuel process requires a fuel slot; the workstation column carries it,
/// so callers pass the already-compiled workstation decl (if any).
pub fn compile_process(
    merged: &BTreeMap<String, serde_json::Value>,
    workstation: Option<&WorkstationDecl>,
    pack_id: &str,
    file: &str,
    key: &str,
) -> Result<Option<ProcessDecl>, BlockJsonError> {
    use sc_packloader::block::component::{Process as Schema, ProcessKind as SchemaKind};
    let field = |name: &str| format!("$.minecraft:block.components.{name}（状态 {key:?}）");
    let bad = |detail: String| BlockJsonError::new(pack_id, file, field(Schema::NAME), detail);
    let Some(raw) = merged.get(Schema::NAME) else {
        return Ok(None);
    };
    let parsed: Schema = serde_json::from_value(raw.clone())
        .map_err(|e| bad(format!("内部不一致：{e}")))?;
    let kind = match parsed.kind {
        SchemaKind::BurnCook => ProcessKind::BurnCook,
    };
    if parsed.fuel
        && !workstation.is_some_and(|decl| {
            decl.slots
                .iter()
                .any(|slot| slot.role == SlotRole::Fuel)
        })
    {
        return Err(bad(
            "fuel 进程需要 workstation 声明含 fuel 槽".to_string(),
        ));
    }
    Ok(Some(ProcessDecl {
        kind,
        cook_ticks: parsed.cook_ticks,
        fuel: parsed.fuel,
        lit: parsed.lit,
        lit_variant: parsed.lit_variant.map(String::into_boxed_str),
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

    fn merged(value: serde_json::Value) -> BTreeMap<String, serde_json::Value> {
        BTreeMap::from([("sc:workstation".to_string(), value)])
    }

    #[test]
    fn workstation_absent_compiles_to_none() {
        let empty = BTreeMap::new();
        assert!(compile_workstation(&empty, "p", "f", "k", &budgets())
            .expect("无声明应成功")
            .is_none());
        assert!(compile_process(&empty, None, "p", "f", "k")
            .expect("无声明应成功")
            .is_none());
    }

    #[test]
    fn workstation_compiles_slots_and_grid() {
        let decl = compile_workstation(
            &merged(json!({
                "station": "furnace",
                "container": 2,
                "slots": [
                    {"role": "input", "container": 25},
                    {"role": "fuel", "container": 24},
                    {"role": "result", "container": 26}
                ]
            })),
            "p",
            "f",
            "k",
            &budgets(),
        )
        .expect("应编译")
        .expect("应有声明");
        assert_eq!(decl.station.as_ref(), "furnace");
        assert_eq!(decl.container, 2);
        assert_eq!(decl.slots.len(), 3);
        // Slot budget enforced.
        let mut tight = budgets();
        tight.max_workstation_slots_per_state = 2;
        assert!(compile_workstation(
            &merged(json!({
                "station": "furnace",
                "container": 2,
                "slots": [
                    {"role": "input", "container": 25},
                    {"role": "fuel", "container": 24},
                    {"role": "result", "container": 26}
                ]
            })),
            "p",
            "f",
            "k",
            &tight,
        )
        .is_err());
    }

    #[test]
    fn process_requires_fuel_slot_when_fueled() {
        let process = json!({
            "kind": "burn_cook",
            "cook_ticks": 200,
            "fuel": true,
            "lit_variant": "minecraft:lit_furnace"
        });
        let map = BTreeMap::from([("sc:process".to_string(), process)]);
        // No workstation at all: fuel process rejected.
        assert!(compile_process(&map, None, "p", "f", "k").is_err());
        let station = compile_workstation(
            &merged(json!({
                "station": "furnace",
                "container": 2,
                "slots": [{"role": "input", "container": 25}]
            })),
            "p",
            "f",
            "k",
            &budgets(),
        )
        .expect("应编译")
        .expect("应有声明");
        // Workstation without a fuel slot: still rejected.
        assert!(compile_process(&map, Some(&station), "p", "f", "k").is_err());
    }

    #[test]
    fn process_without_fuel_needs_no_slots() {
        let map = BTreeMap::from([(
            "sc:process".to_string(),
            json!({"kind": "burn_cook", "cook_ticks": 100}),
        )]);
        let decl = compile_process(&map, None, "p", "f", "k")
            .expect("应编译")
            .expect("应有声明");
        assert_eq!(decl.cook_ticks, 100);
        assert!(!decl.fuel);
        assert!(decl.lit_variant.is_none());
    }
}
