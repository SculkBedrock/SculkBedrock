//! 逐状态 `sc:mining` / 战利品表掉落语义编译。
//!
//! Per-state `sc:mining` / loot-table drops semantic compilation.
//!
//! 由 [`super`] 再导出，既有引用路径保持不变。
//!
//! Re-exported from [`super`] so existing paths keep compiling.

use super::*;
use std::collections::{BTreeMap, HashMap};
use sc_packloader::macros::ComponentKey;
use sc_packloader::block::{BlockBundleBudgets, BlockJsonError, Mining};

/// 数量分布概率和的容差（逐条目校验）。
///
/// Tolerance for count-distribution sums (validated per entry).
pub const COUNT_SUM_TOLERANCE: f64 = 1e-6;
/// 单条目编译后数量上限（约束畸形表泛滥掉落队列）。
///
/// Max compiled count per entry (bounds malformed tables flooding the drop queue).
pub const MAX_DROP_COUNT_TOTAL: u32 = 1024;

// ---------------------------------------------------------------------------
// Compile (after per-state merge; rejects the whole pack, never publishes a partial snapshot).
// ---------------------------------------------------------------------------

/// 构造一个带字段定位的编译错误。
///
/// Builds a compile error with field location.
///
/// - `pack_id` / `file` / `field`: 定位用的包 id、文件名与字段路径
/// - `detail`: 错误详情
fn bad(pack_id: &str, file: &str, field: &str, detail: String) -> BlockJsonError {
    BlockJsonError::new(pack_id, file, field, detail)
}

/// 编译逐状态 `sc:mining`（`None` 表示本状态无挖掘配置，走旧路径）。
///
/// Compiles per-state `sc:mining` (`None` means no mining for this state, legacy path applies).
///
/// - `merged` 是本状态合并后的组件映射（共享 + 全排列叠加）；
/// - `destructible` 是同状态的 `minecraft:destructible_by_mining.value`（继承源）；
/// - 缺 `base_time_seconds` 时继承 destructible；两者都缺则拒绝整个包；
/// - 值冲突时以 `base_time_seconds` 为准并告警（不拒绝）；
/// - 工具/掉落物品的存在性经 `item_exists` 检查（未知即拒绝整个包，无默认回退）。
///
/// - `merged` is the merged component map for this state (shared plus whole-permutation overlay);
/// - `destructible` is the same state's `minecraft:destructible_by_mining.value` (inheritance source);
/// - Missing `base_time_seconds` inherits destructible; missing both rejects the whole pack;
/// - Conflicting values prefer `base_time_seconds` with a warning (no rejection);
/// - Tool/drop item existence is checked via `item_exists` (unknown rejects the whole pack, no default fallback).
///
/// - `merged`: 合并后的组件 JSON 映射
/// - `destructible`: 可破坏硬度继承值
/// - `pack_id` / `file` / `key`: 定位用的包 id、文件名与状态键
/// - `budgets`: 方块 bundle 预算
/// - `item_exists`: 物品存在性判定
/// - `warnings`: 告警收集器（冲突值等信息）
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

/// 从引用的战利品表编译逐状态掉落 profile
/// （`None` 表示本状态无掉落，不做任何猜测）。
///
/// Compiles the per-state drops profile from the referenced loot table
/// (`None` means no drops for this state, nothing guessed).
///
/// 表是抽取管线预编译形态（无原版 conditions/functions）：
/// `{"pools": [{"rolls": 1, "entries": [...]}]}`，`item`/`empty` 条目携带
/// `name`、`count`（裸整数或 `[{value, chance}]` 分布）、可选 `chance`
/// （独立池）、可选 `weight`（加权 one_of 池）与可选 `requires.items`
/// 手持条件。其他形态一律显式报错：
///
/// - `rolls != 1`、未知键、未知条目类型，或一个池内混合带/不带 weight 的
///   item 条目，均被拒绝；
/// - 单条目池是独立的（逐条目掷 chance，禁止 `weight`）；多个加权 item
///   条目的池是 one_of（选择概率 = `weight / 总权重`，empty 条目计入
///   总分但什么都不掉）；
/// - 一个表内混合 one_of 与独立池会被拒绝（编译配置只携带单一模式）；
/// - `count` 分布概率和须为 1 ± [`COUNT_SUM_TOLERANCE`]，取值在
///   `1..=[MAX_DROP_COUNT_TOTAL]`。
///
/// The tables are the extraction-pipeline precompiled shape (no vanilla
/// conditions/functions): `{"pools": [{"rolls": 1, "entries": [...]}]}` with
/// `item`/`empty` entries carrying `name`, `count` (bare int or a
/// `[{value, chance}]` distribution), an optional `chance` (independent
/// pools), an optional `weight` (weighted one_of pools), and an optional
/// `requires.items` held-item condition. Anything else is a loud error:
///
/// - `rolls != 1`, unknown keys, unknown entry types, or a pool mixing
///   weighted and unweighted item entries are rejected;
/// - single-entry pools are independent (chance roll per entry, `weight`
///   forbidden); a pool with several weighted item entries is one_of
///   (selection chance = `weight / total weight`, empty entries count toward
///   the total and drop nothing);
/// - mixing one_of and independent pools in one table is rejected (the
///   compiled profile carries a single mode);
/// - `count` distributions must sum to 1 ± [`COUNT_SUM_TOLERANCE`] with values
///   in `1..=[MAX_DROP_COUNT_TOTAL]`.
///
/// - `loot_path`: 引用的战利品表路径（`None` 表示无掉落）
/// - `tables`: 包内全部战利品表
/// - `pack_id` / `file` / `key`: 定位用的包 id、文件名与状态键
/// - `budgets`: 方块 bundle 预算
/// - `item_exists`: 物品存在性判定
pub fn compile_drops_from_loot(
    loot_path: Option<&str>,
    tables: &BTreeMap<String, serde_json::Value>,
    pack_id: &str,
    file: &str,
    key: &str,
    budgets: &BlockBundleBudgets,
    item_exists: &dyn Fn(&str) -> bool,
) -> Result<Option<DropCompiled>, BlockJsonError> {
    let Some(path) = loot_path else {
        return Ok(None);
    };
    let Some(table) = tables.get(path) else {
        return Ok(None);
    };
    let field = || format!("minecraft:loot → {path}（状态 {key:?}）");
    let bad = |detail: String| BlockJsonError::new(pack_id, file, field(), detail);

    let Some(root) = table.as_object() else {
        return Err(bad("表根应为 JSON 对象".to_string()));
    };
    for k in root.keys() {
        if k != "pools" {
            return Err(bad(format!("未知顶层键 {k:?}（只允许 pools）")));
        }
    }
    let Some(pools) = root.get("pools").and_then(serde_json::Value::as_array) else {
        return Err(bad("缺 pools 数组".to_string()));
    };

    /// 一条解析出的条目（定模式前）。`item: None` 表示空备选项
    /// （什么都不掉；仅作为加权 one_of 分支有意义）。
    ///
    /// One parsed entry (pre-mode). `item: None` is an empty alternative
    /// (drops nothing; only meaningful as a weighted one_of branch).
    #[derive(Clone)]
    struct RawEntry {
        item: Option<String>,
        chance: Option<f64>,
        weight: Option<f64>,
        count: CountDistCompiled,
        requires: Vec<Box<str>>,
    }
    let parse_entries = |pool_index: usize,
                         entries: &[serde_json::Value]|
     -> Result<Vec<RawEntry>, BlockJsonError> {
        let efield = |i: usize, detail: String| {
            BlockJsonError::new(
                pack_id,
                file,
                format!("{} pools[{pool_index}].entries[{i}]", field()),
                detail,
            )
        };
        let mut out = Vec::with_capacity(entries.len());
        for (i, entry) in entries.iter().enumerate() {
            let Some(obj) = entry.as_object() else {
                return Err(efield(i, "应为 JSON 对象".to_string()));
            };
            for k in obj.keys() {
                if !matches!(k.as_str(), "type" | "name" | "count" | "chance" | "weight" | "requires") {
                    return Err(efield(i, format!("未知键 {k:?}")));
                }
            }
            let weight = match obj.get("weight") {
                None => None,
                Some(w) => {
                    let w = w.as_f64().ok_or_else(|| efield(i, "weight 必须为数值".to_string()))?;
                    if !w.is_finite() || w < 0.0 {
                        return Err(efield(i, "weight 必须为非负有限数值".to_string()));
                    }
                    Some(w)
                }
            };
            let etype = obj
                .get("type")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| efield(i, "缺 type（item|empty）".to_string()))?;
            if etype == "empty" {
                // An empty alternative drops nothing; only `weight` may qualify it.
                for k in ["name", "count", "chance", "requires"] {
                    if obj.contains_key(k) {
                        return Err(efield(i, format!("empty 条目不允许 {k:?}")));
                    }
                }
                out.push(RawEntry {
                    item: None,
                    chance: None,
                    weight,
                    count: CountDistCompiled {
                        options: vec![(1, 1.0)],
                        cumulative: vec![1.0],
                    },
                    requires: Vec::new(),
                });
                continue;
            }
            if etype != "item" {
                return Err(efield(i, format!("type 非法：{etype:?}（只允许 item|empty）")));
            }
            let item = obj
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| efield(i, "item 条目缺 name".to_string()))?;
            if item.len() > budgets.max_identifier_len {
                return Err(efield(i, format!("name 过长")));
            }
            if !item_exists(item) {
                return Err(efield(i, format!("掉落物品 {item:?} 不在当前版本包物品注册表中")));
            }
            let count = match obj.get("count") {
                None | Some(serde_json::Value::Null) => CountDistCompiled {
                    options: vec![(1, 1.0)],
                    cumulative: vec![1.0],
                },
                Some(serde_json::Value::Number(n)) => {
                    let value = n.as_u64().ok_or_else(|| {
                        efield(i, "count 必须为非负整数或 [{value, chance}] 分布".to_string())
                    })? as u32;
                    if value < 1 {
                        return Err(efield(i, "count value 必须 >=1".to_string()));
                    }
                    if value > MAX_DROP_COUNT_TOTAL {
                        return Err(efield(i, format!("count value {value} 超出上限 {MAX_DROP_COUNT_TOTAL}")));
                    }
                    CountDistCompiled {
                        options: vec![(value, 1.0)],
                        cumulative: vec![1.0],
                    }
                }
                Some(serde_json::Value::Array(options)) => {
                    if options.len() > budgets.max_count_options_per_entry {
                        return Err(efield(i, format!("count 选项数 {} 超出预算 {}", options.len(), budgets.max_count_options_per_entry)));
                    }
                    if options.is_empty() {
                        return Err(efield(i, "count 分布不能为空".to_string()));
                    }
                    let mut cum = Vec::with_capacity(options.len());
                    let mut opts = Vec::with_capacity(options.len());
                    let mut acc = 0.0;
                    let mut sum = 0.0;
                    for opt in options {
                        let value = opt.get("value").and_then(serde_json::Value::as_u64).ok_or_else(|| efield(i, "count[].value 必须为非负整数".to_string()))? as u32;
                        if value < 1 {
                            return Err(efield(i, "count value 必须 >=1".to_string()));
                        }
                        if value > MAX_DROP_COUNT_TOTAL {
                            return Err(efield(i, format!("count value {value} 超出上限 {MAX_DROP_COUNT_TOTAL}")));
                        }
                        let chance = opt.get("chance").and_then(serde_json::Value::as_f64).ok_or_else(|| efield(i, "count[].chance 必须为数值".to_string()))?;
                        if !(0.0..=1.0).contains(&chance) {
                            return Err(efield(i, format!("count[].chance {chance} 越界（0..=1）")));
                        }
                        sum += chance;
                        acc += chance;
                        cum.push(acc);
                        opts.push((value, chance));
                    }
                    if (sum - 1.0).abs() > COUNT_SUM_TOLERANCE {
                        return Err(efield(i, format!("count 分布概率和为 {sum}，要求 1.0 ± {COUNT_SUM_TOLERANCE}")));
                    }
                    CountDistCompiled { options: opts, cumulative: cum }
                }
                Some(_) => {
                    return Err(efield(i, "count 必须为非负整数或 [{value, chance}] 分布".to_string()))
                }
            };
            let chance = match obj.get("chance") {
                None => None,
                Some(c) => {
                    let c = c.as_f64().ok_or_else(|| efield(i, "chance 必须为数值".to_string()))?;
                    if !(0.0..=1.0).contains(&c) {
                        return Err(efield(i, format!("chance {c} 越界（0..=1）")));
                    }
                    Some(c)
                }
            };
            let mut requires = Vec::new();
            if let Some(rule) = obj.get("requires") {
                let items = rule
                    .get("items")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| efield(i, "requires 应为 {\"items\": [...]}".to_string()))?;
                if items.len() > budgets.max_requires_items_per_entry {
                    return Err(efield(i, format!("requires.items 数量 {} 超出预算 {}", items.len(), budgets.max_requires_items_per_entry)));
                }
                let mut seen = std::collections::HashSet::new();
                for item in items {
                    let item = item.as_str().ok_or_else(|| efield(i, "requires.items 必须为字符串".to_string()))?;
                    if !seen.insert(item) {
                        return Err(efield(i, format!("requires.items 内重复：{item:?}")));
                    }
                    if item.len() > budgets.max_identifier_len {
                        return Err(efield(i, "requires.items 过长".to_string()));
                    }
                    if !item_exists(item) {
                        return Err(efield(i, format!("requires 物品 {item:?} 不在当前版本包物品注册表中")));
                    }
                    requires.push(Box::from(item));
                }
                requires.sort();
            }
            out.push(RawEntry {
                item: Some(item.to_string()),
                chance,
                weight,
                count,
                requires,
            });
        }
        Ok(out)
    };

    // Classify each pool: weighted one_of or independent; flatten to entries
    // with their final selection chance (empty alternatives drop out here).
    /// 池分类结果：独立池或 one_of 池（条目已带最终选择概率）。
    ///
    /// Pool classification: an independent pool or a one_of pool (entries already
    /// carry their final selection chance).
    enum Pool {
        Independent(Vec<RawEntry>),
        OneOf(Vec<RawEntry>),
    }
    let mut pools_out = Vec::with_capacity(pools.len());
    for (pi, pool) in pools.iter().enumerate() {
        let Some(obj) = pool.as_object() else {
            return Err(bad(format!("pools[{pi}] 应为 JSON 对象")));
        };
        for k in obj.keys() {
            if !matches!(k.as_str(), "rolls" | "entries") {
                return Err(bad(format!("pools[{pi}] 未知键 {k:?}（只允许 rolls|entries）")));
            }
        }
        match obj.get("rolls") {
            Some(serde_json::Value::Number(n)) if n.as_u64() == Some(1) => {}
            _ => return Err(bad(format!("pools[{pi}].rolls 必须为 1（表已预展开）"))),
        }
        let Some(entries) = obj.get("entries").and_then(serde_json::Value::as_array) else {
            return Err(bad(format!("pools[{pi}] 缺 entries 数组")));
        };
        let parsed = parse_entries(pi, entries)?;
        let weighted = parsed.iter().any(|e| e.weight.is_some());
        if weighted && parsed.iter().any(|e| e.weight.is_none()) {
            return Err(bad(format!(
                "pools[{pi}] 混合带 weight 与不带 weight 的条目（不猜测缺失权重）"
            )));
        }
        if weighted {
            // one_of: selection chance = weight / total weight; empty alternatives
            // count toward the total (their share drops nothing).
            let total: f64 = parsed.iter().filter_map(|e| e.weight).sum();
            if !(total > 0.0) {
                return Err(bad(format!("pools[{pi}] weight 总和必须为正")));
            }
            let items = parsed
                .into_iter()
                .filter_map(|mut e| {
                    e.chance = Some(e.weight.unwrap_or(0.0) / total);
                    e.item.take().map(|item| RawEntry {
                        item: Some(item),
                        chance: e.chance,
                        weight: None,
                        count: e.count,
                        requires: e.requires,
                    })
                })
                .collect();
            pools_out.push(Pool::OneOf(items));
        } else if parsed.is_empty() {
            continue; // empty pool contributes nothing
        } else {
            // Independent: each item entry rolls on its own `chance` (default 1.0);
            // weight is forbidden (loud), empty alternatives drop out.
            let items = parsed
                .into_iter()
                .filter_map(|mut e| {
                    e.item.take().map(|item| RawEntry {
                        item: Some(item),
                        chance: Some(e.chance.unwrap_or(1.0)),
                        weight: None,
                        count: e.count,
                        requires: e.requires,
                    })
                })
                .collect();
            pools_out.push(Pool::Independent(items));
        }
    }

    // Single compiled mode: mixing one_of and independent pools is rejected loudly,
    // and at most ONE one_of pool exists per table (each pool rolls separately in
    // vanilla; the compiled profile carries a single flat selection, so a second
    // weighted pool would corrupt the cumulative selection).
    let one_of_pools = pools_out
        .iter()
        .filter(|p| matches!(p, Pool::OneOf(_)))
        .count();
    let has_one_of = one_of_pools > 0;
    let has_independent = pools_out
        .iter()
        .any(|p| matches!(p, Pool::Independent(_)));
    if has_one_of && has_independent {
        return Err(bad("表中混合 one_of 池与独立池（编译配置只支持单一模式）".to_string()));
    }
    if one_of_pools > 1 {
        return Err(bad(format!(
            "表含 {one_of_pools} 个 one_of 池（每个池独立掷骰，编译配置至多支持一个）"
        )));
    }
    let (mode, raw_entries) = if has_one_of {
        let mut entries = Vec::new();
        for pool in pools_out.iter() {
            if let Pool::OneOf(list) = pool {
                entries.extend(list.iter().cloned());
            }
        }
        (DropMode::OneOf, entries)
    } else {
        let mut entries = Vec::new();
        for pool in pools_out.iter() {
            if let Pool::Independent(list) = pool {
                entries.extend(list.iter().cloned());
            }
        }
        (DropMode::Independent, entries)
    };
    if raw_entries.len() > budgets.max_drop_entries_per_state {
        return Err(bad(format!(
            "编译后掉落条目数 {} 超出预算 {}",
            raw_entries.len(),
            budgets.max_drop_entries_per_state
        )));
    }
    if raw_entries.is_empty() {
        return Ok(None);
    }
    let entries = raw_entries
        .into_iter()
        .map(|e| DropEntryCompiled {
            item: Box::from(e.item.expect("空条目已在池分类时剔除").as_str()),
            chance: e.chance.expect("池分类已写入选择概率"),
            count: e.count,
            // The precompiled table shape carries no fortune rules.
            fortune: None,
            requires: e.requires,
        })
        .collect::<Vec<_>>();
    Ok(Some(DropCompiled {
        enabled: true,
        mode,
        entries,
        // The table shape carries no harvest gate; the vanilla default applies.
        require_harvest: true,
    }))
}
