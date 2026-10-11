//! Pure fall-damage formula (no World/ECS access, fully unit-testable).
//!
//! Baseline: `damage = fallDistance - 3.255 - jumpBoost - rideReduction`.
//! Landing gates applied before the formula, in priority order:
//! water > slime (player, not sneaking) > gamemode/flight/gamerule immunity.

/// Fall-damage offset: no damage within the first 3.255 blocks.
pub const FALL_DAMAGE_OFFSET: f32 = 3.255;
/// Below this settled distance only block side effects apply, never damage.
pub const FALL_LANDING_THRESHOLD: f32 = 0.75;

/// Landing material classification for the settled position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandingMaterial {
    Normal,
    Water,
    Slime,
}

/// Inputs to the fall-damage decision.
#[derive(Clone, Copy, Debug)]
pub struct FallDamageInput {
    pub fall_distance: f32,
    pub landing: LandingMaterial,
    /// Player is sneaking (slime requires non-sneaking to cushion).
    pub sneaking: bool,
    /// Creative / spectator / invulnerable.
    pub immune_gamemode: bool,
    /// Flight-ability immunity (creative-style `Player::attack` FALL immunity).
    pub can_fly: bool,
    /// World `fallDamage` gamerule (players only; mobs ignore it).
    pub gamerule_enabled: bool,
    /// Whether this entity is a player (gamerule only gates players).
    pub is_player: bool,
    /// `SLOW_FALLING`-style effect present.
    pub has_slow_falling: bool,
    /// Jump-boost levels (reserved; 0 until the effect system lands).
    pub jump_boost: f32,
    /// Ride-jump reduction (reserved; 0 until rideable entities land).
    pub ride_reduction: f32,
}

/// Outcome of the fall decision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FallDamageOutcome {
    NoDamage,
    Damage(f32),
}

/// Pure decision: immunity gates first, then the offset formula.
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
