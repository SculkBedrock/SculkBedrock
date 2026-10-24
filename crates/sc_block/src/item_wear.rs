//! 物品耐久损耗判定（纯函数，无 World/锁/日志）。
//!
//! Item wear decision (pure: no World access, no locks, no logging).
//!
//! 与 [`crate::mining_drops::mining_decision`] 同形：调用方把编译好的
//! [`WearProfile`] 与本次动作的 [`WearCause`] 传进来，函数只做算术。
//! 随机数由调用方以确定性种子驱动（`SplitMix64`），因此同一
//! `(OperationId, roll_kind)` 永远得到同一结果，重试可复现。
//!
//! Shaped like [`crate::mining_drops::mining_decision`]: the caller passes the
//! compiled [`WearProfile`] plus the action's [`WearCause`] and the function only
//! does arithmetic. Randomness is caller-driven with a deterministic seed
//! (`SplitMix64`), so one `(OperationId, roll_kind)` always reproduces the same
//! result across retries.

use sc_packloader::item::{DurabilityCosts, WearProfile};

/// 磨损掷骰类型：正确工具破坏方块。
///
/// Wear roll kind: breaking a block with the correct tool.
pub const ROLL_WEAR_MINE_CORRECT: u64 = 200;
/// 磨损掷骰类型：非正确工具破坏方块。
///
/// Wear roll kind: breaking a block with an incorrect tool.
pub const ROLL_WEAR_MINE_INCORRECT: u64 = 201;
/// 磨损掷骰类型：攻击实体（首版不触发）。
///
/// Wear roll kind: attacking an entity (not wired in v1).
pub const ROLL_WEAR_ATTACK: u64 = 202;
/// 磨损掷骰类型：物品使用（首版不触发）。
///
/// Wear roll kind: item use (not wired in v1).
pub const ROLL_WEAR_USE_ITEM: u64 = 203;

/// 触发磨损的动作类别。
///
/// The action that triggers wear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WearCause {
    /// 正确工具破坏方块（命中该状态的 `tools[]`）。
    ///
    /// Breaking a block with the correct tool (matches the state's `tools[]`).
    MineCorrect,
    /// 非正确工具破坏方块（走 `default` 规则）。
    ///
    /// Breaking a block with an incorrect tool (took the `default` rule).
    MineIncorrect,
    /// 攻击实体（首版不触发）。
    ///
    /// Attacking an entity (not wired in v1).
    Attack,
    /// 物品使用（首版不触发）。
    ///
    /// Item use (not wired in v1).
    UseItem,
}

impl WearCause {
    /// 该动作的掷骰类型（用于派生确定性种子，互不重叠）。
    ///
    /// Roll kind of this action (derives a deterministic seed; never shared).
    pub fn roll_kind(self) -> u64 {
        match self {
            Self::MineCorrect => ROLL_WEAR_MINE_CORRECT,
            Self::MineIncorrect => ROLL_WEAR_MINE_INCORRECT,
            Self::Attack => ROLL_WEAR_ATTACK,
            Self::UseItem => ROLL_WEAR_USE_ITEM,
        }
    }

    /// 该动作的损耗点数。
    ///
    /// Durability points this action costs.
    pub fn cost(self, costs: &DurabilityCosts) -> u32 {
        match self {
            Self::MineCorrect => costs.mine_correct,
            Self::MineIncorrect => costs.mine_incorrect,
            Self::Attack => costs.attack,
            Self::UseItem => costs.use_item,
        }
    }
}

/// 一次磨损结算的输入（按值，无锁）。
///
/// Input of one wear settlement (by value, lock-free).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WearInput {
    /// 该物品的编译态耐久配置。
    ///
    /// Compiled wear profile of the item.
    pub profile: WearProfile,
    /// 本次动作类别。
    ///
    /// Action being performed.
    pub cause: WearCause,
    /// 物品当前的已损耗值（`ItemStack.damage`）。
    ///
    /// The item's current damage (`ItemStack.damage`).
    pub current_damage: u32,
}

/// 磨损结算结果。
///
/// Result of one wear settlement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WearOutcome {
    /// 概率未命中：`damage` 不变。
    ///
    /// Chance missed: `damage` unchanged.
    Skip,
    /// 命中且未达上限：写入新的 `damage`。
    ///
    /// Hit and still below the cap: write the new `damage`.
    Wear {
        /// 新的已损耗值。
        ///
        /// New damage value.
        damage: u32,
    },
    /// 命中且达到上限：物品破损，槽位置空。
    ///
    /// Hit and reached the cap: the item breaks, clear its slot.
    Break {
        /// 写入后随即置空的已损耗值（恒等于上限，绝不超出）。
        ///
        /// Damage reached before clearing (always the cap, never beyond).
        damage: u32,
    },
}

/// 结算一次磨损。
///
/// Settles one wear event.
///
/// 抽样顺序固定：恒定消耗两次 `roll()`（先用区间选概率，再做命中判定），
/// 因此修改某物品的 `damage_chance` 不会让其它抽样位移，重放稳定。
///
/// Fixed draw order: exactly two `roll()` calls (range pick, then hit test), so
/// editing one item's `damage_chance` never shifts other draws and replays stay
/// stable.
///
/// - `input`: 结算输入
/// - `roll`: 确定性随机源，每次返回一个 `u64`
pub fn wear_decision(input: &WearInput, roll: &mut impl FnMut() -> u64) -> WearOutcome {
    let profile = &input.profile;
    if profile.max_durability == 0 {
        return WearOutcome::Skip;
    }
    let first = roll();
    let second = roll();
    let (min, max) = profile.damage_chance;
    // `min <= max` is guaranteed at compile time; `span >= 1` keeps the modulo safe.
    let span = (max as u64).saturating_sub(min as u64) + 1;
    let chance = min as u64 + first % span;
    if second % 100 >= chance {
        return WearOutcome::Skip;
    }
    let cost = input.cause.cost(&profile.costs);
    let damage = input.current_damage.saturating_add(cost);
    if damage >= profile.max_durability {
        WearOutcome::Break {
            damage: profile.max_durability,
        }
    } else {
        WearOutcome::Wear { damage }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mining_drops::SplitMix64;

    fn profile(max_durability: u32, chance: (u8, u8)) -> WearProfile {
        WearProfile {
            max_durability,
            damage_chance: chance,
            costs: DurabilityCosts::default(),
        }
    }

    fn always_roll() -> impl FnMut() -> u64 {
        || 0
    }

    #[test]
    fn no_durability_never_wears() {
        let input = WearInput {
            profile: profile(0, (100, 100)),
            cause: WearCause::MineCorrect,
            current_damage: 0,
        };
        assert_eq!(wear_decision(&input, &mut always_roll()), WearOutcome::Skip);
    }

    #[test]
    fn zero_chance_never_wears() {
        let input = WearInput {
            profile: profile(60, (0, 0)),
            cause: WearCause::MineCorrect,
            current_damage: 10,
        };
        assert_eq!(wear_decision(&input, &mut always_roll()), WearOutcome::Skip);
    }

    #[test]
    fn full_chance_costs_one_for_correct_tool() {
        let input = WearInput {
            profile: profile(60, (100, 100)),
            cause: WearCause::MineCorrect,
            current_damage: 58,
        };
        assert_eq!(
            wear_decision(&input, &mut always_roll()),
            WearOutcome::Wear { damage: 59 }
        );
    }

    #[test]
    fn incorrect_tool_costs_two_and_breaks_at_cap() {
        let input = WearInput {
            profile: profile(60, (100, 100)),
            cause: WearCause::MineIncorrect,
            current_damage: 58,
        };
        assert_eq!(
            wear_decision(&input, &mut always_roll()),
            WearOutcome::Break { damage: 60 }
        );
    }

    #[test]
    fn damage_never_exceeds_cap() {
        // 59 + 2 would be 61; the reported damage is clamped to the cap.
        let input = WearInput {
            profile: profile(60, (100, 100)),
            cause: WearCause::MineIncorrect,
            current_damage: 59,
        };
        assert_eq!(
            wear_decision(&input, &mut always_roll()),
            WearOutcome::Break { damage: 60 }
        );
    }

    #[test]
    fn half_chance_splits_on_the_hit_draw() {
        // chance = 50: hit-draw < 50 wears, >= 50 skips (second draw decides).
        let input = WearInput {
            profile: profile(60, (50, 50)),
            cause: WearCause::MineCorrect,
            current_damage: 0,
        };
        let mut hit = || 49u64;
        assert_eq!(
            wear_decision(&input, &mut hit),
            WearOutcome::Wear { damage: 1 }
        );
        let mut miss = || 50u64;
        assert_eq!(wear_decision(&input, &mut miss), WearOutcome::Skip);
    }

    #[test]
    fn same_seed_reproduces_same_outcome() {
        let input = WearInput {
            profile: profile(120, (0, 100)),
            cause: WearCause::MineIncorrect,
            current_damage: 7,
        };
        let decide = || {
            let mut rng = SplitMix64::new(0x5EED);
            wear_decision(&input, &mut move || rng.next_u64())
        };
        let first = decide();
        for _ in 0..8 {
            assert_eq!(decide(), first);
        }
    }

    #[test]
    fn roll_kinds_are_distinct() {
        let kinds = [
            WearCause::MineCorrect.roll_kind(),
            WearCause::MineIncorrect.roll_kind(),
            WearCause::Attack.roll_kind(),
            WearCause::UseItem.roll_kind(),
        ];
        for (index, kind) in kinds.iter().enumerate() {
            assert_eq!(
                kinds.iter().filter(|other| *other == kind).count(),
                1,
                "roll kind {index} collides"
            );
        }
    }
}
