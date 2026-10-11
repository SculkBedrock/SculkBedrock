//! Fall damage domain (player-first).
//!
//! Reference behavior (`fallDistance - 3.255 - jumpBoost - rideReduction`,
//! `>0` damages). This module holds the pure formula plus landing-material gates;
//! owner-tick systems in `movement` own tracking, `apply` owns health writes.

pub mod apply;
pub mod fall;

pub use apply::{apply_fall_damage, decide_fall_damage, Dead, FallSettlement};

pub use fall::{
    calc_fall_damage, FallDamageInput, FallDamageOutcome, LandingMaterial, FALL_DAMAGE_OFFSET,
    FALL_LANDING_THRESHOLD,
};
