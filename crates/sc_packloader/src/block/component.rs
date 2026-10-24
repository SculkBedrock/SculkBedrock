//! 方块静态组件：ECS Component 结构体（遵循 `item::component`）。
//!
//! Block static components: ECS Component structs (follows `item::component`).
//!
//! 组件名在有原版对应时使用原版 `minecraft:*` 名；服务端专有概念保留
//! `sc:` 命名空间：
//!
//! Component names use vanilla `minecraft:*` names where they exist; server-only
//! concepts keep the `sc:` namespace:
//!
//! | 本组件 | 原版对应 | 说明 |
//! |---|---|---|
//! | `CollisionBoxes` | `minecraft:collision_box` | 单 box `{origin, size}`（1/16 单位）或 box 数组；无碰撞 = `enabled: false` |
//! | `LightEmission` | `minecraft:light_emission` | 整数 0..15 |
//! | `LightDampening` | `minecraft:light_dampening` | 整数 0..15 |
//! | `DestructibleByMining` | `minecraft:destructible_by_mining` | `{"value": 秒}`；不可破坏见 `sc:unbreakable` |
//! | `Replaceable` | `sc:replaceable` | 服务端标志：可替换布尔 |
//! | `Liquid` | `sc:liquid` | 服务端标志：不可自定义液体方块的液体种类 |
//! | `RandomTick` | `sc:random_tick` | 服务端标志：随机刻候选过滤 |
//! | `NeedsSupport` | `sc:needs_support` | 服务端标志：支撑条件数据引用 |
//! | `CanContainLiquid` | `sc:can_contain_liquid` | 服务端标志：是否允许液体层 |
//! | `Unbreakable` | `sc:unbreakable` | 服务端标志：与 mining 互斥 |
//! | `Mining` | `sc:mining` | 挖掘速度/收获/效率调整（`formula_version=1` 精确公式） |
//!
//! Loot-related components (`minecraft:loot` path reference with its parsed
//! table content carried on the bundle, see [`crate::loot_tables`]) live in
//! the separate [`crate::loot_tables`] domain; they are registered in
//! the [`crate::block::BlockComponents`] `components_export!` list like every
//! other component.
//!
//! 战利品相关组件（`minecraft:loot` 路径引用及其解析后的表内容随 bundle
//! 携带，见 [`crate::loot_tables`]）位于独立的 [`crate::loot_tables`] 域；
//! 它们与其他组件一样注册在 [`crate::block::BlockComponents`] 的
//! `components_export!` 列表中。
//!
//! Client-side entries (`display_name`, `geometry`, `material_instances`, ...)
//! are not registered: at parse time they are recognized and dropped per
//! [`IGNORED_COMPONENTS`], never entering snapshots.
//!
//! 客户端侧条目（`display_name`、`geometry`、`material_instances` 等）
//! 不注册：解析时按 [`IGNORED_COMPONENTS`] 识别并丢弃，绝不进入快照。
//!
//! Each component is a standalone struct deriving [`sc_ecs::component::Component`]
//! + `Deserialize`; there is no load-time range/shape validation — malformed
//! values surface where they are consumed (compile or gameplay), keeping the
//! load path a direct load. Registered keys and typed reads come from the
//! `components_export!` enum ([`crate::block::BlockComponents`]).
//!
//! 每个组件都是独立结构体，derive [`sc_ecs::component::Component`] +
//! `Deserialize`；加载期不做范围/形状校验——非法值在消费处
//! （编译或游玩）暴露，保持加载路径为直接加载。注册键与类型化读取
//! 来自 `components_export!` 枚举（[`crate::block::BlockComponents`]）。

use sc_ecs::component::Component;
use serde::Deserialize;
use serde_json::Value;



/// 碰撞箱（origin + size，1/16 单位；`enabled: false` = 无碰撞）。
///
/// Collision box (16ths origin + size; `enabled: false` = no collision).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CollisionBox {
    /// 是否启用碰撞。
    ///
    /// Whether collision is enabled.
    #[serde(default = "default_collision_enabled")]
    pub enabled: bool,
    /// 碰撞箱原点（1/16 单位）。
    ///
    /// Box origin (in 16ths).
    pub origin: [f32; 3],
    /// 碰撞箱尺寸（1/16 单位）。
    ///
    /// Box size (in 16ths).
    pub size: [f32; 3],
}

/// 碰撞启用缺省值。
///
/// Default value for collision enabled.
fn default_collision_enabled() -> bool {
    true
}

/// 两种原版 `minecraft:collision_box` 写法：单 box 对象或 box 数组。
///
/// The two vanilla `minecraft:collision_box` spellings: single box object, or box array.
///
/// 组件身份（`Component` + 注册条目）即本枚举；
/// [`collision_boxes_of`] 把两种写法展开为 box 列表。
///
/// Component identity (`Component` + registry entry) is this enum;
/// [`collision_boxes_of`] unwraps both spellings into a box list.
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(untagged)]
pub enum CollisionBoxes {
    One(CollisionBox),
    Many(Vec<CollisionBox>),
}

/// 两种原版写法 → box 列表（每盒保留 `enabled`）。
///
/// Both vanilla spellings to box list (`enabled` kept per box).
///
/// - `value`: 原始 JSON 值
///
/// # Errors
/// 形状非法时返回描述错误串。
pub fn collision_boxes_of(value: &Value) -> Result<Vec<CollisionBox>, String> {
    serde_json::from_value::<CollisionBoxes>(value.clone())
        .map(|b| match b {
            CollisionBoxes::One(b) => vec![b],
            CollisionBoxes::Many(v) => v,
        })
        .map_err(|e| format!("collision_box 形状非法：{e}"))
}

/// 发光等级（0..15）。
///
/// Light emission (0..15).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct LightEmission(pub u8);

/// 遮光等级（0..15）。
///
/// Light dampening (0..15).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct LightDampening(pub u8);

/// 可挖掘秒数（用于破坏进度；不可破坏见 `sc:unbreakable`）。
///
/// Mineable seconds (for break progress; indestructible see `sc:unbreakable`).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DestructibleByMining {
    /// 破坏耗时秒数。
    ///
    /// Seconds to break.
    pub value: f32,
}

/// 可替换标志（服务端专有概念）。
///
/// Replaceable boolean (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Replaceable(pub bool);

/// 液体种类（原版液体方块不可自定义）。
///
/// Liquid kind (vanilla liquid blocks are not customizable).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Liquid {
    /// 液体种类标识。
    ///
    /// Liquid kind identifier.
    pub kind: String,
}

/// 是否参与随机刻候选过滤（服务端专有概念）。
///
/// Whether it participates in random-tick candidate filtering (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RandomTick(pub bool);

/// 支撑条件的数据引用（服务端专有概念）。
///
/// Data reference for support conditions (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct NeedsSupport {
    /// 支撑条件表达式。
    ///
    /// Support condition expression.
    pub condition: String,
}

/// 是否允许液体层（服务端专有概念）。
///
/// Whether a liquid layer is allowed (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanContainLiquid(pub bool);

/// 不可破坏（无原版自定义方块对应；与 mining 互斥，
/// 由 `sc_block` 在合并状态上检查）。
///
/// Unbreakable (no vanilla custom-block equivalent; mutually exclusive with mining,
/// checked by `sc_block` on the merged state).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Unbreakable {}

/// 工作台/熔炉槽位角色（服务端容器寻址）。`Extra` 预留给
/// 4 槽以上的处理（营火）；目前无声明使用它。
///
/// Workstation/furnace slot role (server container addressing). `Extra` is
/// reserved for 4+-slot processes (campfire); no shipped decl uses it yet.
#[derive(Clone, Deserialize, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum WorkstationSlotRole {
    Input,
    Fuel,
    Result,
    Extra,
}

/// 一个工作台槽位：游玩角色 + Bedrock 容器枚举字节
/// （客户端在 Take/Place 中使用的取值，如熔炉 25/24/26）。
///
/// One workstation slot: gameplay role plus Bedrock container enum byte
/// (the values the client uses in Take/Place, e.g. furnace 25/24/26).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct WorkstationSlot {
    /// 槽位角色。
    ///
    /// Slot role.
    pub role: WorkstationSlotRole,
    /// 容器枚举字节。
    ///
    /// Container enum byte.
    pub container: u8,
}

/// 箱子容器组件（`sc:chest`）。
///
/// Chest container component (`sc:chest`).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Chest {
    /// 槽位数。
    ///
    /// Slot count.
    #[serde(default = "default_chest_slots")]
    pub slots: u16,
    /// 容器类型枚举值。
    ///
    /// Container type enum value.
    #[serde(default = "default_chest_container_type")]
    pub container_type: i8,
    /// 是否可配对（大箱）。
    ///
    /// Whether pairable (double chest).
    #[serde(default = "default_true")]
    pub pairable: bool,
    /// 是否检查遮挡。
    ///
    /// Whether obstruction is checked.
    #[serde(default = "default_true")]
    pub check_obstruction: bool,
    /// 打开音效。
    ///
    /// Open sound.
    #[serde(default = "default_chest_open_sound")]
    pub open_sound: String,
    /// 关闭音效。
    ///
    /// Close sound.
    #[serde(default = "default_chest_close_sound")]
    pub close_sound: String,
}

/// 箱子槽位数缺省值。
///
/// Default chest slot count.
fn default_chest_slots() -> u16 {
    27
}

/// 箱子容器类型缺省值。
///
/// Default chest container type.
fn default_chest_container_type() -> i8 {
    0
}

/// 布尔真缺省值。
///
/// Default boolean true.
fn default_true() -> bool {
    true
}

/// 箱子打开音效缺省值。
///
/// Default chest open sound.
fn default_chest_open_sound() -> String {
    "random.chestopen".to_string()
}

/// 箱子关闭音效缺省值。
///
/// Default chest close sound.
fn default_chest_close_sound() -> String {
    "random.chestclosed".to_string()
}

/// 熔炉处理组件（`sc:furnace`）。
///
/// Furnace processing component (`sc:furnace`).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Furnace {
    /// 熔炉种类（furnace/blast_furnace/smoker）。
    ///
    /// Furnace kind (furnace/blast_furnace/smoker).
    #[serde(default = "default_furnace_kind")]
    pub kind: String,
    /// 烹饪耗时（刻）。
    ///
    /// Cook time (ticks).
    #[serde(default = "default_furnace_cook_ticks")]
    pub cook_ticks: u16,
    /// 是否为燃料处理方块。
    ///
    /// Whether this is a fuel-processing block.
    #[serde(default = "default_true")]
    pub fuel: bool,
    /// 是否点燃状态。
    ///
    /// Whether lit.
    #[serde(default)]
    pub lit: bool,
    /// 点燃变体标识。
    ///
    /// Lit variant identifier.
    #[serde(default)]
    pub lit_variant: Option<String>,
    /// 容器类型枚举值。
    ///
    /// Container type enum value.
    #[serde(default = "default_furnace_container_type")]
    pub container_type: i8,
    /// 槽位列表。
    ///
    /// Slot list.
    #[serde(default = "default_furnace_slots")]
    pub slots: Vec<WorkstationSlot>,
}

/// 熔炉种类缺省值。
///
/// Default furnace kind.
fn default_furnace_kind() -> String {
    "furnace".to_string()
}

/// 熔炉烹饪耗时缺省值。
///
/// Default furnace cook ticks.
fn default_furnace_cook_ticks() -> u16 {
    200
}

/// 熔炉容器类型缺省值。
///
/// Default furnace container type.
fn default_furnace_container_type() -> i8 {
    2
}

/// 熔炉缺省槽位表（输入 25 / 燃料 24 / 产出 26）。
///
/// Default furnace slot table (input 25 / fuel 24 / result 26).
fn default_furnace_slots() -> Vec<WorkstationSlot> {
    vec![
        WorkstationSlot {
            role: WorkstationSlotRole::Input,
            container: 25,
        },
        WorkstationSlot {
            role: WorkstationSlotRole::Fuel,
            container: 24,
        },
        WorkstationSlot {
            role: WorkstationSlotRole::Result,
            container: 26,
        },
    ]
}

/// 工作台组件（`sc:crafting_table`）。
///
/// Crafting table component (`sc:crafting_table`).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CraftingTable {
    /// 网格边长（2/3）。
    ///
    /// Grid side length (2/3).
    #[serde(default = "default_crafting_table_grid")]
    pub grid: u8,
    /// 工作站标识。
    ///
    /// Station identifier.
    #[serde(default = "default_crafting_table_station")]
    pub station: String,
    /// 容器类型枚举值。
    ///
    /// Container type enum value.
    #[serde(default = "default_crafting_table_container_type")]
    pub container_type: i8,
}

/// 工作台网格边长缺省值。
///
/// Default crafting table grid side.
fn default_crafting_table_grid() -> u8 {
    3
}

/// 工作站标识缺省值。
///
/// Default station identifier.
fn default_crafting_table_station() -> String {
    "crafting_table".to_string()
}

/// 工作台容器类型缺省值。
///
/// Default crafting table container type.
fn default_crafting_table_container_type() -> i8 {
    1
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

/// sc:mining.default 或 tools 数组中的一条完整挖掘规则。
///
/// 规则描述是否可挖掘、是否需要采集工具以及速度倍率。
///
/// A complete mining rule for sc:mining.default or an entry of tools, describing whether mining is allowed, whether harvesting is required, and the speed multiplier.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MiningRule {
    pub can_mine: bool,
    pub harvest: bool,
    pub speed_multiplier: f64,
}

/// sc:mining.tools 数组的一项：适用物品列表与对应挖掘规则。
///
/// One entry of sc:mining.tools: the applicable item list and its mining rule.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MiningToolEntry {
    pub items: Vec<String>,
    pub can_mine: bool,
    pub harvest: bool,
    pub speed_multiplier: f64,
}

/// sc:mining.enchantments 中效率附魔的等级上限配置。
///
/// Efficiency enchantment level cap configuration within sc:mining.enchantments.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EfficiencyRule {
    pub max_level: u32,
}

/// sc:mining 组件的挖掘参数：默认规则、工具条目与效率配置。
///
/// Mining parameters of the sc:mining component: default rule, tool entries, and efficiency configuration.
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
                "sc:chest",
                "sc:crafting_table",
                "sc:furnace",
                "sc:liquid",
                "sc:mining",
                "sc:needs_support",
                "sc:random_tick",
                "sc:replaceable",
                "sc:unbreakable",
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
        assert_eq!(Chest::NAME, "sc:chest");
        assert_eq!(Furnace::NAME, "sc:furnace");
        assert_eq!(CraftingTable::NAME, "sc:crafting_table");
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
    fn domain_components_shapes() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            Chest::NAME.to_string(),
            json!({
                "slots": 27,
                "pairable": true,
                "check_obstruction": true,
            }),
        );
        map.insert(
            Furnace::NAME.to_string(),
            json!({
                "kind": "blast_furnace",
                "cook_ticks": 100,
                "fuel": true,
                "lit_variant": "minecraft:lit_blast_furnace"
            }),
        );
        map.insert(
            CraftingTable::NAME.to_string(),
            json!({
                "grid": 3,
                "station": "crafting_table"
            }),
        );

        let chest: Chest = BlockComponents::get_from(&map).expect("Chest 应读出");
        assert_eq!(chest.slots, 27);
        assert_eq!(chest.container_type, 0);
        assert!(chest.pairable);
        assert!(chest.check_obstruction);
        assert_eq!(chest.open_sound, "random.chestopen");

        let furnace: Furnace = BlockComponents::get_from(&map).expect("Furnace 应读出");
        assert_eq!(furnace.kind, "blast_furnace");
        assert_eq!(furnace.cook_ticks, 100);
        assert!(furnace.fuel);
        assert_eq!(furnace.lit_variant.as_deref(), Some("minecraft:lit_blast_furnace"));
        assert_eq!(furnace.slots.len(), 3);

        let table: CraftingTable = BlockComponents::get_from(&map).expect("CraftingTable 应读出");
        assert_eq!(table.grid, 3);
        assert_eq!(table.station, "crafting_table");
        assert_eq!(table.container_type, 1);
    }
}
