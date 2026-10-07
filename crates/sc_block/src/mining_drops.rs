//! Compiled `sc:mining` / `sc:drops` snapshots plus runtime pure functions (in `sc_block`).
//!
//! Split: `sc_packloader::block::component` validates shape/range/normalization/allowlist;
//! this module validates merged per-state semantics (exclusion, inheritance, existence), compiles dense tables,
//! and computes runtime pure functions (mine time, drop rolls, deterministic RNG). The JSON parse tree stays off hot paths;
//! runtime only reads the read-only profile compiled here.
//!
//! Namespaces: `sc:mining` / `sc:drops` are the only canonical names; any `ur:*` key is rejected.

use std::collections::{BTreeMap, HashMap};

use sc_packloader::block::{
    BlockBundleBudgets, BlockComponentSchema, BlockJsonError, Drops, Mining, COUNT_SUM_TOLERANCE,
    MAX_DROP_COUNT_TOTAL, ONE_OF_TOTAL_TOLERANCE,
};

// ---------------------------------------------------------------------------
// Compiled output (densely addressed by `BlockStateId`; same length as `states` in the snapshot).
// ---------------------------------------------------------------------------

/// Single tool rule (compiled).
#[derive(Clone, Debug)]
pub struct ToolRuleCompiled {
    pub can_mine: bool,
    pub harvest: bool,
    pub speed_multiplier: f64,
}

/// Efficiency rule (compiled; bonus formula is `speed += level^2 + 1`, only the clamp stored here).
#[derive(Clone, Debug)]
pub struct EfficiencyCompiled {
    pub max_level: u32,
}

/// Per-state mining config (compiled; `None` means no `sc:mining`, legacy path applies).
#[derive(Clone, Debug)]
pub struct MiningCompiled {
    /// Parsed base time (seconds; explicit or inherited; raw hardness value, branch factor in the formula).
    pub base_seconds: f64,
    /// Whether inherited from `minecraft:destructible_by_mining` (for warning tracking).
    pub base_from_destructible: bool,
    /// Harvest penalty multiplier (rule time factor `base*penalty` when `harvest=false`; defaults to `5.0`).
    pub penalty: f64,
    pub default_rule: ToolRuleCompiled,
    /// Exact item index (deduped at build time; empty hand never consults this table).
    pub tools: HashMap<Box<str>, ToolRuleCompiled>,
    pub efficiency: Option<EfficiencyCompiled>,
    pub formula_version: u32,
}

/// Count distribution (compiled; cumulative, in array order).
#[derive(Clone, Debug)]
pub struct CountDistCompiled {
    pub options: Vec<(u32, f64)>,
    pub cumulative: Vec<f64>,
}

/// Single fortune-bonus row (compiled; ascending by level).
#[derive(Clone, Debug)]
pub struct FortuneCompiled {
    pub level: u32,
    pub count: u32,
    pub chance: f64,
}

/// Drop entry (compiled).
#[derive(Clone, Debug)]
pub struct DropEntryCompiled {
    pub item: Box<str>,
    pub chance: f64,
    pub count: CountDistCompiled,
    /// Ascending by level; `None` means no fortune rule.
    pub fortune: Option<Vec<FortuneCompiled>>,
    /// Held-item condition (sorted; empty means unrestricted, any held item rolls).
    pub requires: Vec<Box<str>>,
}

/// Drop mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropMode {
    Independent,
    OneOf,
}

/// Per-state drop config (compiled).
#[derive(Clone, Debug)]
pub struct DropCompiled {
    pub enabled: bool,
    pub mode: DropMode,
    pub entries: Vec<DropEntryCompiled>,
    /// Harvest gate (defaults to true; `true` plus unharvested takes the default empty drop).
    pub require_harvest: bool,
}

// ---------------------------------------------------------------------------
// Compile (after per-state merge; rejects the whole pack, never publishes a partial snapshot).
// ---------------------------------------------------------------------------

fn bad(pack_id: &str, file: &str, field: &str, detail: String) -> BlockJsonError {
    BlockJsonError::new(pack_id, file, field, detail)
}

/// Compiles per-state `sc:mining` (`None` means no mining for this state, legacy path applies).
///
/// - `merged` is the merged component map for this state (shared plus whole-permutation overlay);
/// - `destructible` is the same state's `minecraft:destructible_by_mining.value` (inheritance source);
/// - Missing `base_time_seconds` inherits destructible; missing both rejects the whole pack;
/// - Conflicting values prefer `base_time_seconds` with a warning (no rejection);
/// - Tool/drop item existence is checked via `item_exists` (unknown rejects the whole pack, no default fallback).
pub fn compile_mining(
    merged: &BTreeMap<String, serde_json::Value>,
    destructible: Option<f32>,
    pack_id: &str,
    file: &str,
    key: &str,
    budgets: &BlockBundleBudgets,
    item_exists: &dyn Fn(&str) -> bool,
    warnings: &mut Vec<String>,
) -> Result<Option<MiningCompiled>, BlockJsonError> {
    use sc_packloader::block::component::{DestructibleByMining, Mining as MiningSchema};
    let field = |name: &str| format!("$.minecraft:block.components.{name}（状态 {key:?}）");
    let Some(raw) = merged.get(MiningSchema::NAME) else {
        return Ok(None);
    };
    let mining: Mining = serde_json::from_value(raw.clone()).map_err(|e| {
        bad(
            pack_id,
            file,
            &field(MiningSchema::NAME),
            format!("内部不一致：{e}"),
        )
    })?;
    // Shape/range already validated in packloader; rechecks budgets and existence here (defense in depth).
    if mining.tools.len() > budgets.max_mining_tools_per_state {
        return Err(bad(
            pack_id,
            file,
            &field(MiningSchema::NAME),
            format!(
                "tools 数量 {} 超出预算 {}",
                mining.tools.len(),
                budgets.max_mining_tools_per_state
            ),
        ));
    }
    if mining.enchantments.len() > budgets.max_mining_enchants_per_state {
        return Err(bad(
            pack_id,
            file,
            &field(MiningSchema::NAME),
            format!(
                "enchantments 数量 {} 超出预算 {}",
                mining.enchantments.len(),
                budgets.max_mining_enchants_per_state
            ),
        ));
    }
    // Base resolution: explicit beats inherited beats rejection.
    let (base_seconds, base_from_destructible) = match (mining.base_time_seconds, destructible) {
        (Some(b), _) => {
            if !b.is_finite() || b < 0.0 {
                return Err(bad(
                    pack_id,
                    file,
                    &field(MiningSchema::NAME),
                    "base_time_seconds 必须为非负有限数值".to_string(),
                ));
            }
            // Warns on conflicting values (base wins; NaN/Inf/negative already rejected above).
            // Compares at f32 precision: same-source decimals differ ~1e-8 between f64/f32 parses,
            // so EPSILON would false-positive; truly different values still differ at f32.
            // Logs each message once (compiles per state, otherwise multi-state blocks would spam).
            if let Some(d) = destructible {
                if d.is_finite() && d >= 0.0 && (b as f32) != d {
                    let msg = format!(
                        "{pack_id}: {file}: sc:mining.base_time_seconds ({b}) 与 \
                         minecraft:destructible_by_mining.value ({d}) 不一致，以前者为准"
                    );
                    if !warnings.contains(&msg) {
                        warnings.push(msg);
                    }
                }
            }
            (b, false)
        }
        (None, Some(d)) => {
            if !d.is_finite() || d < 0.0 {
                return Err(bad(
                    pack_id,
                    file,
                    &field(DestructibleByMining::NAME),
                    "destructible_by_mining.value 必须为非负有限数值".to_string(),
                ));
            }
            (d as f64, true)
        }
        (None, None) => {
            return Err(bad(
                pack_id,
                file,
                &field(MiningSchema::NAME),
                "sc:mining 缺 base_time_seconds 且同状态无 minecraft:destructible_by_mining 继承源（opt-in 必须显式）".to_string(),
            ));
        }
    };
    // Tool index (deduped at build time; duplicates across entries rejected).
    let mut tools: HashMap<Box<str>, ToolRuleCompiled> = HashMap::new();
    for entry in mining.tools.iter() {
        if entry.items.len() > budgets.max_mining_items_per_rule {
            return Err(bad(
                pack_id,
                file,
                &field(MiningSchema::NAME),
                format!(
                    "tools[].items 数量 {} 超出预算 {}",
                    entry.items.len(),
                    budgets.max_mining_items_per_rule
                ),
            ));
        }
        for item in entry.items.iter() {
            if item.len() > budgets.max_identifier_len {
                return Err(bad(
                    pack_id,
                    file,
                    &field(MiningSchema::NAME),
                    format!("工具物品 {item:?} 过长"),
                ));
            }
            if !item_exists(item) {
                return Err(bad(
                    pack_id,
                    file,
                    &field(MiningSchema::NAME),
                    format!("工具物品 {item:?} 不在当前版本包物品注册表中（不回退 default）"),
                ));
            }
            let rule = ToolRuleCompiled {
                can_mine: entry.can_mine,
                harvest: entry.harvest,
                speed_multiplier: entry.speed_multiplier,
            };
            if tools.insert(Box::from(item.as_str()), rule).is_some() {
                return Err(bad(
                    pack_id,
                    file,
                    &field(MiningSchema::NAME),
                    format!("物品 {item:?} 在同一状态重复出现，不依赖数组顺序解决冲突"),
                ));
            }
        }
    }
    // Efficiency (only efficiency; unknown keys already rejected in packloader; see efficiency_bonus for the formula).
    let efficiency = match mining.enchantments.get("minecraft:efficiency") {
        None => None,
        Some(r) => Some(EfficiencyCompiled {
            max_level: r.max_level,
        }),
    };
    if mining.enchantments.len() > 1 {
        return Err(bad(
            pack_id,
            file,
            &field(MiningSchema::NAME),
            "每状态至多一条 efficiency 规则".to_string(),
        ));
    }
    // Harvest penalty multiplier (defaults to 10/3 when missing).
    let penalty = match mining.harvest_penalty_multiplier {
        Some(p) => {
            if !p.is_finite() || !(p > 0.0) {
                return Err(bad(
                    pack_id,
                    file,
                    &field(MiningSchema::NAME),
                    "harvest_penalty_multiplier 必须为正有限数值".to_string(),
                ));
            }
            p
        }
        None => sc_packloader::block::component::DEFAULT_HARVEST_PENALTY,
    };
    Ok(Some(MiningCompiled {
        base_seconds,
        base_from_destructible,
        penalty,
        default_rule: ToolRuleCompiled {
            can_mine: mining.default.can_mine,
            harvest: mining.default.harvest,
            speed_multiplier: mining.default.speed_multiplier,
        },
        tools,
        efficiency,
        formula_version: mining.formula_version,
    }))
}

/// Compiles per-state `sc:drops` (`None` means no drops for this state, no guessed drops).
pub fn compile_drops(
    merged: &BTreeMap<String, serde_json::Value>,
    pack_id: &str,
    file: &str,
    key: &str,
    budgets: &BlockBundleBudgets,
    item_exists: &dyn Fn(&str) -> bool,
) -> Result<Option<DropCompiled>, BlockJsonError> {
    use sc_packloader::block::component::Drops as DropsSchema;
    let field = |name: &str| format!("$.minecraft:block.components.{name}（状态 {key:?}）");
    let Some(raw) = merged.get(DropsSchema::NAME) else {
        return Ok(None);
    };
    let drops: Drops = serde_json::from_value(raw.clone()).map_err(|e| {
        bad(
            pack_id,
            file,
            &field(DropsSchema::NAME),
            format!("内部不一致：{e}"),
        )
    })?;
    if drops.entries.len() > budgets.max_drop_entries_per_state {
        return Err(bad(
            pack_id,
            file,
            &field(DropsSchema::NAME),
            format!(
                "entries 数量 {} 超出预算 {}",
                drops.entries.len(),
                budgets.max_drop_entries_per_state
            ),
        ));
    }
    let mode = match drops.mode.as_str() {
        "independent" => DropMode::Independent,
        "one_of" => DropMode::OneOf,
        other => {
            return Err(bad(
                pack_id,
                file,
                &field(DropsSchema::NAME),
                format!("mode 非法：{other:?}"),
            ));
        }
    };
    if !drops.enabled {
        return Ok(Some(DropCompiled {
            enabled: false,
            mode,
            entries: Vec::new(),
            require_harvest: drops.require_harvest.unwrap_or(true),
        }));
    }
    let mut entries = Vec::with_capacity(drops.entries.len());
    for (i, e) in drops.entries.iter().enumerate() {
        if e.item.len() > budgets.max_identifier_len {
            return Err(bad(
                pack_id,
                file,
                &field(DropsSchema::NAME),
                format!("entries[{i}].item 过长"),
            ));
        }
        if !item_exists(&e.item) {
            return Err(bad(
                pack_id,
                file,
                &field(DropsSchema::NAME),
                format!("掉落物品 {:?} 不在当前版本包物品注册表中", e.item),
            ));
        }
        if e.count.len() > budgets.max_count_options_per_entry {
            return Err(bad(
                pack_id,
                file,
                &field(DropsSchema::NAME),
                format!(
                    "entries[{i}].count 选项数 {} 超出预算 {}",
                    e.count.len(),
                    budgets.max_count_options_per_entry
                ),
            ));
        }
        // Rechecks normalization (validated in packloader; double-checked against bypasses).
        let sum: f64 = e.count.iter().map(|o| o.chance).sum();
        if (sum - 1.0).abs() > COUNT_SUM_TOLERANCE {
            return Err(bad(
                pack_id,
                file,
                &field(DropsSchema::NAME),
                format!("entries[{i}].count 分布概率和为 {sum}，要求 1.0 ± {COUNT_SUM_TOLERANCE}"),
            ));
        }
        let mut cumulative = Vec::with_capacity(e.count.len());
        let mut acc = 0.0;
        let mut options = Vec::with_capacity(e.count.len());
        for opt in e.count.iter() {
            if opt.value < 1 {
                return Err(bad(
                    pack_id,
                    file,
                    &field(DropsSchema::NAME),
                    format!("entries[{i}].count value 必须 >=1"),
                ));
            }
            if opt.value > MAX_DROP_COUNT_TOTAL {
                return Err(bad(
                    pack_id,
                    file,
                    &field(DropsSchema::NAME),
                    format!(
                        "entries[{i}].count value {} 超出上限 {MAX_DROP_COUNT_TOTAL}",
                        opt.value
                    ),
                ));
            }
            acc += opt.chance;
            cumulative.push(acc);
            options.push((opt.value, opt.chance));
        }
        let fortune = match e.enchantments.get("minecraft:fortune") {
            None => None,
            Some(rule) => {
                if rule.bonus_by_level.len() > budgets.max_fortune_rules_per_entry {
                    return Err(bad(
                        pack_id,
                        file,
                        &field(DropsSchema::NAME),
                        format!(
                            "entries[{i}].bonus 行数 {} 超出预算 {}",
                            rule.bonus_by_level.len(),
                            budgets.max_fortune_rules_per_entry
                        ),
                    ));
                }
                let mut sorted = rule.bonus_by_level.clone();
                sorted.sort_by_key(|b| b.level);
                let compiled = sorted
                    .into_iter()
                    .map(|b| FortuneCompiled {
                        level: b.level,
                        count: b.count,
                        chance: b.chance,
                    })
                    .collect::<Vec<_>>();
                Some(compiled)
            }
        };
        if e.enchantments.len() > 1 {
            return Err(bad(
                pack_id,
                file,
                &field(DropsSchema::NAME),
                format!("entries[{i}].enchantments 至多一条 fortune"),
            ));
        }
        let mut requires: Vec<Box<str>> = Vec::new();
        if let Some(rule) = e.requires.as_ref() {
            if rule.items.len() > budgets.max_requires_items_per_entry {
                return Err(bad(
                    pack_id,
                    file,
                    &field(DropsSchema::NAME),
                    format!(
                        "entries[{i}].requires.items 数量 {} 超出预算 {}",
                        rule.items.len(),
                        budgets.max_requires_items_per_entry
                    ),
                ));
            }
            {
                use std::collections::HashSet;
                let mut seen = HashSet::new();
                for item in rule.items.iter() {
                    if !seen.insert(item) {
                        return Err(bad(
                            pack_id,
                            file,
                            &field(DropsSchema::NAME),
                            format!("entries[{i}].requires.items 内重复：{item:?}"),
                        ));
                    }
                    if item.len() > budgets.max_identifier_len {
                        return Err(bad(
                            pack_id,
                            file,
                            &field(DropsSchema::NAME),
                            format!("entries[{i}].requires.items 过长"),
                        ));
                    }
                    if !item_exists(item) {
                        return Err(bad(
                            pack_id,
                            file,
                            &field(DropsSchema::NAME),
                            format!(
                                "entries[{i}].requires 物品 {item:?} 不在当前版本包物品注册表中"
                            ),
                        ));
                    }
                    requires.push(Box::from(item.as_str()));
                }
            }
            requires.sort();
        }
        entries.push(DropEntryCompiled {
            item: Box::from(e.item.as_str()),
            chance: e.chance,
            count: CountDistCompiled {
                options,
                cumulative,
            },
            fortune,
            requires,
        });
    }
    if mode == DropMode::OneOf {
        let total: f64 = entries.iter().map(|e| e.chance).sum();
        if total > 1.0 + ONE_OF_TOTAL_TOLERANCE {
            return Err(bad(
                pack_id,
                file,
                &field(DropsSchema::NAME),
                format!("one_of 总概率为 {total}，要求 <= 1 + {ONE_OF_TOTAL_TOLERANCE}"),
            ));
        }
    }
    Ok(Some(DropCompiled {
        enabled: true,
        mode,
        entries,
        require_harvest: drops.require_harvest.unwrap_or(true),
    }))
}

// ---------------------------------------------------------------------------
// Runtime pure functions (lock-free, no IO, no network; inputs are by-value snapshots).
// ---------------------------------------------------------------------------

/// Mining input (held-item snapshot at break request; by value, holds no inventory lock).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HandSnapshot {
    /// Held item identifier (`None` means empty hand; empty hand always takes `default`).
    pub item: Option<String>,
    /// Efficiency level (`0` means none; clamped at runtime to `min(actual, max_level)`).
    pub efficiency_level: u32,
    /// Fortune level (`0` means none; clamped at runtime to `0..=3`).
    pub fortune_level: u32,
}

/// Mining decision result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MineDecision {
    /// Mineable, needs `required_ticks` (at least 1); `harvested` feeds the drop gate
    /// (`false` plus `require_harvest` takes the default empty drop).
    Mine {
        required_ticks: i32,
        harvested: bool,
    },
    /// Survival denial (`can_mine=false`; creative uses creative semantics and ignores this result).
    Deny,
}

/// Efficiency speed bonus (`speed += level^2 + 1`).
///
/// Applies only when `allowed` (correct tool and harvested, see [`mining_decision`]) with an actual level `>0`;
/// the actual level is clamped to `min(actual, max_level)`.
pub fn efficiency_bonus(
    rule: Option<&EfficiencyCompiled>,
    actual_level: u32,
    allowed: bool,
) -> f64 {
    let Some(r) = rule else {
        return 0.0;
    };
    if !allowed || actual_level == 0 {
        return 0.0;
    }
    let level = actual_level.min(r.max_level) as f64;
    level * level + 1.0
}

/// Mining-time computation (`formula_version=1`, bitwise-consistent break-time math):
/// `time = base * branch / (tool_speed + efficiency_bonus)` →
/// `max(1, ceil(time*20))`, where `base` is the raw hardness value,
/// `branch` is `1.5` when harvested, else `harvest_penalty_multiplier` (defaults to `5.0`),
/// `efficiency_bonus = level^2 + 1` (only with the correct tool, harvested, and leveled).
///
/// - Empty hand (`hand.item=None`) uses `default` directly without consulting the `tools` index;
/// - Unmatched tools also take `default`; no wildcard/ordering semantics;
/// - Blocks with empty `tools` (no specific tool needed): any held item (including empty hand)
///   counts as the correct tool;
/// - `can_mine=false` means `Deny` (survival denial; creative bypass handled by the caller).
pub fn mining_decision(compiled: &MiningCompiled, hand: &HandSnapshot) -> MineDecision {
    use sc_packloader::block::component::HARVESTED_BRANCH_FACTOR;
    let (rule, correct) = match hand.item.as_deref() {
        None => (&compiled.default_rule, compiled.tools.is_empty()),
        Some(id) => match compiled.tools.get(id) {
            Some(r) => (r, true),
            None => (&compiled.default_rule, compiled.tools.is_empty()),
        },
    };
    if !rule.can_mine {
        return MineDecision::Deny;
    }
    let harvested = rule.harvest;
    let bonus = efficiency_bonus(
        compiled.efficiency.as_ref(),
        hand.efficiency_level,
        correct && harvested,
    );
    let speed = rule.speed_multiplier + bonus;
    if !(speed > 0.0) || !speed.is_finite() {
        return MineDecision::Deny;
    }
    // Operation order: multiply the branch factor first, then divide by speed (bitwise consistent).
    let branch = if harvested {
        HARVESTED_BRANCH_FACTOR
    } else {
        compiled.penalty
    };
    if !branch.is_finite() || !(branch > 0.0) {
        return MineDecision::Deny;
    }
    let seconds = compiled.base_seconds * branch / speed;
    if !seconds.is_finite() || seconds < 0.0 {
        return MineDecision::Deny;
    }
    let ticks = (seconds * 20.0).ceil() as i64;
    let ticks = ticks.max(1).min(i32::MAX as i64) as i32;
    MineDecision::Mine {
        required_ticks: ticks,
        harvested,
    }
}

/// Deterministic SplitMix64 (for drop rolls only; never uses `thread_rng` for drop decisions).
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform float in `[0,1)` (53-bit precision).
    pub fn next_unit(&mut self) -> f64 {
        const DIV: f64 = (1u64 << 53) as f64;
        ((self.next_u64() >> 11) as f64) / DIV
    }
}

/// Drop-roll domain (deterministic stream derived with an `OperationId`; frozen with golden tests).
///
/// `domain = "sc-drops-v1"`; inputs cover world/position/content version/operation id/entry index/roll kind.
pub fn drop_seed(
    world_id_lo: u64,
    world_id_hi: u64,
    x: i32,
    y: i32,
    z: i32,
    content_generation: u64,
    operation_id: u64,
    entry_index: u64,
    roll_kind: u64,
) -> u64 {
    fn mix(mut h: u64, v: u64) -> u64 {
        h ^= v
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(h << 6)
            .wrapping_add(h >> 2);
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^ (h >> 27)
    }
    let mut h = 0x8A6E_90E5_8FF3_2B25u64; // Fixed domain prefix for "sc-drops-v1" (frozen).
    h = mix(h, world_id_lo);
    h = mix(h, world_id_hi);
    h = mix(h, x as u64);
    h = mix(h, y as u64);
    h = mix(h, z as u64);
    h = mix(h, content_generation);
    h = mix(h, operation_id);
    h = mix(h, entry_index);
    h = mix(h, roll_kind);
    // One SplitMix64 avalanche round (same algorithm as `SplitMix64::next_u64`, preserves distribution).
    h = h.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = h;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Roll kind (the `roll_kind` of `drop_seed`).
pub const ROLL_ENTRY: u64 = 1;
pub const ROLL_COUNT: u64 = 2;
pub const ROLL_FORTUNE_BASE: u64 = 100;

/// Single drop result (item identifier plus total; callers split stacks per registry cap, total unchanged).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RolledDrop {
    pub item: Box<str>,
    pub count: u32,
}

/// Count-distribution roll (cumulates in array order, first `r < acc` wins).
pub fn roll_count(dist: &CountDistCompiled, r: f64) -> u32 {
    for (i, acc) in dist.cumulative.iter().enumerate() {
        if r < *acc {
            return dist.options[i].0;
        }
    }
    // Float-tail fallback: returns the last entry (distribution already normalized, semantics unchanged).
    dist.options.last().map(|o| o.0).unwrap_or(1)
}

/// Fortune bonus (ascending by level; `actual` clamped to `0..=3`; adds count only, never chance/type).
pub fn roll_fortune_bonus(
    rules: Option<&[FortuneCompiled]>,
    actual_fortune: u32,
    mut rand_unit: impl FnMut() -> f64,
) -> u32 {
    let Some(rules) = rules else {
        return 0;
    };
    let actual = actual_fortune.min(3);
    if actual == 0 {
        return 0;
    }
    let mut bonus = 0u32;
    for rule in rules.iter() {
        if actual < rule.level {
            continue;
        }
        if rand_unit() < rule.chance {
            bonus = bonus.saturating_add(rule.count);
        }
    }
    bonus
}

/// Drop decision (runs after commit; inputs by value; caller derives RNG from `drop_seed`).
///
/// - `independent`: rolls each entry independently, dropping 0..N entries;
/// - `one_of`: picks at most one entry (cumulates in array order; `r >= total` means empty);
/// - Harvest gate: `require_harvest=true` (default) plus `harvested=false` takes the default empty drop
///   (consumes no RNG, always empty);
/// - Entry held-item condition: entries with non-empty `requires` roll only on exact held-item match,
///   skipped otherwise (consumes no RNG; empty hand never matches);
/// - Callers split over-stacked counts, total unchanged (no splitting here, only totals returned).
pub fn roll_drops(
    compiled: &DropCompiled,
    fortune_level: u32,
    harvested: bool,
    hand: Option<&str>,
    mut rand_unit: impl FnMut(u64, u64) -> f64,
) -> Vec<RolledDrop> {
    if !compiled.enabled {
        return Vec::new();
    }
    // Default empty drop: unharvested with harvest required skips rolling and stays empty.
    if compiled.require_harvest && !harvested {
        return Vec::new();
    }
    let eligible = |entry: &DropEntryCompiled| {
        entry.requires.is_empty()
            || hand.is_some_and(|h| entry.requires.iter().any(|r| r.as_ref() == h))
    };
    match compiled.mode {
        DropMode::Independent => {
            let mut out = Vec::new();
            for (i, entry) in compiled.entries.iter().enumerate() {
                if !eligible(entry) {
                    continue;
                }
                let r_entry = rand_unit(i as u64, ROLL_ENTRY);
                if r_entry >= entry.chance {
                    continue;
                }
                let r_count = rand_unit(i as u64, ROLL_COUNT);
                let mut qty = roll_count(&entry.count, r_count);
                // Rolls each row independently (level feeds the kind, keeps results reproducible).
                let mut per_level_bonus = 0u32;
                if let Some(rules) = entry.fortune.as_deref() {
                    let actual = fortune_level.min(3);
                    for rule in rules.iter() {
                        if actual < rule.level {
                            continue;
                        }
                        let r = rand_unit(i as u64, ROLL_FORTUNE_BASE + rule.level as u64);
                        if r < rule.chance {
                            per_level_bonus = per_level_bonus.saturating_add(rule.count);
                        }
                    }
                }
                qty = qty.saturating_add(per_level_bonus);
                out.push(RolledDrop {
                    item: entry.item.clone(),
                    count: qty.max(1),
                });
            }
            out
        }
        DropMode::OneOf => {
            let total: f64 = compiled
                .entries
                .iter()
                .filter(|e| eligible(e))
                .map(|e| e.chance)
                .sum();
            // Uses entry 0 selection randomness (callers pass entry_index=u64::MAX by convention).
            let r = rand_unit(u64::MAX, ROLL_ENTRY);
            if r >= total {
                return Vec::new();
            }
            let mut acc = 0.0;
            for (i, entry) in compiled.entries.iter().enumerate() {
                if !eligible(entry) {
                    continue;
                }
                acc += entry.chance;
                if r < acc {
                    let r_count = rand_unit(i as u64, ROLL_COUNT);
                    let mut qty = roll_count(&entry.count, r_count);
                    let mut per_level_bonus = 0u32;
                    if let Some(rules) = entry.fortune.as_deref() {
                        let actual = fortune_level.min(3);
                        for rule in rules.iter() {
                            if actual < rule.level {
                                continue;
                            }
                            let rf = rand_unit(i as u64, ROLL_FORTUNE_BASE + rule.level as u64);
                            if rf < rule.chance {
                                per_level_bonus = per_level_bonus.saturating_add(rule.count);
                            }
                        }
                    }
                    qty = qty.saturating_add(per_level_bonus);
                    return vec![RolledDrop {
                        item: entry.item.clone(),
                        count: qty.max(1),
                    }];
                }
            }
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_packloader::block::component::DEFAULT_HARVEST_PENALTY;

    fn mining_fixture() -> MiningCompiled {
        let mut tools = HashMap::new();
        tools.insert(
            Box::from("minecraft:iron_pickaxe"),
            ToolRuleCompiled {
                can_mine: true,
                harvest: true,
                speed_multiplier: 5.0,
            },
        );
        tools.insert(
            Box::from("minecraft:wooden_pickaxe"),
            ToolRuleCompiled {
                can_mine: true,
                harvest: false,
                speed_multiplier: 2.0,
            },
        );
        MiningCompiled {
            base_seconds: 3.0,
            base_from_destructible: false,
            penalty: DEFAULT_HARVEST_PENALTY,
            default_rule: ToolRuleCompiled {
                can_mine: false,
                harvest: false,
                speed_multiplier: 1.0,
            },
            tools,
            efficiency: Some(EfficiencyCompiled { max_level: 5 }),
            formula_version: 1,
        }
    }

    fn hand(item: Option<&str>, eff: u32) -> HandSnapshot {
        HandSnapshot {
            item: item.map(str::to_string),
            efficiency_level: eff,
            fortune_level: 0,
        }
    }

    #[test]
    fn efficiency_bonus_follows_pnx_quadratic() {
        let max5 = EfficiencyCompiled { max_level: 5 };
        assert_eq!(efficiency_bonus(None, 5, true), 0.0);
        assert_eq!(efficiency_bonus(Some(&max5), 0, true), 0.0);
        assert_eq!(efficiency_bonus(Some(&max5), 3, false), 0.0);
        assert_eq!(efficiency_bonus(Some(&max5), 1, true), 2.0);
        assert_eq!(efficiency_bonus(Some(&max5), 3, true), 10.0);
        assert_eq!(efficiency_bonus(Some(&max5), 5, true), 26.0);
        // Clamps above max to max (level 10 counts as 5).
        assert_eq!(efficiency_bonus(Some(&max5), 10, true), 26.0);
        let max2 = EfficiencyCompiled { max_level: 2 };
        assert_eq!(efficiency_bonus(Some(&max2), 5, true), 5.0);
    }

    #[test]
    fn mining_time_matches_spec_example() {
        let m = mining_fixture();
        // Empty hand with can_mine=false is denied.
        assert_eq!(mining_decision(&m, &hand(None, 0)), MineDecision::Deny);
        // Missing diamond pickaxe falls to a non-mineable default in this fixture.
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:diamond_pickaxe"), 0)),
            MineDecision::Deny
        );
        // Unenchanted iron pickaxe: 3*1.5/5=0.9s becomes 18 ticks (harvested).
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:iron_pickaxe"), 0)),
            MineDecision::Mine {
                required_ticks: 18,
                harvested: true
            }
        );
        // Iron pickaxe Efficiency III: speed=5+(9+1)=15, 4.5/15=0.3s becomes 6 ticks.
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:iron_pickaxe"), 3)),
            MineDecision::Mine {
                required_ticks: 6,
                harvested: true
            }
        );
        // Efficiency above max clamps to max (level 10 as 5: speed=5+26=31, 4.5/31s becomes 3 ticks).
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:iron_pickaxe"), 10)),
            MineDecision::Mine {
                required_ticks: 3,
                harvested: true
            }
        );
        // Wooden pickaxe (lower tier, unharvested): penalty 3*5/2=7.5s becomes 150 ticks, harvested=false.
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:wooden_pickaxe"), 0)),
            MineDecision::Mine {
                required_ticks: 150,
                harvested: false
            }
        );
        // Efficiency does not apply when unharvested (wooden Efficiency V still 150 ticks).
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:wooden_pickaxe"), 5)),
            MineDecision::Mine {
                required_ticks: 150,
                harvested: false
            }
        );
        // base 0 becomes 1 tick (instant-mine floor).
        let mut instant = mining_fixture();
        instant.base_seconds = 0.0;
        instant.default_rule = ToolRuleCompiled {
            can_mine: true,
            harvest: true,
            speed_multiplier: 1.0,
        };
        assert_eq!(
            mining_decision(&instant, &HandSnapshot::default()),
            MineDecision::Mine {
                required_ticks: 1,
                harvested: true
            }
        );
    }

    #[test]
    fn unharvested_hand_gets_default_empty_drop() {
        let compiled = DropCompiled {
            enabled: true,
            mode: DropMode::Independent,
            entries: vec![DropEntryCompiled {
                item: Box::from("minecraft:cobblestone"),
                chance: 1.0,
                count: CountDistCompiled {
                    options: vec![(1, 1.0)],
                    cumulative: vec![1.0],
                },
                fortune: None,
                requires: vec![],
            }],
            require_harvest: true,
        };
        // Unharvested consumes no RNG (calling the closure panics), always empty.
        let drops = roll_drops(&compiled, 0, false, None, |_, _| {
            panic!("unharvested must not roll")
        });
        assert!(drops.is_empty());
        // Harvested rolls normally.
        let drops = roll_drops(&compiled, 0, true, None, |_, _| 0.0);
        assert_eq!(drops.len(), 1);
        // With require_harvest=false, unharvested still rolls.
        let mut open = compiled.clone();
        open.require_harvest = false;
        let drops = roll_drops(&open, 0, false, None, |_, _| 0.0);
        assert_eq!(drops.len(), 1);
    }

    #[test]
    fn requires_filters_entries_without_consuming_rng() {
        let entry = |item: &str, requires: Vec<Box<str>>| DropEntryCompiled {
            item: Box::from(item),
            chance: 1.0,
            count: CountDistCompiled {
                options: vec![(1, 1.0)],
                cumulative: vec![1.0],
            },
            fortune: None,
            requires,
        };
        let compiled = DropCompiled {
            enabled: true,
            mode: DropMode::Independent,
            entries: vec![
                entry("minecraft:grass", vec![Box::from("minecraft:shears")]),
                entry("minecraft:wheat_seeds", vec![]),
            ],
            require_harvest: false,
        };
        // Empty hand: the self entry is skipped (consumes no RNG; calling the closure would panic.
        // The seed entry here has no requires, so it still rolls with fixed randomness).
        let drops = roll_drops(&compiled, 0, true, None, |_, _| 0.0);
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item.as_ref(), "minecraft:wheat_seeds");
        // Shears: both entries hit.
        let drops = roll_drops(&compiled, 0, true, Some("minecraft:shears"), |_, _| 0.0);
        assert_eq!(drops.len(), 2);
        // Stick (unlisted): seed only.
        let drops = roll_drops(&compiled, 0, true, Some("minecraft:stick"), |_, _| 0.0);
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item.as_ref(), "minecraft:wheat_seeds");
    }

    #[test]
    fn drop_seed_is_deterministic_and_spread() {
        let a = drop_seed(1, 2, 3, 64, 5, 7, 9, 0, ROLL_ENTRY);
        let b = drop_seed(1, 2, 3, 64, 5, 7, 9, 0, ROLL_ENTRY);
        assert_eq!(a, b);
        let c = drop_seed(1, 2, 3, 64, 5, 7, 9, 1, ROLL_ENTRY);
        assert_ne!(a, c);
        let d = drop_seed(1, 2, 3, 64, 5, 7, 9, 0, ROLL_COUNT);
        assert_ne!(a, d);
    }

    #[test]
    fn independent_and_one_of_follow_spec() {
        let compiled = DropCompiled {
            enabled: true,
            mode: DropMode::Independent,
            require_harvest: true,
            entries: vec![
                DropEntryCompiled {
                    item: Box::from("minecraft:cobblestone"),
                    chance: 1.0,
                    count: CountDistCompiled {
                        options: vec![(1, 1.0)],
                        cumulative: vec![1.0],
                    },
                    fortune: None,
                    requires: vec![],
                },
                DropEntryCompiled {
                    item: Box::from("minecraft:coal"),
                    chance: 0.25,
                    count: CountDistCompiled {
                        options: vec![(1, 0.75), (2, 0.25)],
                        cumulative: vec![0.75, 1.0],
                    },
                    fortune: None,
                    requires: vec![],
                },
            ],
        };
        // Entry 0 with chance=1 always drops; entry 1 rolls.
        let drops = roll_drops(&compiled, 0, true, None, |idx, kind| match (idx, kind) {
            (0, ROLL_ENTRY) => 0.5, // <1 passes
            (1, ROLL_ENTRY) => 0.1, // <0.25 passes
            (1, ROLL_COUNT) => 0.8, // >=0.75 picks value 2
            _ => 0.0,
        });
        assert_eq!(drops.len(), 2);
        assert_eq!(drops[0].item.as_ref(), "minecraft:cobblestone");
        assert_eq!(drops[0].count, 1);
        assert_eq!(drops[1].count, 2);

        let one_of = DropCompiled {
            enabled: false,
            mode: DropMode::OneOf,
            entries: vec![],
            require_harvest: true,
        };
        assert!(roll_drops(&one_of, 0, true, None, |_, _| 0.0).is_empty());

        let one_of = DropCompiled {
            enabled: true,
            mode: DropMode::OneOf,
            require_harvest: true,
            entries: vec![
                DropEntryCompiled {
                    item: Box::from("minecraft:a"),
                    chance: 0.3,
                    count: CountDistCompiled {
                        options: vec![(1, 1.0)],
                        cumulative: vec![1.0],
                    },
                    fortune: None,
                    requires: vec![],
                },
                DropEntryCompiled {
                    item: Box::from("minecraft:b"),
                    chance: 0.7,
                    count: CountDistCompiled {
                        options: vec![(1, 1.0)],
                        cumulative: vec![1.0],
                    },
                    fortune: None,
                    requires: vec![],
                },
            ],
        };
        // r=0.2 <0.3 picks a; r=0.5 picks b; r=0.99 picks b; total=1 always drops one.
        for (r, expect) in [
            (0.2, "minecraft:a"),
            (0.5, "minecraft:b"),
            (0.99, "minecraft:b"),
        ] {
            let got = roll_drops(&one_of, 0, true, None, |idx, kind| {
                if idx == u64::MAX && kind == ROLL_ENTRY {
                    return r;
                }
                0.0
            });
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].item.as_ref(), expect);
        }
        // total<1 leaves the rest empty.
        let sparse = DropCompiled {
            enabled: true,
            mode: DropMode::OneOf,
            require_harvest: true,
            entries: vec![DropEntryCompiled {
                item: Box::from("minecraft:a"),
                chance: 0.3,
                count: CountDistCompiled {
                    options: vec![(1, 1.0)],
                    cumulative: vec![1.0],
                },
                fortune: None,
                requires: vec![],
            }],
        };
        let empty = roll_drops(&sparse, 0, true, None, |idx, kind| {
            if idx == u64::MAX && kind == ROLL_ENTRY {
                return 0.5; // >=0.3 empty
            }
            0.0
        });
        assert!(empty.is_empty());
    }

    #[test]
    fn fortune_only_adds_count() {
        let entry = DropEntryCompiled {
            item: Box::from("minecraft:coal"),
            chance: 1.0,
            count: CountDistCompiled {
                options: vec![(1, 1.0)],
                cumulative: vec![1.0],
            },
            fortune: Some(vec![
                FortuneCompiled {
                    level: 1,
                    count: 1,
                    chance: 0.5,
                },
                FortuneCompiled {
                    level: 3,
                    count: 2,
                    chance: 1.0,
                },
            ]),
            requires: vec![],
        };
        let compiled = DropCompiled {
            enabled: true,
            mode: DropMode::Independent,
            require_harvest: true,
            entries: vec![entry],
        };
        // Fortune 0 means no bonus.
        let d0 = roll_drops(&compiled, 0, true, None, |_, _| 0.0);
        assert_eq!(d0[0].count, 1);
        // Fortune 1 passes the level-1 roll for +1, skips level 3 (actual<level).
        let d1 = roll_drops(&compiled, 1, true, None, |_, kind| {
            if kind == ROLL_COUNT || kind == ROLL_ENTRY {
                0.0
            } else {
                0.4
            }
        });
        assert_eq!(d1[0].count, 2);
        // Fortune 3 passes both rows (0.4<0.5 and 0.4<1.0): 1+1+2=4.
        let d3 = roll_drops(&compiled, 3, true, None, |_, _| 0.4);
        assert_eq!(d3[0].count, 4);
        // Fortune 10 clamps to 3.
        let d10 = roll_drops(&compiled, 10, true, None, |_, _| 0.4);
        assert_eq!(d10[0].count, 4);
    }
}
