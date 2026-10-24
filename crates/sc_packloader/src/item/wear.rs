//! 物品耐久损耗表：把版本包声明编译为按 runtime id 索引的只读稠密表。
//!
//! Item wear table: compiles version-pack declarations into a read-only dense
//! table indexed by runtime id.
//!
//! 两个来源各司其职，**同一个语义只有一个可写位置**：
//! Both sources have a single job each, and no semantic has two writable homes:
//!
//! 1. `minecraft:durability`（官方组件）→ 容量上限 + 每次动作的受损概率；
//!    `minecraft:durability` (official) → capacity cap + damage chance;
//! 2. `sc:durability_costs`（服务端专有）→ 每次动作扣多少点，可选；
//!    `sc:durability_costs` (server-only) → points lost per action, optional;
//! 3. 没有官方组件 → 该物品不磨损（表中无条目）；只有 `sc:durability_costs`
//!    而没有官方容量视为配置错误（记 error 并丢弃）。
//!    no official component → the item never wears (no entry); costs declared
//!    without an official cap are a configuration error (logged and dropped).
//!
//! 运行期只读：编译一次，之后 `resolve` 只是一次下标查询。任何声明错误都在
//! 编译期以显式日志报告并整条丢弃，绝不静默退化成“无耐久”。
//!
//! Read-only at runtime: compiled once, after which `resolve` is one indexed
//! lookup. Declaration errors are reported explicitly at compile time and the
//! entry is dropped; they never silently degrade into "no durability".

use crate::item::component::{Durability, DurabilityCosts};
use crate::item::ItemComponents;
use crate::macros::ComponentKey;
use crate::version_control::runtime::MinecraftRuntimeManager;
use parking_lot::RwLock;
use sc_ecs::resource::Resource;
use serde_json::{Map, Value};
use std::sync::Arc;

/// 单件物品允许声明的最大耐久上限（防御性预算）。
///
/// Upper bound for a declared maximum durability (defensive budget).
pub const MAX_WEAR_DURABILITY: u32 = 1_000_000;

/// 单次动作允许的损耗上限（防御性预算）。
///
/// Upper bound for a per-action cost (defensive budget).
pub const MAX_WEAR_COST: u32 = 16;

/// 概率区间的上界（百分比）。
///
/// Upper bound of a damage-chance range (percent).
pub const MAX_WEAR_CHANCE_PERCENT: u8 = 100;

/// 未声明概率时的缺省区间（恒受损）。
///
/// Default chance range when nothing is declared (always damaged).
pub const DEFAULT_WEAR_CHANCE: (u8, u8) = (100, 100);

/// 单件物品的编译态耐久配置。
///
/// Compiled wear configuration of one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WearProfile {
    /// 最大耐久；达到即破损。恒 `>= 1`（无耐久不建条目）。
    ///
    /// Maximum durability; reaching it breaks the item. Always `>= 1` (items
    /// without durability get no entry).
    pub max_durability: u32,
    /// 受损概率区间（百分比，闭区间，`min <= max`）。
    ///
    /// Damage chance range in percent (closed interval, `min <= max`).
    pub damage_chance: (u8, u8),
    /// 每次动作的损耗点数。
    ///
    /// Durability points lost per action.
    pub costs: DurabilityCosts,
}

/// 按 runtime id 索引的耐久损耗表。
///
/// Wear table indexed by runtime id.
#[derive(Resource, Clone, Debug, Default)]
pub struct WearTable {
    /// 稠密表：`runtime_id` 即下标，空洞为 `None`。
    ///
    /// Dense table: `runtime_id` is the index, holes are `None`.
    entries: Arc<RwLock<Vec<Option<WearProfile>>>>,
}

impl WearTable {
    /// 创建空表。
    ///
    /// Creates an empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// 从运行时管理器编译整表（`SCLoad` 调用一次）。
    ///
    /// Compiles the whole table from the runtime manager (called once during
    /// `SCLoad`).
    ///
    /// 失败即闭合：解析错误、越界数值与半声明条目都会被记 error 并丢弃，
    /// 不会 panic，也不会留下半条配置。
    ///
    /// - `runtime_manager`: 已加载物品定义的运行时管理器
    ///
    /// Fail-closed: parse errors, out-of-range values and half-declared entries
    /// are logged and dropped — no panic, no partial profile.
    ///
    /// - `runtime_manager`: runtime manager with item definitions already loaded
    pub fn build(&mut self, runtime_manager: &MinecraftRuntimeManager) {
        let mut entries = self.entries.write();
        entries.clear();
        let mut context = BuildContext::default();
        for (name, runtime_id) in runtime_manager.runtime_entries() {
            let id = runtime_id as u16;
            if id == 0 {
                continue;
            }
            let Some(components) = runtime_manager
                .get_item_spawner(runtime_id)
                .and_then(|spawner| spawner.components.as_ref())
            else {
                continue;
            };
            let ItemComponents::Map(map) = components;
            let Some(profile) = compile_profile(&name, map, &mut context) else {
                continue;
            };
            if entries.len() <= id as usize {
                entries.resize(id as usize + 1, None);
            }
            entries[id as usize] = Some(profile);
        }
        log::debug!(
            "[item-wear] compiled {} wearable items ({} rejected)",
            context.compiled,
            context.rejected
        );
    }

    /// 直接写入一条配置（覆盖同 id 旧值）。
    ///
    /// Writes one profile directly (overwrites the same id).
    ///
    /// 供集成测试与程序化注册使用；版本包数据仍走 [`Self::build`]。
    /// For integration tests and programmatic registration; version-pack data
    /// still goes through [`Self::build`].
    ///
    /// - `runtime_id`: 物品的网络运行时 id
    /// - `profile`: 编译态耐久配置
    pub fn insert(&self, runtime_id: u16, profile: WearProfile) {
        let mut entries = self.entries.write();
        if entries.len() <= runtime_id as usize {
            entries.resize(runtime_id as usize + 1, None);
        }
        entries[runtime_id as usize] = Some(profile);
    }

    /// 取指定 runtime id 的耐久配置；无声明返回 `None`（该物品不磨损）。
    ///
    /// Looks up the wear profile of one runtime id; `None` when nothing is
    /// declared (the item never wears).
    ///
    /// - `runtime_id`: 物品的网络运行时 id
    pub fn resolve(&self, runtime_id: u16) -> Option<WearProfile> {
        self.entries
            .read()
            .get(runtime_id as usize)
            .copied()
            .flatten()
    }

    /// 已声明耐久配置的物品数量（跳过空洞）。
    ///
    /// Number of items with a declared wear profile (holes skipped).
    pub fn count(&self) -> usize {
        self.entries.read().iter().filter(|entry| entry.is_some()).count()
    }

    /// 是否没有任何物品声明耐久。
    ///
    /// Whether no item declares durability.
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }
}

/// 编译期计数与日志去重状态。
///
/// Compile-time counters and log de-duplication state.
#[derive(Default)]
struct BuildContext {
    compiled: usize,
    rejected: usize,
}

/// 编译单件物品的耐久配置；无声明返回 `None`。
///
/// Compiles one item's wear profile; `None` when nothing is declared.
///
/// - `item`: 物品标识符（仅用于日志）
/// - `map`: 该物品的原始组件映射
/// - `context`: 编译计数
fn compile_profile(
    item: &str,
    map: &Map<String, Value>,
    context: &mut BuildContext,
) -> Option<WearProfile> {
    let official = parse_component::<Durability>(item, map, context)?;
    let costs = parse_component::<DurabilityCosts>(item, map, context)?;

    // Capacity and chance come only from the official component.
    let Some(official) = official else {
        if costs.is_some() {
            log::error!(
                "[item-wear] {item}: 声明了 sc:durability_costs 却没有 minecraft:durability（无容量），条目已丢弃"
            );
            context.rejected += 1;
        }
        return None;
    };
    if official.max_durability < 0 || official.max_durability as u32 > MAX_WEAR_DURABILITY {
        log::error!(
            "[item-wear] {item}: max_durability {} 越界（允许 1..={MAX_WEAR_DURABILITY}），条目已丢弃",
            official.max_durability
        );
        context.rejected += 1;
        return None;
    }
    if official.max_durability == 0 {
        return None;
    }
    let Some(chance) = resolve_damage_chance(item, &official, context) else {
        return None;
    };
    let costs = costs.unwrap_or_default();
    if let Some(bad) = invalid_cost(&costs) {
        log::error!(
            "[item-wear] {item}: 损耗点数 {bad} 越界（允许 1..={MAX_WEAR_COST}），条目已丢弃"
        );
        context.rejected += 1;
        return None;
    }

    context.compiled += 1;
    Some(WearProfile {
        max_durability: official.max_durability as u32,
        damage_chance: chance,
        costs,
    })
}

/// 解析一个可选组件；解析失败记 error 并返回 `None`（不阻断其它物品）。
///
/// Parses one optional component; a parse failure is logged and yields `None`
/// (other items keep compiling).
fn parse_component<T>(
    item: &str,
    map: &Map<String, Value>,
    context: &mut BuildContext,
) -> Option<Option<T>>
where
    T: ComponentKey + serde::de::DeserializeOwned,
{
    let Some(value) = map.get(T::NAME) else {
        return Some(None);
    };
    match serde_json::from_value::<T>(value.clone()) {
        Ok(parsed) => Some(Some(parsed)),
        Err(error) => {
            log::error!("[item-wear] {item}: 组件 {} 解析失败：{error}", T::NAME);
            context.rejected += 1;
            None
        }
    }
}

/// 读取官方组件的受损概率区间（百分比，钳制到 `0..=100`）。
///
/// Reads the official component's damage chance range (percent, clamped to
/// `0..=100`).
fn resolve_damage_chance(
    item: &str,
    official: &Durability,
    context: &mut BuildContext,
) -> Option<(u8, u8)> {
    let (min, max) = official.damage_chance.bounds();
    if !min.is_finite() || !max.is_finite() {
        log::error!("[item-wear] {item}: damage_chance 不是有限数，条目已丢弃");
        context.rejected += 1;
        return None;
    }
    if min > max {
        log::error!("[item-wear] {item}: damage_chance.min ({min}) > max ({max})，条目已丢弃");
        context.rejected += 1;
        return None;
    }
    let clamp = |value: f64| -> u8 { value.clamp(0.0, 100.0).round() as u8 };
    let (min, max) = (clamp(min), clamp(max));
    debug_assert!(min <= max && max <= MAX_WEAR_CHANCE_PERCENT);
    Some((min, max))
}

/// 返回第一个越界的损耗点数。
///
/// Returns the first out-of-range cost, if any.
fn invalid_cost(costs: &DurabilityCosts) -> Option<u32> {
    [costs.mine_correct, costs.mine_incorrect, costs.attack, costs.use_item]
        .into_iter()
        .find(|cost| *cost == 0 || *cost > MAX_WEAR_COST)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => panic!("expected object"),
        }
    }

    fn compile(value: Value) -> Option<WearProfile> {
        let mut context = BuildContext::default();
        compile_profile("minecraft:test", &map(value), &mut context)
    }

    #[test]
    fn official_durability_supplies_cap_and_chance() {
        let profile = compile(json!({
            "minecraft:durability": { "max_durability": 1560, "damage_chance": { "min": 0, "max": 100 } }
        }))
        .expect("profile");
        assert_eq!(profile.max_durability, 1560);
        assert_eq!(profile.damage_chance, (0, 100));
        assert_eq!(profile.costs, DurabilityCosts::default());
    }

    #[test]
    fn missing_damage_chance_defaults_to_always() {
        let profile = compile(json!({ "minecraft:durability": { "max_durability": 60 } }))
            .expect("profile");
        assert_eq!(profile.damage_chance, DEFAULT_WEAR_CHANCE);
    }

    #[test]
    fn durability_costs_override_only_the_costs() {
        let profile = compile(json!({
            "minecraft:durability": { "max_durability": 251 },
            "sc:durability_costs": { "mine_correct": 3, "mine_incorrect": 4 }
        }))
        .expect("profile");
        // Capacity stays official.
        assert_eq!(profile.max_durability, 251);
        assert_eq!(profile.costs.mine_correct, 3);
        assert_eq!(profile.costs.mine_incorrect, 4);
        // Unlisted costs keep their defaults.
        assert_eq!(profile.costs.attack, 1);
        assert_eq!(profile.costs.use_item, 1);
    }

    #[test]
    fn authored_range_lowers_the_wear_rate() {
        // `damage_chance` is a *probability range*, not "always": `{0,100}` picks a
        // random percentage per action, so roughly half of them do not wear.
        let profile = compile(json!({
            "minecraft:durability": { "max_durability": 60, "damage_chance": { "min": 0, "max": 100 } }
        }))
        .expect("profile");
        assert_eq!(profile.damage_chance, (0, 100));
        // Tools must omit it (or author 100) so every break charges.
        let tool = compile(json!({ "minecraft:durability": { "max_durability": 60 } }))
            .expect("profile");
        assert_eq!(tool.damage_chance, DEFAULT_WEAR_CHANCE);
    }

    #[test]
    fn costs_without_official_capacity_are_rejected() {
        // `sc:durability_costs` alone cannot invent a cap.
        assert!(compile(json!({ "sc:durability_costs": { "mine_correct": 1 } })).is_none());
    }

    #[test]
    fn no_declaration_builds_no_entry() {
        assert!(compile(json!({ "minecraft:fuel": { "duration": 80.0 } })).is_none());
    }

    #[test]
    fn zero_or_negative_cap_is_dropped() {
        assert!(compile(json!({ "minecraft:durability": { "max_durability": 0 } })).is_none());
        assert!(compile(json!({ "minecraft:durability": { "max_durability": -5 } })).is_none());
    }

    #[test]
    fn out_of_range_cost_is_dropped() {
        assert!(compile(json!({
            "minecraft:durability": { "max_durability": 100 },
            "sc:durability_costs": { "mine_correct": 0 }
        }))
        .is_none());
        assert!(compile(json!({
            "minecraft:durability": { "max_durability": 100 },
            "sc:durability_costs": { "use_item": 99 }
        }))
        .is_none());
    }

    #[test]
    fn inverted_chance_is_dropped() {
        assert!(compile(json!({
            "minecraft:durability": { "max_durability": 60, "damage_chance": { "min": 80, "max": 20 } }
        }))
        .is_none());
    }

    #[test]
    fn unknown_cost_field_is_rejected() {
        // `deny_unknown_fields` keeps authoring mistakes loud: capacity does not
        // live here, so `max_durability` is an unknown field.
        assert!(compile(json!({
            "minecraft:durability": { "max_durability": 100 },
            "sc:durability_costs": { "max_durability": 100 }
        }))
        .is_none());
        assert!(compile(json!({
            "minecraft:durability": { "max_durability": 100 },
            "sc:durability_costs": { "mine_correct": 1, "mine_incorerct": 2 }
        }))
        .is_none());
    }

    #[test]
    fn legacy_tool_json_compiles() {
        // Version-pack declaration for a legacy (non data-driven) tool item.
        let raw = include_str!(
            "../../../../version_packs/[SC原版版本包] Vanilla-1.26.40/behavior_packs/vanilla/items/wooden_pickaxe.json"
        );
        let parsed: Value = serde_json::from_reader(json_comments::StripComments::new(raw.as_bytes()))
            .expect("valid json");
        let components = parsed["minecraft:item"]["components"].clone();
        let profile = compile(components).expect("profile");
        assert_eq!(profile.max_durability, 60);
        // Tools omit `damage_chance`: the default makes every break wear.
        assert_eq!(profile.damage_chance, DEFAULT_WEAR_CHANCE);
        assert_eq!(profile.costs.mine_correct, 1);
        assert_eq!(profile.costs.mine_incorrect, 2);
    }

    /// 版本包内原版工具的 `minecraft:durability` 声明必须齐全且数值正确。
    ///
    /// Every vanilla tool in the version pack must declare
    /// `minecraft:durability` with the expected cap.
    #[test]
    fn version_pack_declares_every_vanilla_tool() {
        use crate::{MinecraftJson, MinecraftJsonDeserializer};

        let materials = [
            ("wooden", 60),
            ("stone", 132),
            ("copper", 192),
            ("iron", 251),
            ("golden", 33),
            ("diamond", 1562),
            ("netherite", 2032),
        ];
        let kinds = ["pickaxe", "axe", "shovel", "hoe", "sword"];
        let mut expected: Vec<(String, u32)> = Vec::new();
        for (material, durability) in materials {
            for kind in kinds {
                expected.push((format!("{material}_{kind}"), durability));
            }
        }
        expected.push(("shears".to_string(), 239));
        expected.push(("flint_and_steel".to_string(), 65));

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../version_packs/[SC原版版本包] Vanilla-1.26.40/behavior_packs/vanilla/items",
        );
        for (file_stem, durability) in expected {
            let path = dir.join(format!("{file_stem}.json"));
            let raw = std::fs::read(&path).unwrap_or_else(|error| {
                panic!("missing version-pack declaration {}: {error}", path.display())
            });
            let parsed = serde_json::from_reader::<_, MinecraftJsonDeserializer>(
                json_comments::StripComments::new(raw.as_slice()),
            )
            .unwrap_or_else(|error| panic!("{file_stem}.json 解析失败：{error}"));
            let MinecraftJson::MinecraftItemSpawner(spawner) = parsed.get() else {
                panic!("{file_stem}.json 不是 minecraft:item");
            };
            assert_eq!(
                spawner.description.map(|description| description.identifier),
                Some(format!("minecraft:{file_stem}"))
            );
            let components = spawner.components.expect("components");
            let ItemComponents::Map(map) = &components;
            let profile = compile_profile(&format!("minecraft:{file_stem}"), map, &mut BuildContext::default())
                .unwrap_or_else(|| panic!("{file_stem}.json 未编译出耐久配置"));
            assert_eq!(profile.max_durability, durability, "{file_stem}");
        }
    }

    /// 端到端探针：加载真实版本包并编译整张表（`cargo test -- --ignored`）。
    ///
    /// End-to-end probe: loads the real version pack and compiles the whole
    /// table (run with `cargo test -- --ignored`).
    #[test]
    #[ignore = "reads the real version pack; run with --ignored"]
    fn real_version_pack_builds_the_wear_table() {
        use crate::version_control::loader::VersionPackLoader;

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../version_packs");
        let versions = VersionPackLoader::new(root).get_versions();
        let mut selected = versions
            .into_iter()
            .max_by_key(|version| version.manifest.protocol_version)
            .expect("version pack");
        let mut manager = MinecraftRuntimeManager::new(selected.take_runtime_id());
        let mut packs = selected.take_behavior_packs();
        packs.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (_, pack) in packs {
            manager.push_item_map(pack.items);
        }

        let mut table = WearTable::new();
        table.build(&manager);
        let resolve = |name: &str| {
            let runtime_id = manager.get_runtime_id(name)? as u16;
            table.resolve(runtime_id)
        };
        // Legacy tools come from the item JSON declarations.
        assert_eq!(resolve("minecraft:wooden_pickaxe").unwrap().max_durability, 60);
        assert_eq!(resolve("minecraft:netherite_sword").unwrap().max_durability, 2032);
        assert_eq!(resolve("minecraft:shears").unwrap().max_durability, 239);
        // Data-driven items keep using the official component.
        assert_eq!(resolve("minecraft:diamond_spear").unwrap().max_durability, 1560);
        // Items without any declaration never wear.
        assert!(resolve("minecraft:stone").is_none());
        assert!(table.count() >= 37, "declared items: {}", table.count());
    }

    #[test]
    fn real_spear_json_compiles() {
        // Golden data: the declaration shipped by the version pack (JSONC, same
        // comment-stripping as the pack loader).
        let raw = include_str!(
            "../../../../version_packs/[SC原版版本包] Vanilla-1.26.40/behavior_packs/vanilla_1.21.130/items/diamond_spear.json"
        );
        let parsed: Value = serde_json::from_reader(json_comments::StripComments::new(raw.as_bytes()))
            .expect("valid json");
        let components = parsed["minecraft:item"]["components"].clone();
        let profile = compile(components).expect("profile");
        assert_eq!(profile.max_durability, 1560);
        assert_eq!(profile.damage_chance, (0, 100));
    }
}
