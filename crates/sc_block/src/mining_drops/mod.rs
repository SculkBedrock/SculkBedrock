//! sc:mining 与战利品表掉落的编译快照及运行期纯函数。
//!
//! 校验逐状态语义后编译稠密表，运行期提供挖掘时长、掉落掷骰与确定性随机数。
//!
//! Compiled sc:mining and loot-table drop snapshots with runtime pure functions.
//!
//! Validates per-state semantics, compiles dense tables, and provides mining time, drop rolls, and deterministic RNG at runtime.

use std::collections::HashMap;

pub mod compile;
pub mod roll;

pub use compile::{compile_drops_from_loot, compile_mining};
pub use roll::{
    drop_seed, efficiency_bonus, mining_decision, roll_count, roll_drops, roll_fortune_bonus,
    HandSnapshot, MineDecision, RolledDrop, SplitMix64, ROLL_COUNT, ROLL_ENTRY,
    ROLL_FORTUNE_BASE,
};

// ---------------------------------------------------------------------------
// Compiled output (densely addressed by `BlockStateId`; same length as `states` in the snapshot).
// ---------------------------------------------------------------------------

/// 单条工具规则（编译产物）。
///
/// Single tool rule (compiled).
#[derive(Clone, Debug)]
pub struct ToolRuleCompiled {
    pub can_mine: bool,
    pub harvest: bool,
    pub speed_multiplier: f64,
}

/// 效率附魔规则（编译产物；加成公式为 `speed += level^2 + 1`，此处只存上限）。
///
/// Efficiency rule (compiled; bonus formula is `speed += level^2 + 1`, only the clamp stored here).
#[derive(Clone, Debug)]
pub struct EfficiencyCompiled {
    pub max_level: u32,
}

/// 逐状态挖掘配置（编译产物；`None` 表示无 `sc:mining`，走旧路径）。
///
/// Per-state mining config (compiled; `None` means no `sc:mining`, legacy path applies).
#[derive(Clone, Debug)]
pub struct MiningCompiled {
    /// 解析出的基础时间（秒；显式或继承；即原始硬度值，公式中作被除数）。
    ///
    /// Parsed base time (seconds; explicit or inherited; raw hardness value, branch factor in the formula).
    pub base_seconds: f64,
    /// 是否继承自 `minecraft:destructible_by_mining`（用于告警跟踪）。
    ///
    /// Whether inherited from `minecraft:destructible_by_mining` (for warning tracking).
    pub base_from_destructible: bool,
    /// 未收割惩罚倍率（`harvest=false` 时规则时长为 `base*penalty`；默认 `5.0`）。
    ///
    /// Harvest penalty multiplier (rule time factor `base*penalty` when `harvest=false`; defaults to `5.0`).
    pub penalty: f64,
    pub default_rule: ToolRuleCompiled,
    /// 精确物品索引（构建期去重；空手从不查此表）。
    ///
    /// Exact item index (deduped at build time; empty hand never consults this table).
    pub tools: HashMap<Box<str>, ToolRuleCompiled>,
    pub efficiency: Option<EfficiencyCompiled>,
    pub formula_version: u32,
}

/// 数量分布（编译产物；累积式，按数组顺序）。
///
/// Count distribution (compiled; cumulative, in array order).
#[derive(Clone, Debug)]
pub struct CountDistCompiled {
    pub options: Vec<(u32, f64)>,
    pub cumulative: Vec<f64>,
}

/// 单行时运加成（编译产物；按等级升序）。
///
/// Single fortune-bonus row (compiled; ascending by level).
#[derive(Clone, Debug)]
pub struct FortuneCompiled {
    pub level: u32,
    pub count: u32,
    pub chance: f64,
}

/// 掉落条目（编译产物）。
///
/// Drop entry (compiled).
#[derive(Clone, Debug)]
pub struct DropEntryCompiled {
    pub item: Box<str>,
    pub chance: f64,
    pub count: CountDistCompiled,
    /// 按等级升序；`None` 表示无时运规则。
    ///
    /// Ascending by level; `None` means no fortune rule.
    pub fortune: Option<Vec<FortuneCompiled>>,
    /// 手持物品条件（已排序；空表示不限制，任意手持物均可掷）。
    ///
    /// Held-item condition (sorted; empty means unrestricted, any held item rolls).
    pub requires: Vec<Box<str>>,
}

/// 掉落模式。
///
/// Drop mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropMode {
    Independent,
    OneOf,
}

/// 逐状态掉落配置（编译产物）。
///
/// Per-state drop config (compiled).
#[derive(Clone, Debug)]
pub struct DropCompiled {
    pub enabled: bool,
    pub mode: DropMode,
    pub entries: Vec<DropEntryCompiled>,
    /// 收割门（默认 true；为 true 且未收割时走默认空掉落）。
    ///
    /// Harvest gate (defaults to true; `true` plus unharvested takes the default empty drop).
    pub require_harvest: bool,
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
    fn efficiency_bonus_follows_quadratic() {
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
                harvested: true,
                correct_tool: true
            }
        );
        // Iron pickaxe Efficiency III: speed=5+(9+1)=15, 4.5/15=0.3s becomes 6 ticks.
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:iron_pickaxe"), 3)),
            MineDecision::Mine {
                required_ticks: 6,
                harvested: true,
                correct_tool: true
            }
        );
        // Efficiency above max clamps to max (level 10 as 5: speed=5+26=31, 4.5/31s becomes 3 ticks).
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:iron_pickaxe"), 10)),
            MineDecision::Mine {
                required_ticks: 3,
                harvested: true,
                correct_tool: true
            }
        );
        // Wooden pickaxe (lower tier, unharvested): penalty 3*5/2=7.5s becomes 150 ticks, harvested=false.
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:wooden_pickaxe"), 0)),
            MineDecision::Mine {
                required_ticks: 150,
                harvested: false,
                correct_tool: true
            }
        );
        // Efficiency does not apply when unharvested (wooden Efficiency V still 150 ticks).
        assert_eq!(
            mining_decision(&m, &hand(Some("minecraft:wooden_pickaxe"), 5)),
            MineDecision::Mine {
                required_ticks: 150,
                harvested: false,
                correct_tool: true
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
                harvested: true,
                correct_tool: false
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