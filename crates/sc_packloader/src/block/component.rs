//! Block static components: ECS Component structs + registry (follows `item::component`).
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
//! | `Loot` | `minecraft:loot` | Loot-table path reference (full table lives in a separate data domain) |
//! | `Replaceable` | `sc:replaceable` | Server-only flag: replaceable boolean |
//! | `Liquid` | `sc:liquid` | Server-only flag: liquid kind for non-customizable liquid blocks |
//! | `RandomTick` | `sc:random_tick` | Server-only flag: random-tick candidate filtering |
//! | `NeedsSupport` | `sc:needs_support` | Server-only flag: data reference for support conditions |
//! | `CanContainLiquid` | `sc:can_contain_liquid` | Server-only flag: whether a liquid layer is allowed |
//! | `Unbreakable` | `sc:unbreakable` | Server-only flag: mutually exclusive with mining |
//! | `Mining` | `sc:mining` | Mining speed/harvest/efficiency adjustment (`formula_version=1` exact formula) |
//! | `Drops` | `sc:drops` | Direct drop definitions (`independent`/`one_of` + count distribution + fortune bonus) |
//!
//! Namespace: `sc:*` is the only canonical namespace; any `ur:*` key is rejected.
//! New components (`sc:mining`/`sc:drops`) only accept `sc:*`; `ur:mining`/`ur:drops`
//! are rejected.
//!
//! Client-side entries (`display_name`, `geometry`, `material_instances`,
//! `selection_box`, `transformation`, `crafting_table`, `placement_filter`,
//! `custom_components`, `tick_queue`) are not registered here: at parse time they are
//! accepted (volume-bounded only) and discarded per [`IGNORED_COMPONENTS`]
//! (see `schema.rs`), and never enter snapshots.
//!
//! Each component is a standalone struct deriving [`sc_ecs::component::Component`] +
//! `Deserialize`; unknown fields are always rejected; value ranges are validated by
//! each struct's [`BlockComponentSchema`] implementation. The two vanilla
//! `minecraft:collision_box` spellings (single box / box array) are unwrapped by
//! [`collision_boxes_of`]; the component identity remains the single box.

use sc_ecs::component::Component;
use serde::Deserialize;
use serde_json::Value;

use crate::block::BlockJsonError;
use sc_log::t_log;

/// Block component schema: name + typed validation.
///
/// Parse flow: `serde_json::from_value::<T>` (shape) then `T::validate()` (ranges).
pub trait BlockComponentSchema: Component + serde::de::DeserializeOwned {
    /// Component name (e.g. `"minecraft:collision_box"`).
    const NAME: &'static str;
    /// Range/semantic check (shape is already guaranteed by Deserialize).
    fn validate(&self) -> Result<(), String>;
}

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
/// Component identity (`Component` + registry entry) is the single-box struct; unwrapping and
/// validation of both spellings happen in [`BlockComponentSchema for CollisionBox`] and [`collision_boxes_of`]
/// (the ECS component table registers per box, not per array wrapper).
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum CollisionBoxes {
    One(CollisionBox),
    Many(Vec<CollisionBox>),
}

/// Collision-box format sanity bound (not gameplay data, only rejects malformed input; 16ths).
pub const COLLISION_FORMAT_BOUND: f64 = 64.0;

/// Both vanilla spellings to box list (`enabled` kept per box).
pub fn collision_boxes_of(value: &Value) -> Result<Vec<CollisionBox>, String> {
    serde_json::from_value::<CollisionBoxes>(value.clone())
        .map(|b| match b {
            CollisionBoxes::One(b) => vec![b],
            CollisionBoxes::Many(v) => v,
        })
        .map_err(|e| format!("collision_box 形状非法：{e}"))
}

impl BlockComponentSchema for CollisionBox {
    const NAME: &'static str = "minecraft:collision_box";

    fn validate(&self) -> Result<(), String> {
        validate_collision_boxes(std::slice::from_ref(self))
    }
}

/// Box-list validation (per-box ranges + non-empty).
pub fn validate_collision_boxes(boxes: &[CollisionBox]) -> Result<(), String> {
    if boxes.is_empty() {
        return Err("collision_box 数组为空（无碰撞请写 enabled:false）".to_string());
    }
    for (bi, b) in boxes.iter().enumerate() {
        for (i, n) in b.origin.iter().chain(b.size.iter()).enumerate() {
            if !n.is_finite() {
                return Err(format!("boxes[{bi}] origin/size[{i}] 禁止 NaN/Inf"));
            }
            let f = *n as f64;
            if f < -COLLISION_FORMAT_BOUND || f > COLLISION_FORMAT_BOUND {
                return Err(format!(
                    "boxes[{bi}] origin/size[{i}] 超出格式边界 ±{COLLISION_FORMAT_BOUND}"
                ));
            }
        }
        for (i, s) in b.size.iter().enumerate() {
            if *s < 0.0 {
                return Err(format!("boxes[{bi}].size[{i}] 为负"));
            }
        }
    }
    Ok(())
}

/// Light emission (0..15).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct LightEmission(pub u8);

impl BlockComponentSchema for LightEmission {
    const NAME: &'static str = "minecraft:light_emission";

    fn validate(&self) -> Result<(), String> {
        if self.0 > 15 {
            return Err(format!("light_emission 超出 0..=15：{}", self.0));
        }
        Ok(())
    }
}

/// Light dampening (0..15).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct LightDampening(pub u8);

impl BlockComponentSchema for LightDampening {
    const NAME: &'static str = "minecraft:light_dampening";

    fn validate(&self) -> Result<(), String> {
        if self.0 > 15 {
            return Err(format!("light_dampening 超出 0..=15：{}", self.0));
        }
        Ok(())
    }
}

/// Mineable seconds (for break progress; indestructible see `sc:unbreakable`).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DestructibleByMining {
    pub value: f32,
}

impl BlockComponentSchema for DestructibleByMining {
    const NAME: &'static str = "minecraft:destructible_by_mining";

    fn validate(&self) -> Result<(), String> {
        if !self.value.is_finite() || self.value < 0.0 {
            return Err("destructible_by_mining.value 必须为非负有限数值".to_string());
        }
        Ok(())
    }
}

/// Loot-table path reference (full table lives in a separate data domain, no inline chances).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(transparent)]
pub struct Loot(pub Box<str>);

impl BlockComponentSchema for Loot {
    const NAME: &'static str = "minecraft:loot";

    fn validate(&self) -> Result<(), String> {
        if self.0.is_empty() {
            return Err("minecraft:loot 路径为空".to_string());
        }
        Ok(())
    }
}

/// Replaceable boolean (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Replaceable(pub bool);

impl BlockComponentSchema for Replaceable {
    const NAME: &'static str = "sc:replaceable";

    fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Liquid kind (vanilla liquid blocks are not customizable).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Liquid {
    pub kind: String,
}

impl BlockComponentSchema for Liquid {
    const NAME: &'static str = "sc:liquid";

    fn validate(&self) -> Result<(), String> {
        if self.kind.is_empty() {
            return Err("sc:liquid.kind 为空".to_string());
        }
        Ok(())
    }
}

/// Whether it participates in random-tick candidate filtering (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RandomTick(pub bool);

impl BlockComponentSchema for RandomTick {
    const NAME: &'static str = "sc:random_tick";

    fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Data reference for support conditions (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct NeedsSupport {
    pub condition: String,
}

impl BlockComponentSchema for NeedsSupport {
    const NAME: &'static str = "sc:needs_support";

    fn validate(&self) -> Result<(), String> {
        if self.condition.is_empty() {
            return Err("sc:needs_support.condition 为空".to_string());
        }
        Ok(())
    }
}

/// Whether a liquid layer is allowed (server-only concept).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanContainLiquid(pub bool);

impl BlockComponentSchema for CanContainLiquid {
    const NAME: &'static str = "sc:can_contain_liquid";

    fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Unbreakable (no vanilla custom-block equivalent; mutually exclusive with mining, see mapping-layer check).
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Unbreakable {}

impl BlockComponentSchema for Unbreakable {
    const NAME: &'static str = "sc:unbreakable";

    fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// sc:mining / sc:drops (mining, drop, and enchantment extensions; formula_version=1 exact formula)
// ---------------------------------------------------------------------------

/// Enchant identifier shape check (`namespace:path`, lowercase charset; length re-checked against schema budgets).
fn check_identifier_shape(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("identifier 为空".to_string());
    }
    let mut parts = id.splitn(2, ':');
    let ns = parts.next().unwrap_or("");
    let path = parts
        .next()
        .ok_or_else(|| "identifier 缺少 ':'".to_string())?;
    fn is_ns_char(b: u8) -> bool {
        matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-')
    }
    if ns.is_empty() || !ns.bytes().all(is_ns_char) {
        return Err(format!("namespace 非法: {ns:?}"));
    }
    if path.is_empty() {
        return Err("path 为空".to_string());
    }
    for seg in path.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(format!("path 含非法段: {seg:?}"));
        }
        if !seg.bytes().all(is_ns_char) {
            return Err(format!("path 段字符非法: {seg:?}"));
        }
    }
    Ok(())
}

fn check_finite_non_negative(v: f64, what: &str) -> Result<(), String> {
    if !v.is_finite() {
        return Err(format!("{what} 必须为有限数值（禁止 NaN/Inf）"));
    }
    if v < 0.0 {
        return Err(format!("{what} 不得为负数"));
    }
    Ok(())
}

fn check_positive_finite(v: f64, what: &str) -> Result<(), String> {
    if !v.is_finite() {
        return Err(format!("{what} 必须为有限数值（禁止 NaN/Inf）"));
    }
    if !(v > 0.0) {
        return Err(format!("{what} 必须为正数"));
    }
    Ok(())
}

fn check_chance(v: f64, what: &str) -> Result<(), String> {
    if !v.is_finite() {
        return Err(format!("{what} 必须为有限数值（禁止 NaN/Inf）"));
    }
    if !(0.0..=1.0).contains(&v) {
        return Err(format!("{what} 必须在 0..=1 内（实际 {v}）"));
    }
    Ok(())
}

/// Allowed mining enchants (allowlist for `sc:mining.enchantments` keys).
pub const ALLOWED_MINING_ENCHANTS: &[&str] = &["minecraft:efficiency"];
/// Allowed drop enchants (allowlist for `entries[].enchantments` keys).
pub const ALLOWED_DROP_ENCHANTS: &[&str] = &["minecraft:fortune"];
/// Fortune `bonus_by_level.level` range (1..=3).
pub const FORTUNE_LEVEL_MIN: u32 = 1;
pub const FORTUNE_LEVEL_MAX: u32 = 3;
/// Efficiency `max_level` range.
pub const EFFICIENCY_MAX_LEVEL_MIN: u32 = 1;
pub const EFFICIENCY_MAX_LEVEL_MAX: u32 = 255;
/// Count-distribution normalization tolerance.
pub const COUNT_SUM_TOLERANCE: f64 = 1e-6;
/// `one_of` total-probability tolerance.
pub const ONE_OF_TOTAL_TOLERANCE: f64 = 1e-9;
/// Per-entry `value + fortune bonus` cap (bounds malformed configs that could flood the drop queue).
pub const MAX_DROP_COUNT_TOTAL: u32 = 1024;

/// Default harvest penalty multiplier (time ratio for the unharvested branch; 10/3).
/// Default harvest penalty multiplier (unharvested-branch `hardness*5` factor).
pub const DEFAULT_HARVEST_PENALTY: f64 = 5.0;
/// Harvested-branch time factor (`hardness*1.5`; pairs with the penalty term).
pub const HARVESTED_BRANCH_FACTOR: f64 = 1.5;

/// One rule for `sc:mining.default` / `tools[]` (no inheritance; every rule is complete).
///
/// - `can_mine=false`: survival cannot break (creative bypasses); then `speed_multiplier` must be
///   the `1.0` placeholder and `harvest` must be `false` (no break means no harvest, no dead config);
/// - `harvest=false`: slow break is allowed, but time is scaled by the harvest penalty and drops fall back to the default empty drop
///   (unharvested branch: `hardness*5` time + empty drops).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MiningRule {
    pub can_mine: bool,
    pub harvest: bool,
    pub speed_multiplier: f64,
}

impl MiningRule {
    fn validate(&self, what: &str) -> Result<(), String> {
        check_positive_finite(self.speed_multiplier, &format!("{what}.speed_multiplier"))?;
        if !self.can_mine {
            if self.speed_multiplier != 1.0 {
                return Err(format!(
                    "{what}: can_mine=false 时 speed_multiplier 必须为 1.0（占位，实际被忽略；实际 {}）",
                    self.speed_multiplier
                ));
            }
            if self.harvest {
                return Err(format!(
                    "{what}: can_mine=false 时 harvest 必须为 false（不能破坏即无收获）"
                ));
            }
        }
        Ok(())
    }
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

impl MiningToolEntry {
    fn validate(&self, idx: usize) -> Result<(), String> {
        let what = format!("tools[{idx}]");
        if self.items.is_empty() {
            return Err(format!("{what}.items 为空（至少一个物品）"));
        }
        {
            use std::collections::HashSet;
            let mut seen = HashSet::new();
            for item in self.items.iter() {
                if !seen.insert(item) {
                    return Err(format!("{what}.items 内重复：{item:?}"));
                }
                check_identifier_shape(item)
                    .map_err(|m| format!("{what}.items {item:?} 形状非法：{m}"))?;
                if item == "minecraft:air" {
                    return Err(format!("{what}.items 禁止 minecraft:air（空手走 default）"));
                }
            }
        }
        MiningRule {
            can_mine: self.can_mine,
            harvest: self.harvest,
            speed_multiplier: self.speed_multiplier,
        }
        .validate(&what)?;
        Ok(())
    }
}

/// `sc:mining.enchantments["minecraft:efficiency"]`.
///
/// Bonus formula is the exact break-time formula (`speed += level^2 + 1`);
/// data only declares the `max_level` clamp (effective level is `min(actual, max_level)`; no enchantment-registry check,
/// see follow-up work).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EfficiencyRule {
    pub max_level: u32,
}

impl EfficiencyRule {
    fn validate(&self, key: &str) -> Result<(), String> {
        if !(EFFICIENCY_MAX_LEVEL_MIN..=EFFICIENCY_MAX_LEVEL_MAX).contains(&self.max_level) {
            return Err(format!(
                "enchantments[{key:?}].max_level 必须在 1..=255 内（实际 {}）",
                self.max_level
            ));
        }
        Ok(())
    }
}

/// `sc:mining` (`formula_version=1`: exact break-time formula).
///
/// Time formula: `time = base * branch / (tool_speed + efficiency_bonus)`, where
/// `base` is the raw hardness value, `branch` is `1.5` when harvested and
/// `harvest_penalty_multiplier` when not (defaults to `5.0`), using the same
/// operation order as the break-time calculation.
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

impl BlockComponentSchema for Mining {
    const NAME: &'static str = "sc:mining";

    fn validate(&self) -> Result<(), String> {
        if self.formula_version != 1 {
            return Err(format!(
                "formula_version only supports 1 (exact formula; got {})",
                self.formula_version
            ));
        }
        if let Some(b) = self.base_time_seconds {
            check_finite_non_negative(b, "base_time_seconds")?;
        }
        if let Some(p) = self.harvest_penalty_multiplier {
            check_positive_finite(p, "harvest_penalty_multiplier")?;
        }
        self.default.validate("default")?;
        {
            use std::collections::HashSet;
            let mut seen: HashSet<&str> = HashSet::new();
            for (i, entry) in self.tools.iter().enumerate() {
                entry.validate(i)?;
                for item in entry.items.iter() {
                    if !seen.insert(item.as_str()) {
                        return Err(format!(
                            "tools: 物品 {item:?} 在同一状态重复出现，不依赖数组顺序解决冲突"
                        ));
                    }
                }
            }
        }
        for (key, rule) in self.enchantments.iter() {
            if !ALLOWED_MINING_ENCHANTS.contains(&key.as_str()) {
                return Err(format!(
                    "enchantments: 首版仅允许 minecraft:efficiency（实际 {key:?}）"
                ));
            }
            check_identifier_shape(key)
                .map_err(|m| format!("enchantments 键 {key:?} 形状非法：{m}"))?;
            rule.validate(key)?;
        }
        Ok(())
    }
}

/// One `count` distribution option.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountOption {
    pub value: u32,
    pub chance: f64,
}

/// One fortune-bonus row.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FortuneBonus {
    pub level: u32,
    pub count: u32,
    pub chance: f64,
}

/// `entries[].enchantments["minecraft:fortune"]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FortuneRule {
    pub bonus_by_level: Vec<FortuneBonus>,
}

impl FortuneRule {
    fn validate(&self, entry_idx: usize) -> Result<(), String> {
        if self.bonus_by_level.is_empty() {
            return Err(format!(
                "entries[{entry_idx}].enchantments: bonus_by_level 为空"
            ));
        }
        {
            use std::collections::HashSet;
            let mut seen = HashSet::new();
            for b in self.bonus_by_level.iter() {
                if !(FORTUNE_LEVEL_MIN..=FORTUNE_LEVEL_MAX).contains(&b.level) {
                    return Err(format!(
                        "entries[{entry_idx}].bonus level 必须在 1..=3 内（实际 {}）",
                        b.level
                    ));
                }
                if !seen.insert(b.level) {
                    return Err(format!("entries[{entry_idx}].bonus level {} 重复", b.level));
                }
                if b.count < 1 {
                    return Err(format!(
                        "entries[{entry_idx}].bonus level {} 的 count 必须 >=1",
                        b.level
                    ));
                }
                check_chance(
                    b.chance,
                    &format!("entries[{entry_idx}].bonus level {} chance", b.level),
                )?;
            }
        }
        Ok(())
    }

    /// Sum of all bonus counts (used for the `value + sum` cap check).
    fn total_bonus(&self) -> u64 {
        self.bonus_by_level.iter().map(|b| b.count as u64).sum()
    }
}

/// `entries[].requires`: per-entry held-item condition (optional).
///
/// The held-item identifier must exactly match one of `items` for the entry to roll;
/// empty hand never matches. When absent, the entry has no held-item restriction.
/// Expresses tool-specific drops (e.g. a grass block dropping itself only with shears).
/// Uses the same exact-match semantics as the `sc:mining` tool index (no wildcards, no ordering).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiresRule {
    pub items: Vec<String>,
}

impl RequiresRule {
    fn validate(&self, idx: usize) -> Result<(), String> {
        if self.items.is_empty() {
            return Err(format!(
                "entries[{idx}].requires.items 为空（至少一个物品）"
            ));
        }
        {
            use std::collections::HashSet;
            let mut seen = HashSet::new();
            for item in self.items.iter() {
                if !seen.insert(item) {
                    return Err(format!("entries[{idx}].requires.items 内重复：{item:?}"));
                }
                check_identifier_shape(item)
                    .map_err(|m| format!("entries[{idx}].requires.items {item:?} 形状非法：{m}"))?;
                if item == "minecraft:air" {
                    return Err(format!(
                        "entries[{idx}].requires.items 禁止 minecraft:air（空手永不命中）"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// One entry of `sc:drops.entries[]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropEntry {
    pub item: String,
    pub chance: f64,
    pub count: Vec<CountOption>,
    #[serde(default)]
    pub enchantments: std::collections::BTreeMap<String, FortuneRule>,
    /// Held-item condition (optional; absent means unrestricted).
    #[serde(default)]
    pub requires: Option<RequiresRule>,
}

impl DropEntry {
    fn validate(&self, idx: usize) -> Result<(), String> {
        check_identifier_shape(&self.item)
            .map_err(|m| format!("entries[{idx}].item {:?} 形状非法：{m}", self.item))?;
        if self.item == "minecraft:air" {
            return Err(format!("entries[{idx}].item 禁止 minecraft:air"));
        }
        check_chance(self.chance, &format!("entries[{idx}].chance"))?;
        if self.chance == 0.0 {
            log::warn!("{}", t_log!("console.block.chance_zero", idx = idx));
        }
        if self.count.is_empty() {
            return Err(format!("entries[{idx}].count 为空（至少一个分布项）"));
        }
        let mut sum = 0.0f64;
        for (i, opt) in self.count.iter().enumerate() {
            if opt.value < 1 {
                return Err(format!("entries[{idx}].count[{i}].value 必须 >=1"));
            }
            check_chance(opt.chance, &format!("entries[{idx}].count[{i}].chance"))?;
            sum += opt.chance;
        }
        if (sum - 1.0).abs() > COUNT_SUM_TOLERANCE {
            return Err(format!(
                "entries[{idx}].count 分布概率和为 {sum}，要求 1.0 ± {COUNT_SUM_TOLERANCE}，不做静默归一化",
            ));
        }
        if self.enchantments.len() > 1 {
            return Err(format!(
                "entries[{idx}].enchantments 首版每条目至多一条 minecraft:fortune（实际 {} 条）",
                self.enchantments.len()
            ));
        }
        if let Some(requires) = self.requires.as_ref() {
            requires.validate(idx)?;
        }
        for (key, rule) in self.enchantments.iter() {
            if !ALLOWED_DROP_ENCHANTS.contains(&key.as_str()) {
                return Err(format!(
                    "entries[{idx}].enchantments 首版仅允许 minecraft:fortune（实际 {key:?}）"
                ));
            }
            rule.validate(idx)?;
            let bonus = rule.total_bonus();
            for (i, opt) in self.count.iter().enumerate() {
                let total = opt.value as u64 + bonus;
                if total > MAX_DROP_COUNT_TOTAL as u64 {
                    return Err(format!(
                        "entries[{idx}].count[{i}].value {} + 时运加成 {bonus} = {total} 超出上限 {}",
                        opt.value, MAX_DROP_COUNT_TOTAL
                    ));
                }
            }
        }
        // Without fortune, still cap value (value alone must not exceed the cap).
        if self.enchantments.is_empty() {
            for (i, opt) in self.count.iter().enumerate() {
                if opt.value > MAX_DROP_COUNT_TOTAL {
                    return Err(format!(
                        "entries[{idx}].count[{i}].value {} 超出上限 {}",
                        opt.value, MAX_DROP_COUNT_TOTAL
                    ));
                }
            }
        }
        Ok(())
    }
}

/// `sc:drops` (direct drop definitions; `minecraft:loot` references resolve to a separate data domain).
///
/// Harvest gate: with `require_harvest=true` (the default), a held tool that fails the harvest
/// requirement (see the `harvest` flag on `sc:mining` rules) falls back to the default empty drop;
/// with `false`, any tool rolls normally.
#[derive(Clone, Component, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Drops {
    pub enabled: bool,
    pub mode: String,
    #[serde(default)]
    pub entries: Vec<DropEntry>,
    /// Harvest-gate switch (defaults to true).
    #[serde(default)]
    pub require_harvest: Option<bool>,
}

impl BlockComponentSchema for Drops {
    const NAME: &'static str = "sc:drops";

    fn validate(&self) -> Result<(), String> {
        if self.mode != "independent" && self.mode != "one_of" {
            return Err(format!(
                "mode 必须为 independent 或 one_of（实际 {:?}）",
                self.mode
            ));
        }
        if !self.enabled && !self.entries.is_empty() {
            return Err("enabled=false 时 entries 必须缺失或为空".to_string());
        }
        if self.enabled && self.entries.is_empty() {
            return Err("enabled=true 时 entries 非空（至少一个条目）".to_string());
        }
        for (i, entry) in self.entries.iter().enumerate() {
            entry.validate(i)?;
        }
        if self.mode == "one_of" {
            let total: f64 = self.entries.iter().map(|e| e.chance).sum();
            if total > 1.0 + ONE_OF_TOTAL_TOLERANCE {
                return Err(format!(
                    "one_of 总概率为 {total}，要求 <= 1 + {ONE_OF_TOTAL_TOLERANCE}（超出即拒绝）"
                ));
            }
            if total == 0.0 {
                log::warn!("{}", t_log!("console.block.oneof_zero"));
            }
        }
        Ok(())
    }
}

/// Fixed component list (kept in sync with the dispatch below; locked by tests).
pub const ALL_BLOCK_COMPONENTS: &[&str] = &[
    "minecraft:collision_box",
    "minecraft:light_emission",
    "minecraft:light_dampening",
    "minecraft:destructible_by_mining",
    "minecraft:loot",
    "sc:replaceable",
    "sc:liquid",
    "sc:random_tick",
    "sc:needs_support",
    "sc:can_contain_liquid",
    "sc:unbreakable",
    "sc:mining",
    "sc:drops",
];

/// Client-side/future ignore list: legal vanilla file content the server currently does not consume.
/// Discarded after a volume-bounded check at parse time (shapes vary: string/object/array), never enters snapshots;
/// any other unknown component is an error (never silently ignored).
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

/// Validate one known component value (shape + ranges), with field paths on errors.
pub fn validate_block_component(
    pack: &str,
    file: &str,
    field: &str,
    name: &str,
    value: &Value,
) -> Result<(), BlockJsonError> {
    fn check<T: BlockComponentSchema>(
        pack: &str,
        file: &str,
        field: &str,
        value: &Value,
    ) -> Result<(), BlockJsonError> {
        let parsed: T = serde_json::from_value(value.clone())
            .map_err(|e| BlockJsonError::new(pack, file, field, format!("组件形状非法：{e}")))?;
        parsed
            .validate()
            .map_err(|m| BlockJsonError::new(pack, file, field, m))
    }
    match name {
        CollisionBox::NAME => {
            let boxes =
                collision_boxes_of(value).map_err(|m| BlockJsonError::new(pack, file, field, m))?;
            validate_collision_boxes(&boxes)
                .map_err(|m| BlockJsonError::new(pack, file, field, m))?;
            Ok(())
        }
        LightEmission::NAME => check::<LightEmission>(pack, file, field, value),
        LightDampening::NAME => check::<LightDampening>(pack, file, field, value),
        DestructibleByMining::NAME => check::<DestructibleByMining>(pack, file, field, value),
        Loot::NAME => check::<Loot>(pack, file, field, value),
        Replaceable::NAME => check::<Replaceable>(pack, file, field, value),
        Liquid::NAME => check::<Liquid>(pack, file, field, value),
        RandomTick::NAME => check::<RandomTick>(pack, file, field, value),
        NeedsSupport::NAME => check::<NeedsSupport>(pack, file, field, value),
        CanContainLiquid::NAME => check::<CanContainLiquid>(pack, file, field, value),
        Unbreakable::NAME => check::<Unbreakable>(pack, file, field, value),
        Mining::NAME => check::<Mining>(pack, file, field, value),
        Drops::NAME => check::<Drops>(pack, file, field, value),
        other => Err(BlockJsonError::new(
            pack,
            file,
            field,
            format!("未知组件：{other}"),
        )),
    }
}

/// Whether the name is a registered fixed component.
pub fn is_known_component(name: &str) -> bool {
    ALL_BLOCK_COMPONENTS.contains(&name)
}

/// Whether the name is an accepted-but-ignored client-side/future component.
pub fn is_ignored_component(name: &str) -> bool {
    IGNORED_COMPONENTS.contains(&name)
}

/// Read a typed component from a validated map (used to build columns and in tests).
pub fn get_block_component<T: BlockComponentSchema>(
    map: &std::collections::BTreeMap<String, Value>,
) -> Option<T> {
    map.get(T::NAME)
        .and_then(|v| serde_json::from_value::<T>(v.clone()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_covers_exactly_the_fixed_set() {
        let mut names = ALL_BLOCK_COMPONENTS.to_vec();
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
                "sc:mining",
                "sc:needs_support",
                "sc:random_tick",
                "sc:replaceable",
                "sc:unbreakable",
            ]
        );
        for name in ALL_BLOCK_COMPONENTS {
            assert!(is_known_component(name));
        }
        assert!(!is_known_component("minecraft:fly"));
        assert!(is_ignored_component("minecraft:geometry"));
        assert!(is_ignored_component("minecraft:menu_category"));
        assert!(!is_ignored_component("minecraft:collision_box"));
    }

    #[test]
    fn struct_validation_keeps_schema_strictness() {
        let ok = |name: &str, v: Value| {
            validate_block_component("p", "f", "$", name, &v).expect("应合法");
        };
        let bad = |name: &str, v: Value| {
            validate_block_component("p", "f", "$", name, &v).expect_err("应拒绝");
        };
        ok(
            "minecraft:collision_box",
            json!({"origin": [-8, 0, -8], "size": [16, 16, 16]}),
        );
        ok(
            "minecraft:collision_box",
            json!([{"origin": [-8, 0, -8], "size": [16, 4, 16]}]),
        );
        ok(
            "minecraft:collision_box",
            json!({"enabled": false, "origin": [0, 0, 0], "size": [0, 0, 0]}),
        );
        // Multiple boxes are legal (e.g. slabs/steps).
        ok(
            "minecraft:collision_box",
            json!([
                {"origin": [-8, 0, -8], "size": [16, 4, 16]},
                {"origin": [-8, 4, -8], "size": [16, 4, 16]}
            ]),
        );
        bad("minecraft:collision_box", json!([]));
        bad(
            "minecraft:collision_box",
            json!([{"origin": [-8, 0, -8], "size": [16, 4, 16]}, {"origin": [0, 0, 0]}]),
        );
        bad("minecraft:collision_box", json!({"origin": [-8, 0, -8]}));
        bad(
            "minecraft:collision_box",
            json!({"origin": [0, 0, 0], "size": [0, -1, 0]}),
        );
        bad(
            "minecraft:collision_box",
            json!({"origin": [0, 0, 0], "size": [16, 16, 16], "extra": 1}),
        );
        ok("minecraft:light_emission", json!(7));
        bad("minecraft:light_emission", json!(16));
        ok("minecraft:light_dampening", json!(15));
        ok("minecraft:destructible_by_mining", json!({"value": 2.0}));
        bad("minecraft:destructible_by_mining", json!({"value": -1.0}));
        bad("minecraft:destructible_by_mining", json!(true));
        ok("minecraft:loot", json!("loot_tables/blocks/stone.json"));
        bad("minecraft:loot", json!(""));
        ok("sc:replaceable", json!(true));
        bad("sc:replaceable", json!(1));
        ok("sc:unbreakable", json!({}));
        ok("sc:liquid", json!({"kind": "water"}));
        bad("sc:liquid", json!({"kind": ""}));
        ok("sc:random_tick", json!(false));
        ok("sc:needs_support", json!({"condition": "below"}));
        bad("sc:needs_support", json!({"condition": ""}));
        ok("sc:can_contain_liquid", json!(true));
        bad("minecraft:fly", json!({}));
    }

    #[test]
    fn mining_validation_rejects_bad_shapes_and_values() {
        let ok = |v: Value| {
            validate_block_component("p", "f", "$", "sc:mining", &v).expect("应合法");
        };
        let bad = |v: Value| {
            validate_block_component("p", "f", "$", "sc:mining", &v).expect_err("应拒绝");
        };
        ok(json!({
            "formula_version": 1,
            "base_time_seconds": 1.5,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0},
            "tools": [
                {"items": ["minecraft:wooden_pickaxe"], "can_mine": true, "harvest": true, "speed_multiplier": 2.0}
            ],
            "enchantments": {"minecraft:efficiency": {"max_level": 5}}
        }));
        // A missing base is legal (inherited from destructible, checked by sc_block).
        ok(json!({
            "formula_version": 1,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0}
        }));
        // base 0 is legal (instant mine maps to 1 tick); a missing penalty defaults to 10/3.
        ok(json!({
            "formula_version": 1,
            "base_time_seconds": 0.0,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0}
        }));
        ok(json!({
            "formula_version": 1,
            "harvest_penalty_multiplier": 3.5,
            "default": {"can_mine": true, "harvest": false, "speed_multiplier": 1.0}
        }));
        bad(json!({
            "formula_version": 1,
            "harvest_penalty_multiplier": 0.0,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0}
        }));
        bad(json!({
            "formula_version": 2,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0}
        }));
        bad(json!({
            "formula_version": 1,
            "base_time_seconds": -1.0,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0}
        }));
        // can_mine=false rejects a non-1.0 multiplier; harvest true is rejected.
        bad(json!({
            "formula_version": 1,
            "default": {"can_mine": false, "harvest": false, "speed_multiplier": 2.0}
        }));
        bad(json!({
            "formula_version": 1,
            "default": {"can_mine": false, "harvest": true, "speed_multiplier": 1.0}
        }));
        ok(json!({
            "formula_version": 1,
            "default": {"can_mine": false, "harvest": false, "speed_multiplier": 1.0}
        }));
        // Same item repeated across entries.
        bad(json!({
            "formula_version": 1,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0},
            "tools": [
                {"items": ["minecraft:iron_pickaxe"], "can_mine": true, "harvest": true, "speed_multiplier": 4.0},
                {"items": ["minecraft:iron_pickaxe"], "can_mine": true, "harvest": true, "speed_multiplier": 6.0}
            ]
        }));
        // Empty items / air / unknown enchant / tag field.
        bad(json!({
            "formula_version": 1,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0},
            "tools": [{"items": [], "can_mine": true, "harvest": true, "speed_multiplier": 1.0}]
        }));
        bad(json!({
            "formula_version": 1,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0},
            "tools": [{"items": ["minecraft:air"], "can_mine": true, "harvest": true, "speed_multiplier": 1.0}]
        }));
        bad(json!({
            "formula_version": 1,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0},
            "enchantments": {"minecraft:fortune": {"max_level": 3}}
        }));
        bad(json!({
            "formula_version": 1,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0},
            "enchantments": {"minecraft:efficiency": {"max_level": 0}}
        }));
        bad(json!({
            "formula_version": 1,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 1.0},
            "tools": [{"items": ["minecraft:stone"], "can_mine": true, "harvest": true, "speed_multiplier": 1.0, "tag": "x"}]
        }));
        bad(json!({
            "formula_version": 1,
            "default": {"can_mine": true, "harvest": true, "speed_multiplier": 0.0}
        }));
    }

    #[test]
    fn drops_validation_rejects_bad_modes_and_distributions() {
        let ok = |v: Value| {
            validate_block_component("p", "f", "$", "sc:drops", &v).expect("应合法");
        };
        let bad = |v: Value| {
            validate_block_component("p", "f", "$", "sc:drops", &v).expect_err("应拒绝");
        };
        ok(json!({
            "enabled": true, "mode": "independent",
            "entries": [{"item": "minecraft:cobblestone", "chance": 1.0, "count": [{"value": 1, "chance": 1.0}]}]
        }));
        ok(json!({
            "enabled": true, "mode": "one_of",
            "entries": [
                {"item": "minecraft:coal", "chance": 0.3, "count": [{"value": 1, "chance": 1.0}]},
                {"item": "minecraft:diamond", "chance": 0.7, "count": [{"value": 1, "chance": 1.0}]}
            ]
        }));
        ok(json!({"enabled": false, "mode": "independent", "entries": []}));
        ok(json!({"enabled": false, "mode": "independent"}));
        bad(json!({
            "enabled": true, "mode": "independent",
            "entries": [{"item": "minecraft:coal", "chance": 0.5, "count": [{"value": 1, "chance": 0.8}, {"value": 2, "chance": 0.3}]}]
        }));
        bad(json!({
            "enabled": true, "mode": "one_of",
            "entries": [
                {"item": "minecraft:a", "chance": 0.6, "count": [{"value": 1, "chance": 1.0}]},
                {"item": "minecraft:b", "chance": 0.6, "count": [{"value": 1, "chance": 1.0}]}
            ]
        }));
        bad(json!({
            "enabled": false, "mode": "independent",
            "entries": [{"item": "minecraft:cobblestone", "chance": 1.0, "count": [{"value": 1, "chance": 1.0}]}]
        }));
        bad(json!({"enabled": true, "mode": "independent", "entries": []}));
        bad(json!({
            "enabled": true, "mode": "independent",
            "entries": [{"item": "minecraft:coal", "chance": 1.5, "count": [{"value": 1, "chance": 1.0}]}]
        }));
        bad(json!({
            "enabled": true, "mode": "independent",
            "entries": [{"item": "minecraft:coal", "chance": 0.5, "count": [{"value": 0, "chance": 1.0}]}]
        }));
        // Duplicate/out-of-range fortune levels / unknown enchant keys.
        bad(json!({
            "enabled": true, "mode": "independent",
            "entries": [{
                "item": "minecraft:coal", "chance": 0.5, "count": [{"value": 1, "chance": 1.0}],
                "enchantments": {"minecraft:fortune": {"bonus_by_level": [
                    {"level": 1, "count": 1, "chance": 0.5},
                    {"level": 1, "count": 1, "chance": 0.5}
                ]}}
            }]
        }));
        bad(json!({
            "enabled": true, "mode": "independent",
            "entries": [{
                "item": "minecraft:coal", "chance": 0.5, "count": [{"value": 1, "chance": 1.0}],
                "enchantments": {"minecraft:efficiency": {"bonus_by_level": [{"level": 1, "count": 1, "chance": 0.5}]}}
            }]
        }));
        bad(json!({"enabled": true, "mode": "weighted", "entries": [
            {"item": "minecraft:coal", "chance": 1.0, "count": [{"value": 1, "chance": 1.0}]}
        ]}));
        // A missing require_harvest means true; explicit false is legal (any tool rolls).
        ok(json!({
            "enabled": true, "mode": "independent", "require_harvest": false,
            "entries": [{"item": "minecraft:torch", "chance": 1.0, "count": [{"value": 1, "chance": 1.0}]}]
        }));
        bad(json!({
            "enabled": true, "mode": "independent", "require_harvest": "yes",
            "entries": [{"item": "minecraft:torch", "chance": 1.0, "count": [{"value": 1, "chance": 1.0}]}]
        }));
        // requires: absent means unrestricted; empty/duplicate/air/unknown fields are rejected.
        ok(json!({
            "enabled": true, "mode": "independent",
            "entries": [{"item": "minecraft:grass", "chance": 1.0,
                          "count": [{"value": 1, "chance": 1.0}],
                          "requires": {"items": ["minecraft:shears"]}}]
        }));
        bad(json!({
            "enabled": true, "mode": "independent",
            "entries": [{"item": "minecraft:grass", "chance": 1.0,
                          "count": [{"value": 1, "chance": 1.0}],
                          "requires": {"items": []}}]
        }));
        bad(json!({
            "enabled": true, "mode": "independent",
            "entries": [{"item": "minecraft:grass", "chance": 1.0,
                          "count": [{"value": 1, "chance": 1.0}],
                          "requires": {"items": ["minecraft:shears", "minecraft:shears"]}}]
        }));
        bad(json!({
            "enabled": true, "mode": "independent",
            "entries": [{"item": "minecraft:grass", "chance": 1.0,
                          "count": [{"value": 1, "chance": 1.0}],
                          "requires": {"items": ["minecraft:air"]}}]
        }));
        bad(json!({
            "enabled": true, "mode": "independent",
            "entries": [{"item": "minecraft:grass", "chance": 1.0,
                          "count": [{"value": 1, "chance": 1.0}],
                          "requires": {"items": ["minecraft:shears"], "tag": "x"}}]
        }));
    }

    #[test]
    fn typed_getter_reads_validated_maps() {
        let mut map = std::collections::BTreeMap::new();
        map.insert("minecraft:light_emission".to_string(), json!(3));
        let light: Option<LightEmission> = get_block_component(&map);
        assert_eq!(light.map(|l| l.0), Some(3));
        let missing: Option<DestructibleByMining> = get_block_component(&map);
        assert!(missing.is_none());
    }
}
