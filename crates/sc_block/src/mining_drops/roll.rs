//! Runtime pure functions: mine time, drop rolls, deterministic RNG.
//!
//! Moved verbatim from `mining_drops.rs` (P4 split); re-exported from [`super`]
//! so existing paths keep compiling.

use super::*;

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
