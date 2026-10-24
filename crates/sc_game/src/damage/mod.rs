//! 摔落伤害域（以玩家为先）。
//!
//! 落地伤害公式为 `fallDistance - 3.255 - jumpBoost - rideReduction`，结果
//! `>0` 时造成伤害。本模块持有纯公式与落地材料门禁；`movement` 中的 owner-tick
//! 系统负责跟踪，`apply` 负责写入生命值。
//!
//! Fall damage domain (player-first).
//!
//! The landing damage formula is `fallDistance - 3.255 - jumpBoost - rideReduction`;
//! damage is dealt when the result is `>0`. This module holds the pure formula plus
//! landing-material gates; owner-tick systems in `movement` own tracking, `apply`
//! owns health writes.

pub mod apply;
pub mod fall;

pub use apply::{apply_fall_damage, decide_fall_damage, Dead, FallSettlement};

pub use fall::{
    calc_fall_damage, FallDamageInput, FallDamageOutcome, LandingMaterial, FALL_DAMAGE_OFFSET,
    FALL_LANDING_THRESHOLD,
};
