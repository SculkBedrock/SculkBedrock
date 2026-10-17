//! Per-state `sc:mining` / `sc:drops` semantic compilation.
//!
//! Moved verbatim from `mining_drops.rs` (P4 split); re-exported from [`super`]
//! so existing paths keep compiling.

use super::*;
use std::collections::{BTreeMap, HashMap};
use sc_packloader::macros::ComponentKey;
use sc_packloader::block::{BlockBundleBudgets, BlockJsonError, Mining};
use sc_packloader::loot_tables::{
    Drops, COUNT_SUM_TOLERANCE, MAX_DROP_COUNT_TOTAL, ONE_OF_TOTAL_TOLERANCE,
};

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
    use sc_packloader::loot_tables::Drops as DropsSchema;
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
