//! 纯摔落伤害公式（不访问 World/ECS，可完整单元测试）。
//!
//! Pure fall-damage formula (no World/ECS access, fully unit-testable).
//!
//! 基线：`damage = fallDistance - 3.255 - jumpBoost - rideReduction`。
//! Baseline: `damage = fallDistance - 3.255 - jumpBoost - rideReduction`.
//! 公式之前按优先级应用落地门禁：
//! Landing gates applied before the formula, in priority order:
//! 水 > 黏液块（玩家且未潜行）> 游戏模式/飞行/游戏规则免疫。
//! water > slime (player, not sneaking) > gamemode/flight/gamerule immunity.

/// 摔落偏移：前 3.255 格内不受伤害。
///
/// Fall-damage offset: no damage within the first 3.255 blocks.
pub const FALL_DAMAGE_OFFSET: f32 = 3.255;
/// 低于该结算距离只产生方块副作用，绝不造成伤害。
///
/// Below this settled distance only block side effects apply, never damage.
pub const FALL_LANDING_THRESHOLD: f32 = 0.75;

/// 结算位置的落地材料分类。
///
/// Landing material classification for the settled position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandingMaterial {
    /// 普通落地。
    ///
    /// Ordinary landing.
    Normal,
    /// 水中。
    ///
    /// In water.
    Water,
    /// 黏液块上。
    ///
    /// On a slime block.
    Slime,
}

/// 摔落伤害判定的输入。
///
/// Inputs to the fall-damage decision.
#[derive(Clone, Copy, Debug)]
pub struct FallDamageInput {
    /// 摔落距离。
    ///
    /// Fall distance.
    pub fall_distance: f32,
    /// 落地材料。
    ///
    /// Landing material.
    pub landing: LandingMaterial,
    /// 玩家正在潜行（黏液块需要未潜行才能缓冲）。
    ///
    /// Player is sneaking (slime requires non-sneaking to cushion).
    pub sneaking: bool,
    /// 创造/旁观/无敌。
    ///
    /// Creative / spectator / invulnerable.
    pub immune_gamemode: bool,
    /// 飞行能力免疫（创造式 `Player::attack` 的 FALL 免疫）。
    ///
    /// Flight-ability immunity (creative-style `Player::attack` FALL
    /// immunity).
    pub can_fly: bool,
    /// 世界 `fallDamage` 游戏规则（仅玩家；生物忽略它）。
    ///
    /// World `fallDamage` gamerule (players only; mobs ignore it).
    pub gamerule_enabled: bool,
    /// 是否为玩家（游戏规则只门禁玩家）。
    ///
    /// Whether this entity is a player (gamerule only gates players).
    pub is_player: bool,
    /// 存在 `SLOW_FALLING` 式效果。
    ///
    /// `SLOW_FALLING`-style effect present.
    pub has_slow_falling: bool,
    /// 跳跃增益等级（当前恒为 0）。
    ///
    /// Jump-boost levels (always 0 today).
    pub jump_boost: f32,
    /// 骑乘跳跃减免（当前恒为 0）。
    ///
    /// Ride-jump reduction (always 0 today).
    pub ride_reduction: f32,
}

/// 摔落判定的结果。
///
/// Outcome of the fall decision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FallDamageOutcome {
    /// 无伤害。
    ///
    /// No damage.
    NoDamage,
    /// 造成指定数值的伤害。
    ///
    /// Deal the given amount of damage.
    Damage(f32),
}

/// 纯判定：先免疫门禁，再偏移公式。
///
/// Pure decision: immunity gates first, then the offset formula.
///
/// - `input`: 判定输入
pub fn calc_fall_damage(input: FallDamageInput) -> FallDamageOutcome {
    if !input.fall_distance.is_finite() || input.fall_distance <= 0.0 {
        return FallDamageOutcome::NoDamage;
    }
    if input.has_slow_falling {
        return FallDamageOutcome::NoDamage;
    }
    if input.immune_gamemode || input.can_fly {
        return FallDamageOutcome::NoDamage;
    }
    if input.is_player && !input.gamerule_enabled {
        return FallDamageOutcome::NoDamage;
    }
    match input.landing {
        LandingMaterial::Water => return FallDamageOutcome::NoDamage,
        LandingMaterial::Slime => {
            if !input.sneaking {
                return FallDamageOutcome::NoDamage;
            }
        }
        LandingMaterial::Normal => {}
    }
    if input.fall_distance <= FALL_LANDING_THRESHOLD {
        return FallDamageOutcome::NoDamage;
    }
    let damage = input.fall_distance - FALL_DAMAGE_OFFSET - input.jump_boost - input.ride_reduction;
    if damage.is_finite() && damage > 0.0 {
        FallDamageOutcome::Damage(damage)
    } else {
        FallDamageOutcome::NoDamage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(distance: f32) -> FallDamageInput {
        FallDamageInput {
            fall_distance: distance,
            landing: LandingMaterial::Normal,
            sneaking: false,
            immune_gamemode: false,
            can_fly: false,
            gamerule_enabled: true,
            is_player: true,
            has_slow_falling: false,
            jump_boost: 0.0,
            ride_reduction: 0.0,
        }
    }

    #[test]
    fn below_offset_takes_no_damage() {
        assert_eq!(calc_fall_damage(base(3.0)), FallDamageOutcome::NoDamage);
        assert_eq!(
            calc_fall_damage(base(FALL_DAMAGE_OFFSET)),
            FallDamageOutcome::NoDamage
        );
    }

    #[test]
    fn above_offset_damages_by_excess() {
        assert_eq!(
            calc_fall_damage(base(4.0)),
            FallDamageOutcome::Damage(4.0 - FALL_DAMAGE_OFFSET)
        );
        assert_eq!(
            calc_fall_damage(base(10.0)),
            FallDamageOutcome::Damage(10.0 - FALL_DAMAGE_OFFSET)
        );
    }

    #[test]
    fn water_and_slime_cushion() {
        let mut water = base(10.0);
        water.landing = LandingMaterial::Water;
        assert_eq!(calc_fall_damage(water), FallDamageOutcome::NoDamage);

        let mut slime = base(10.0);
        slime.landing = LandingMaterial::Slime;
        assert_eq!(calc_fall_damage(slime), FallDamageOutcome::NoDamage);

        // Sneaking on slime keeps damage (`!isSneaking` gate).
        slime.sneaking = true;
        assert!(matches!(
            calc_fall_damage(slime),
            FallDamageOutcome::Damage(_)
        ));
    }

    #[test]
    fn immunity_gates_win() {
        for mutator in [
            |i: &mut FallDamageInput| i.immune_gamemode = true,
            |i: &mut FallDamageInput| i.can_fly = true,
            |i: &mut FallDamageInput| i.has_slow_falling = true,
        ] {
            let mut input = base(10.0);
            mutator(&mut input);
            assert_eq!(calc_fall_damage(input), FallDamageOutcome::NoDamage);
        }
        // Gamerule off protects players only.
        let mut player = base(10.0);
        player.gamerule_enabled = false;
        assert_eq!(calc_fall_damage(player), FallDamageOutcome::NoDamage);
        let mut mob = player;
        mob.is_player = false;
        assert!(matches!(
            calc_fall_damage(mob),
            FallDamageOutcome::Damage(_)
        ));
    }

    #[test]
    fn non_finite_or_negative_distance_never_damages() {
        for distance in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            assert_eq!(
                calc_fall_damage(base(distance)),
                FallDamageOutcome::NoDamage
            );
        }
    }
}
