//! Block static components: ECS Component structs (follows `item::component`).
//!
//! Component names use vanilla `minecraft:*` names where they exist; server-only
//! concepts keep the `sc:` namespace:
//!
//! | This component | Vanilla counterpart | Notes |
//! |---|---|---|
//! | `CollisionBoxes` | `minecraft:collision_box` | Single box `{origin, size}` (16ths) or box array; no collision = `enabled: false` |
//! | `LightEmission` | `minecraft:light_emission` | Integer 0..15 |
//! | `LightDampening` | `minecraft:light_dampening` | Integer 0..15 |
//! | `DestructibleByMining` | `minecraft:destructible_by_mining` | `{"value": seconds}`; for indestructible see `sc:unbreakable` |
//! | `Replaceable` | `sc:replaceable` | Server-only flag: replaceable boolean |
//! | `Liquid` | `sc:liquid` | Server-only flag: liquid kind for non-customizable liquid blocks |
//! | `RandomTick` | `sc:random_tick` | Server-only flag: random-tick candidate filtering |
//! | `NeedsSupport` | `sc:needs_support` | Server-only flag: data reference for support conditions |
//! | `CanContainLiquid` | `sc:can_contain_liquid` | Server-only flag: whether a liquid layer is allowed |
//! | `Unbreakable` | `sc:unbreakable` | Server-only flag: mutually exclusive with mining |
//! | `Mining` | `sc:mining` | Mining speed/harvest/efficiency adjustment (`formula_version=1` exact formula) |
//!
//! Loot-related components (`minecraft:loot` path reference, `sc:drops` direct
//! drops and their fortune/count satellites, plus the parsed `sc:loot_tables`)
//! live in the separate [`crate::loot_tables`] domain; they are registered in
//! the [`crate::block::BlockComponents`] `components_export!` list like every
//! other component.
//!
//! Client-side entries (`display_name`, `geometry`, `material_instances`, ...)
//! are not registered: at parse time they are recognized and dropped per
//! [`IGNORED_COMPONENTS`], never entering snapshots.
//!
//! Each component is a standalone struct deriving [`sc_ecs::component::Component`]
//! + `Deserialize`; there is no load-time range/shape validation — malformed
//! values surface where they are consumed (compile or gameplay), keeping the
//! load path a direct load. Registered keys and typed reads come from the
//! `components_export!` enum ([`crate::block::BlockComponents`]).

use sc_ecs::component::Component;
use serde::Deserialize;
use serde_json::Value;



/// Collision box (16ths origin + size; `enabled: false` = no collision).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CollisionBox {
    #[serde(default = "default_collision_enabled")]
    pub enabled: bool,
    pub origin: [f32; 3],
    pub size: [f32; 3],
}

fn default_collision_enabled() -> bool {
    true
}

/// The two vanilla `minecraft:collision_box` spellings: single box object, or box array.
///
/// Component identity (`Component` + registry entry) is this enum;
/// [`collision_boxes_of`] unwraps both spellings into a box list.
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(untagged)]
pub enum CollisionBoxes {
    One(CollisionBox),
    Many(Vec<CollisionBox>),
}

/// Both vanilla spellings to box list (`enabled` kept per box).
pub fn collision_boxes_of(value: &Value) -> Result<Vec<CollisionBox>, String> {
    serde_json::from_value::<CollisionBoxes>(value.clone())
        .map(|b| match b {
            CollisionBoxes::One(b) => vec![b],
            CollisionBoxes::Many(v) => v,
        })
        .map_err(|e| format!("collision_box 形状非法：{e}"))
}

/// Light emission (0..15).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct LightEmission(pub u8);

/// Light dampening (0..15).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct LightDampening(pub u8);

/// Mineable seconds (for break progress; indestructible see `sc:unbreakable`).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DestructibleByMining {
    pub value: f32,
}

/// Replaceable boolean (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Replaceable(pub bool);

/// Liquid kind (vanilla liquid blocks are not customizable).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Liquid {
    pub kind: String,
}

/// Whether it participates in random-tick candidate filtering (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RandomTick(pub bool);

/// Data reference for support conditions (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct NeedsSupport {
    pub condition: String,
}

/// Whether a liquid layer is allowed (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanContainLiquid(pub bool);

/// Unbreakable (no vanilla custom-block equivalent; mutually exclusive with mining,
/// checked by `sc_block` on the merged state).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Unbreakable {}

/// Workstation slot role (server container addressing). `Extra` is
/// reserved for 4+-slot processes (campfire); no shipped decl uses it yet.
#[derive(Clone, Deserialize, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum WorkstationSlotRole {
    Input,
    Fuel,
    Result,
    Extra,
}

/// One workstation slot: gameplay role plus Bedrock container enum byte
/// (the values the client uses in Take/Place, e.g. furnace 25/24/26).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct WorkstationSlot {
    pub role: WorkstationSlotRole,
    pub container: u8,
}

/// Workstation window declaration: which recipe station a block serves,
/// which container window opens, and how slots address on the wire.
/// `grid` is the NxN crafting grid size when present (workbench 3).
/// `window` marks the five player-openable stations (table, cutter, three
/// furnaces); brewing/smithing/campfire decls exist for addressing but do
/// not open (the open gate reads this bit, never a station enum).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Workstation {
    pub station: String,
    pub container: i8,
    #[serde(default)]
    pub window: bool,
    #[serde(default)]
    pub grid: Option<u8>,
    #[serde(default)]
    pub slots: Vec<WorkstationSlot>,
}

/// Duration-process kind (furnace family shares one kind with params).
#[derive(Clone, Deserialize, Debug, PartialEq, Eq)]
pub enum ProcessKind {
    #[serde(rename = "burn_cook")]
    BurnCook,
}

/// Duration-process declaration: burn/cook process attached to a block.
/// `lit_variant` names the swap target block (facing preserved); absent
/// means no lit swap. `lit` marks the lit member of a lit pair, so the
/// tick only swaps toward the burn state (unlit+burning→lit,
/// lit+!burning→unlit) instead of following pointers blindly.
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Process {
    pub kind: ProcessKind,
    pub cook_ticks: u16,
    #[serde(default)]
    pub fuel: bool,
    #[serde(default)]
    pub lit: bool,
    #[serde(default)]
    pub lit_variant: Option<String>,
}

// ---------------------------------------------------------------------------
// sc:mining (mining speed / harvest / efficiency; formula_version=1 exact formula)
// ---------------------------------------------------------------------------

/// Allowed mining enchants (allowlist for `sc:mining.enchantments` keys).
pub const ALLOWED_MINING_ENCHANTS: &[&str] = &["minecraft:efficiency"];
/// Efficiency `max_level` range.
pub const EFFICIENCY_MAX_LEVEL_MIN: u32 = 1;
pub const EFFICIENCY_MAX_LEVEL_MAX: u32 = 255;
/// Default harvest penalty multiplier (unharvested-branch `hardness*5` factor).
pub const DEFAULT_HARVEST_PENALTY: f64 = 5.0;
/// Harvested-branch time factor (`hardness*1.5`; pairs with the penalty term).
pub const HARVESTED_BRANCH_FACTOR: f64 = 1.5;

/// One rule for `sc:mining.default` / `tools[]` (no inheritance; every rule is complete).
///
/// - `can_mine=false`: survival cannot break (creative bypasses); `speed_multiplier` is the
///   `1.0` placeholder and `harvest` must be `false` (no break means no harvest);
/// - `harvest=false`: slow break is allowed, but time is scaled by the harvest penalty and drops
///   fall back to the default empty drop (unharvested branch: `hardness*5` time + empty drops).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MiningRule {
    pub can_mine: bool,
    pub harvest: bool,
    pub speed_multiplier: f64,
}

/// One entry of `sc:mining.tools[]` (item list + rule).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MiningToolEntry {
    pub items: Vec<String>,
    pub can_mine: bool,
    pub harvest: bool,
    pub speed_multiplier: f64,
}

/// `sc:mining.enchantments["minecraft:efficiency"]`.
///
/// Bonus formula is the exact break-time formula (`speed += level^2 + 1`);
/// data only declares the `max_level` clamp.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EfficiencyRule {
    pub max_level: u32,
}

/// `sc:mining` (`formula_version=1`: exact break-time formula).
///
/// Time formula: `time = base * branch / (tool_speed + efficiency_bonus)`, where
/// `base` is the raw hardness value, `branch` is `1.5` when harvested and
/// `harvest_penalty_multiplier` when not (defaults to `5.0`).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Mining {
    pub formula_version: u32,
    #[serde(default)]
    pub base_time_seconds: Option<f64>,
    /// Harvest penalty multiplier (rule time factor `base*penalty` for `harvest=false`; defaults to `5.0`).
    #[serde(default)]
    pub harvest_penalty_multiplier: Option<f64>,
    pub default: MiningRule,
    #[serde(default)]
    pub tools: Vec<MiningToolEntry>,
    #[serde(default)]
    pub enchantments: std::collections::BTreeMap<String, EfficiencyRule>,
}

/// Client-side/future ignore list: legal vanilla file content the server currently does not consume.
/// Dropped at parse time (shapes vary: string/object/array), never enters snapshots.
pub const IGNORED_COMPONENTS: &[&str] = &[
    "minecraft:selection_box",
    "minecraft:geometry",
    "minecraft:material_instances",
    "minecraft:display_name",
    "minecraft:menu_category",
    "minecraft:transformation",
    "minecraft:crafting_table",
    "minecraft:placement_filter",
    "minecraft:custom_components",
    "minecraft:tick_queue",
];

/// Whether the name is an accepted-but-ignored client-side/future component.
pub fn is_ignored_component(name: &str) -> bool {
    IGNORED_COMPONENTS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macros::ComponentKey;
    use crate::block::BlockComponents;
    use crate::loot_tables::{Drops, LootTables};
    use serde_json::json;

    #[test]
    fn registry_covers_exactly_the_fixed_set() {
        let mut names = BlockComponents::NAMES.to_vec();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "minecraft:collision_box",
                "minecraft:destructible_by_mining",
                "minecraft:light_dampening",
                "minecraft:light_emission",
                "minecraft:loot",
                "sc:can_contain_liquid",
                "sc:drops",
                "sc:liquid",
                "sc:loot_tables",
                "sc:mining",
                "sc:needs_support",
                "sc:process",
                "sc:random_tick",
                "sc:replaceable",
                "sc:unbreakable",
                "sc:workstation",
            ]
        );
        for name in BlockComponents::NAMES {
            assert!(BlockComponents::is_known(name));
        }
        assert!(!BlockComponents::is_known("minecraft:fly"));
        assert!(is_ignored_component("minecraft:geometry"));
        assert!(is_ignored_component("minecraft:menu_category"));
        assert!(!is_ignored_component("minecraft:collision_box"));
        // Listed component types carry their registered key as an inherent const.
        assert_eq!(LightEmission::NAME, "minecraft:light_emission");
        assert_eq!(Drops::NAME, "sc:drops");
        assert_eq!(LootTables::NAME, "sc:loot_tables");
    }

    #[test]
    fn typed_getter_reads_raw_maps() {
        let mut map = std::collections::BTreeMap::new();
        map.insert("minecraft:light_emission".to_string(), json!(3));
        let light: Option<LightEmission> = BlockComponents::get_from(&map);
        assert_eq!(light.map(|l| l.0), Some(3));
        let missing: Option<DestructibleByMining> = BlockComponents::get_from(&map);
        assert!(missing.is_none());
    }

    #[test]
    fn collision_boxes_unwraps_both_spellings() {
        let single = collision_boxes_of(&json!({"origin": [-8, 0, -8], "size": [16, 16, 16]}))
            .expect("单 box 应合法");
        assert_eq!(single.len(), 1);
        let many = collision_boxes_of(&json!([
            {"origin": [-8, 0, -8], "size": [16, 4, 16]},
            {"origin": [-8, 4, -8], "size": [16, 4, 16]}
        ]))
        .expect("box 数组应合法");
        assert_eq!(many.len(), 2);
        assert!(collision_boxes_of(&json!({"origin": [-8, 0, -8]})).is_err());
    }

    #[test]
    fn workstation_and_process_shapes() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            Workstation::NAME.to_string(),
            json!({
                "station": "furnace",
                "container": 2,
                "window": true,
                "slots": [
                    {"role": "input", "container": 25},
                    {"role": "fuel", "container": 24},
                    {"role": "result", "container": 26}
                ]
            }),
        );
        map.insert(
            Process::NAME.to_string(),
            json!({
                "kind": "burn_cook",
                "cook_ticks": 200,
                "fuel": true,
                "lit_variant": "minecraft:lit_furnace"
            }),
        );
        let decl: Workstation = BlockComponents::get_from(&map).expect("应读出");
        assert_eq!(decl.station, "furnace");
        assert!(decl.window);
        assert_eq!(decl.slots.len(), 3);

        let decl: Process = BlockComponents::get_from(&map).expect("应读出");
        assert!(!decl.lit, "lit 缺省为未点燃");
        assert_eq!(decl.lit_variant.as_deref(), Some("minecraft:lit_furnace"));
    }
}
