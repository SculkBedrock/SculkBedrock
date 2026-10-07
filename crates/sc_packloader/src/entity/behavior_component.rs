use crate::types::entity_types::MinecraftEntityTypes;
use crate::types::event::MinecraftEvent;
use crate::types::filter::MinecraftFilterType;
use crate::types::range::MinecraftRangeType;
use crate::types::trigger::MinecraftTrigger;
use sc_ecs::component::Component;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

/*
 warning: All code below was AI-generated and has not been verified for correctness
 Official docs:
 https://learn.microsoft.com/en-us/minecraft/crea/aigoaltor/reference/content/entityreference/exampleslist?view=minecraft-bedrock-stable
*/
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorAdmireItem {
    #[serde(default)]
    pub admire_item_sound: String,
    #[serde(default)]
    pub sound_interval: MinecraftRangeType<f64>,
    #[serde(default)]
    pub on_admire_item_start: MinecraftEvent,
    #[serde(default)]
    pub on_admire_item_stop: MinecraftEvent,
    #[serde(default)]
    pub priority: i32,
}
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorAvoidBlock {
    #[serde(default)]
    pub avoid_block_sound: String,
    #[serde(default)]
    pub on_escape: MinecraftEvent,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub search_height: i32,
    #[serde(default)]
    pub search_range: i32,
    #[serde_inline_default(MinecraftRangeType::Vec([3.0, 8.0]))]
    pub sound_interval: MinecraftRangeType<f64>,
    #[serde_inline_default(1.0)]
    pub sprint_speed_modifier: f64,
    #[serde(default)]
    pub target_blocks: Vec<String>,
    #[serde(default)]
    pub target_selection_method: String,
    #[serde_inline_default(1)]
    pub tick_interval: i32,
    #[serde_inline_default(1.0)]
    pub walk_speed_modifier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorAvoidMobType {
    #[serde(default)]
    pub avoid_mob_sound: String,
    #[serde_inline_default(16)]
    pub avoid_target_xz: i32,
    #[serde_inline_default(7)]
    pub avoid_target_y: i32,
    #[serde(default)]
    pub entity_types: MinecraftFilterType,
    #[serde_inline_default(false)]
    pub ignore_visibility: bool,
    #[serde_inline_default(3.0)]
    pub max_dist: f64,
    #[serde_inline_default(10.0)]
    pub max_flee: f64,
    #[serde(default)]
    pub on_escape_event: MinecraftTrigger,
    #[serde_inline_default(1)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub probability_per_strength: f64,
    #[serde_inline_default(false)]
    pub remove_target: bool,
    #[serde_inline_default(MinecraftRangeType::Vec([3.0, 8.0]))]
    pub sound_interval: MinecraftRangeType<f64>,
    #[serde_inline_default(7.0)]
    pub sprint_distance: f64,
    #[serde_inline_default(1.0)]
    pub sprint_speed_multiplier: f64,
    #[serde_inline_default(1.0)]
    pub walk_speed_multiplier: f64,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorBarter {
    #[serde(default)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorBeg {
    #[serde(default)]
    pub items: Vec<String>,
    #[serde_inline_default(8.0)]
    pub look_distance: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([2.0, 4.0]))]
    pub look_time: MinecraftRangeType<f64>,
    #[serde(default)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorBreed {
    #[serde(default)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorCelebrate {
    #[serde(default)]
    pub celebration_sound: String,
    #[serde_inline_default(30.0)]
    pub duration: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([1.0, 3.5]))]
    pub jump_interval: MinecraftRangeType<f64>,
    #[serde(default)]
    pub on_celebration_end_event: MinecraftTrigger,
    #[serde_inline_default(1)]
    pub priority: i32,
    #[serde_inline_default(MinecraftRangeType::Vec([2.0, 7.0]))]
    pub sound_interval: MinecraftRangeType<f64>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorCelebrateSurvive {
    #[serde(default)]
    pub celebration_sound: String,
    #[serde_inline_default(30.0)]
    pub duration: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([1.0, 3.5]))]
    pub jump_interval: MinecraftRangeType<f64>,
    #[serde(default)]
    pub on_celebration_end_event: MinecraftTrigger,
    #[serde_inline_default(1)]
    pub priority: i32,
    #[serde_inline_default(MinecraftRangeType::Vec([2.0, 7.0]))]
    pub sound_interval: MinecraftRangeType<f64>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorChargeAttack {
    #[serde_inline_default(3.0)]
    pub max_distance: f64,
    #[serde_inline_default(2.0)]
    pub min_distance: f64,
    #[serde(default)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(0.1428)]
    pub success_rate: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorChargeHeldItem {
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorCircleAroundAnchor {
    #[serde_inline_default(15.0)]
    pub angle_change: f64,
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([0.0, 0.0]))]
    pub height_above_target_range: MinecraftRangeType<f64>,
    #[serde_inline_default(0.002857)]
    pub height_adjustment_chance: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([0.0, 0.0]))]
    pub height_offset_range: MinecraftRangeType<f64>,
    #[serde(default)]
    pub priority: i32,
    #[serde_inline_default(0.004)]
    pub radius_adjustment_chance: f64,
    #[serde_inline_default(1.0)]
    pub radius_change: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([5.0, 15.0]))]
    pub radius_range: MinecraftRangeType<f64>,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorControlledByPlayer {
    #[serde_inline_default(0.5)]
    pub fractional_rotation: f64,
    #[serde_inline_default(5.0)]
    pub fractional_rotation_limit: f64,
    #[serde_inline_default(1.0)]
    pub mount_speed_multiplier: f64,
    #[serde(default)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorCroak {
    #[serde_inline_default(MinecraftRangeType::Vec([4.5, 4.5]))]
    pub duration: MinecraftRangeType<f64>,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(MinecraftRangeType::Vec([10.0, 20.0]))]
    pub interval: MinecraftRangeType<f64>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDefendTrustedTarget {
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde(default)]
    pub aggro_sound: String,
    #[serde_inline_default(0)]
    pub attack_interval: i32,
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde_inline_default(3.0)]
    pub must_see_forget_duration: f64,
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    #[serde(default)]
    pub on_defend_start: MinecraftEvent,
    #[serde(default)]
    pub priority: i32,
    #[serde_inline_default(0.05)]
    pub sound_chance: f64,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDefendVillageTarget {
    #[serde_inline_default(0.05)]
    pub attack_chance: f64,
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(true)]
    pub must_reach: bool,
    #[serde(default)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDelayedAttack {
    #[serde_inline_default(0.75)]
    pub attack_duration: f64,
    #[serde_inline_default(false)]
    pub attack_once: bool,
    #[serde(default)]
    pub attack_types: Vec<String>,
    #[serde_inline_default(0.5)]
    pub hit_delay_pct: f64,
    #[serde_inline_default(0.25)]
    pub inner_boundary_time_increase: f64,
    #[serde(default)]
    pub max_dist: f64,
    #[serde_inline_default(0.55)]
    pub max_path_time: f64,
    #[serde_inline_default(90.0)]
    pub melee_fov: f64,
    #[serde_inline_default(0.2)]
    pub min_path_time: f64,
    #[serde(default)]
    pub on_attack: MinecraftEvent,
    #[serde_inline_default(0.5)]
    pub outer_boundary_time_increase: f64,
    #[serde_inline_default(0.75)]
    pub path_fail_time_increase: f64,
    #[serde_inline_default(16.0)]
    pub path_inner_boundary: f64,
    #[serde_inline_default(32.0)]
    pub path_outer_boundary: f64,
    #[serde(default)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub random_stop_interval: i32,
    #[serde_inline_default(1.5)]
    pub reach_multiplier: f64,
    #[serde_inline_default(false)]
    pub require_complete_path: bool,
    #[serde_inline_default(false)]
    pub set_persistent: bool,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde(default)]
    pub target_dist: f64,
    #[serde_inline_default(true)]
    pub track_target: bool,
    #[serde_inline_default(30.0)]
    pub x_max_rotation: f64,
    #[serde_inline_default(30.0)]
    pub y_max_head_rotation: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDig {
    #[serde_inline_default(false)]
    pub allow_dig_when_named: bool,
    #[serde_inline_default(false)]
    pub digs_in_daylight: bool,
    #[serde_inline_default(0.0)]
    pub duration: f64,
    #[serde_inline_default(0.0)]
    pub idle_time: f64,
    #[serde(default)]
    pub on_start: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(false)]
    pub suspicion_is_disturbance: bool,
    #[serde_inline_default(false)]
    pub vibration_is_disturbance: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDrinkMilk {
    #[serde_inline_default(5.0)]
    pub cooldown_seconds: f64,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonChargePlayer {
    #[serde_inline_default(3.0)]
    pub active_speed: f64,
    #[serde_inline_default(0.5)]
    pub continue_charge_threshold_time: f64,
    #[serde_inline_default(0.6)]
    pub flight_speed: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(MinecraftRangeType::Vec([10.0, 150.0]))]
    pub target_zone: MinecraftRangeType<f64>,
    #[serde_inline_default(0.7)]
    pub turn_speed: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragondeath {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonflaming {
    #[serde_inline_default(10.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(0.5)]
    pub flame_time: f64,
    #[serde_inline_default(4)]
    pub ground_flame_count: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(2.0)]
    pub roar_time: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonHoldingPattern {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonLanding {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonScanning {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonStrafePlayer {
    #[serde_inline_default(0.6)]
    pub active_speed: f64,
    #[serde_inline_default(64.0)]
    pub fireball_range: f64,
    #[serde_inline_default(0.6)]
    pub flight_speed: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.125)]
    pub switch_direction_probability: f64,
    #[serde_inline_default(0.25)]
    pub target_in_range_and_in_view_time: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([10.0, 150.0]))]
    pub target_zone: MinecraftRangeType<f64>,
    #[serde_inline_default(0.7)]
    pub turn_speed: f64,
    #[serde_inline_default(10.0)]
    pub view_angle: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonTakeoff {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDrinkPotionPotion {
    #[serde_inline_default(1.0)]
    pub chance: f64,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(-1)]
    pub id: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDrinkPotion {
    #[serde(default)]
    pub potions: Vec<BehaviorDrinkPotionPotion>,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub speed_modifier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDropItemFor {
    #[serde_inline_default(0.2)]
    pub cooldown: f64,
    #[serde_inline_default(1.0)]
    pub drop_item_chance: f64,
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde(default)]
    pub loot_table: String,
    #[serde_inline_default(10.0)]
    pub max_head_look_at_height: f64,
    #[serde_inline_default(2.0)]
    pub minimum_teleport_distance: f64,
    #[serde_inline_default(1.0)]
    pub offering_distance: f64,
    #[serde(default)]
    pub on_drop_attempt: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub search_count: i32,
    #[serde_inline_default(1)]
    pub search_height: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(0.0)]
    pub seconds_before_pickup: f64,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default([0.0, 1.0, 0.0])]
    pub target_range: [f64; 3],
    #[serde_inline_default([0.0, 1.0, 0.0])]
    pub teleport_offset: [f64; 3],
    #[serde_inline_default(MinecraftRangeType::Vec([0.0, 1.0]))]
    pub time_of_day_range: MinecraftRangeType<f64>,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct BehaviorEatBlockEatAndReplaceBlockPair {
    #[serde(default)]
    pub eat_block: String,
    #[serde(default)]
    pub replace_block: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEatBlock {
    #[serde(default)]
    pub eat_and_replace_block_pairs: Vec<BehaviorEatBlockEatAndReplaceBlockPair>,
    #[serde(default)]
    pub on_eat: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.02)]
    pub success_chance: f64,
    #[serde_inline_default(1.8)]
    pub time_until_eat: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEatCarriedItem {
    #[serde_inline_default(0.0)]
    pub delay_before_eating: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEatMob {
    #[serde_inline_default(1.0)]
    pub eat_animation_time: f64,
    #[serde(default)]
    pub eat_mob_sound: String,
    #[serde(default)]
    pub loot_table: String,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub pull_in_force: f64,
    #[serde_inline_default(1.0)]
    pub reach_mob_distance: f64,
    #[serde_inline_default(1.0)]
    pub run_speed: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEmerge {
    #[serde_inline_default(0.5)]
    pub cooldown_time: f64,
    #[serde_inline_default(5.0)]
    pub duration: f64,
    #[serde(default)]
    pub on_done: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEndermanLeaveBlock {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEndermanTakeBlock {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEquipItem {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorExploreOutskirts {
    #[serde_inline_default([5.0, 0.0, 5.0])]
    pub dist_from_boundary: [f64; 3],
    #[serde_inline_default(5.0)]
    pub explore_dist: f64,
    #[serde_inline_default(60.0)]
    pub max_travel_time: f64,
    #[serde_inline_default(0.0)]
    pub max_wait_time: f64,
    #[serde_inline_default(2.2)]
    pub min_dist_from_target: f64,
    #[serde_inline_default(1.0)]
    pub min_perimeter: f64,
    #[serde_inline_default(3.0)]
    pub min_wait_time: f64,
    #[serde_inline_default(5)]
    pub next_xz: i32,
    #[serde_inline_default(3)]
    pub next_y: i32,
    #[serde_inline_default(1)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(2.0)]
    pub timer_ratio: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFertilizeFarmBlock {
    #[serde_inline_default(1.5)]
    pub goal_radius: f64,
    #[serde_inline_default(1)]
    pub max_fertilizer_usage: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(8.0)]
    pub search_cooldown_max_seconds: f64,
    #[serde_inline_default(9)]
    pub search_count: i32,
    #[serde_inline_default(1)]
    pub search_height: i32,
    #[serde_inline_default(1)]
    pub search_range: i32,
    #[serde_inline_default(0.5)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFindCover {
    #[serde_inline_default(0.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFindMount {
    #[serde_inline_default(false)]
    pub avoid_water: bool,
    #[serde_inline_default(-1.0)]
    pub mount_distance: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub start_delay: i32,
    #[serde_inline_default(false)]
    pub target_needed: bool,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFindUnderwaterTreasure {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(2.0)]
    pub stop_distance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFleeSun {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFloat {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(false)]
    pub sink_with_passengers: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFloatWander {
    #[serde_inline_default(MinecraftRangeType::Vec([0.0, 0.0]))]
    pub float_duration: MinecraftRangeType<f64>,
    #[serde_inline_default(false)]
    pub must_reach: bool,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(false)]
    pub random_reselect: bool,
    #[serde_inline_default(10)]
    pub xz_dist: i32,
    #[serde_inline_default(7)]
    pub y_dist: i32,
    #[serde_inline_default(0.0)]
    pub y_offset: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFollowCaravan {
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(1)]
    pub entity_count: i32,
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(false)]
    pub filters: bool,
    #[serde_inline_default(16.0)]
    pub max_dist: f64,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde_inline_default(3.0)]
    pub must_see_forget_duration: f64,
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    #[serde_inline_default(1.0)]
    pub sprint_speed_multiplier: f64,
    #[serde_inline_default(1.0)]
    pub walk_speed_multiplier: f64,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFollowMob {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(2.0)]
    pub stop_distance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFollowOwner {
    #[serde_inline_default(true)]
    pub can_teleport: bool,
    #[serde_inline_default(true)]
    pub ignore_vibration: bool,
    #[serde_inline_default(60.0)]
    pub max_distance: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(10.0)]
    pub start_distance: f64,
    #[serde_inline_default(2.0)]
    pub stop_distance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFollowParent {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFollowTargetCaptain {
    #[serde_inline_default(0.0)]
    pub follow_distance: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorGoAndGiveItemsToNoteblock {
    #[serde_inline_default(0)]
    pub listen_time: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(3.0)]
    pub reach_block_distance: f64,
    #[serde_inline_default(1.0)]
    pub run_speed: f64,
    #[serde_inline_default(0.2)]
    pub throw_force: f64,
    #[serde(default)]
    pub throw_sound: String,
    #[serde_inline_default(1.5)]
    pub vertical_throw_mul: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorGoAndGiveItemsToOwner {
    #[serde_inline_default(0)]
    pub listen_time: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(3.0)]
    pub reach_block_distance: f64,
    #[serde_inline_default(1.0)]
    pubrun_speed: f64,
    #[serde_inline_default(0.2)]
    pub throw_force: f64,
    #[serde(default)]
    pub throw_sound: String,
    #[serde_inline_default(1.5)]
    pub vertical_throw_mul: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorGoHome {
    #[serde_inline_default(2.0)]
    pub calculate_new_path_radius: f64,
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(120)]
    pub interval: i32,
    #[serde(default)]
    pub on_failed: MinecraftTrigger,
    #[serde(default)]
    pub on_home: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorGuardianAttack {
    #[serde_inline_default(2)]
    pub elder_extra_magic_damage: i32,
    #[serde_inline_default(2)]
    pub hard_mode_extra_magic_damage: i32,
    #[serde_inline_default(1)]
    pub magic_damage: i32,
    #[serde_inline_default(3.0)]
    pub min_distance: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.5)]
    pub sound_delay_time: f64,
    #[serde_inline_default(90.0)]
    pub x_max_rotation: f64,
    #[serde_inline_default(90.0)]
    pub y_max_head_rotation: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorHarvestFarmBlock {
    #[serde_inline_default(1.5)]
    pub goal_radius: f64,
    #[serde_inline_default(1.0)]
    pub max_seconds_before_search: f64,
    #[serde_inline_default(8.0)]
    pub search_cooldown_max_seconds: f64,
    #[serde_inline_default(0)]
    pub search_count: i32,
    #[serde_inline_default(1)]
    pub search_height: i32,
    #[serde_inline_default(16)]
    pub search_range: i32,
    #[serde_inline_default(0.5)]
    pub seconds_until_new_task: f64,
    #[serde_inline_default(0.5)]
    pub speed_multiplier: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorHide {
    #[serde_inline_default(1.0)]
    pub duration: f64,
    #[serde(default)]
    pub poi_type: String,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(8.0)]
    pub timeout_cooldown: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorHoldGround {
    #[serde_inline_default(false)]
    pub broadcast: bool,
    #[serde_inline_default(0.0)]
    pub broadcast_range: f64,
    #[serde_inline_default(10.0)]
    pub min_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde(default)]
    pub within_radius_event: MinecraftTrigger,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorHurtByTarget {
    #[serde_inline_default(false)]
    pub alert_same_type: bool,
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde_inline_default(16.0)]
    pub max_dist: f64,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde_inline_default(3.0)]
    pub must_see_forget_duration: f64,
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    #[serde_inline_default(1.0)]
    pub sprint_speed_multiplier: f64,
    #[serde_inline_default(1.0)]
    pub walk_speed_multiplier: f64,
    #[serde_inline_default(false)]
    pub hurt_owner: bool,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorInspectBookshelf {
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(10)]
    pub search_count: i32,
    #[serde_inline_default(1)]
    pub search_height: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorInvestigateSuspiciousLocation {
    #[serde_inline_default(1.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorJumpToBlock {
    #[serde_inline_default(MinecraftRangeType::Vec([10.0, 20.0]))]
    pub cooldown_range: MinecraftRangeType<f64>,
    #[serde(default)]
    pub forbidden_blocks: Vec<String>,
    #[serde_inline_default(1.5)]
    pub max_velocity: f64,
    #[serde_inline_default(2)]
    pub minimum_distance: i32,
    #[serde_inline_default(5)]
    pub minimum_path_length: i32,
    #[serde(default)]
    pub preferred_blocks: Vec<String>,
    #[serde_inline_default(1.0)]
    pub preferred_blocks_chance: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.7)]
    pub scale_factor: f64,
    #[serde_inline_default(10)]
    pub search_height: i32,
    #[serde_inline_default(8)]
    pub search_width: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorKnockbackRoar {
    #[serde_inline_default(0.5)]
    pub attack_time: f64,
    #[serde_inline_default(0.1)]
    pub cooldown_time: f64,
    #[serde(default)]
    pub damage_filters: MinecraftFilterType,
    #[serde_inline_default(1.0)]
    pub duration: f64,
    #[serde_inline_default(6)]
    pub knockback_damage: i32,
    #[serde(default)]
    pub knockback_filters: MinecraftFilterType,
    #[serde_inline_default(0.4)]
    pub knockback_height_cap: f64,
    #[serde_inline_default(4)]
    pub knockback_horizontal_strength: i32,
    #[serde_inline_default(4)]
    pub knockback_range: i32,
    #[serde_inline_default(4)]
    pub knockback_vertical_strength: i32,
    #[serde(default)]
    pub on_roar_end: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorLayDown {
    #[serde_inline_default(120)]
    pub interval: i32,
    #[serde_inline_default(120)]
    pub random_stop_interval: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorLayEgg {
    #[serde_inline_default(false)]
    pub allow_laying_from_below: bool,
    #[serde_inline_default("minecraft:turtle_egg".to_string())]
    pub egg_type: String,
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default("lay_egg".to_string())]
    pub lay_egg_sound: String,
    #[serde_inline_default(10.0)]
    pub lay_seconds: f64,
    #[serde(default)]
    pub on_lay: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1)]
    pub search_height: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(vec!["minecraft:sand".to_string()])]
    pub target_blocks: Vec<String>,
    #[serde_inline_default(vec!["Air".to_string()])]
    pub target_materials_above_block: Vec<String>,
    #[serde_inline_default(true)]
    pub use_default_animation: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorLeapAtTarget {
    #[serde_inline_default(true)]
    pub must_be_on_ground: bool,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(true)]
    pub set_persistent: bool,
    #[serde_inline_default(0.3)]
    pub target_dist: f64,
    #[serde_inline_default(0.0)]
    pub yd: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorLookAtEntity {
    #[serde_inline_default(360)]
    pub angle_of_view_horizontal: i32,
    #[serde_inline_default(360)]
    pub angle_of_view_vertical: i32,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(8.0)]
    pub look_distance: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([2, 4]))]
    pub look_time: MinecraftRangeType<i32>,
    #[serde_inline_default(0.02)]
    pub probability: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorLookAtPlayer {
    #[serde_inline_default(360)]
    pub angle_of_view_horizontal: i32,
    #[serde_inline_default(360)]
    pub angle_of_view_vertical: i32,
    #[serde_inline_default(8.0)]
    pub look_distance: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([2.0, 4.0]))]
    pub look_time: MinecraftRangeType<f64>,
    #[serde_inline_default(0.02)]
    pub probability: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(6.0)]
    pub target_distance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorLookAtTarget {
    #[serde_inline_default(360)]
    pub angle_of_view_horizontal: i32,
    #[serde_inline_default(360)]
    pub angle_of_view_vertical: i32,
    #[serde_inline_default(8.0)]
    pub look_distance: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([2.0, 4.0]))]
    pub look_time: MinecraftRangeType<f64>,
    #[serde_inline_default(0.02)]
    pub probability: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorLookAtTradingPlayer {
    #[serde_inline_default(360)]
    pub angle_of_view_horizontal: i32,
    #[serde_inline_default(360)]
    pub angle_of_view_vertical: i32,
    #[serde_inline_default(8.0)]
    pub look_distance: f64,
    #[serde_inline_default(MinecraftRangeType::Vec([2.0, 4.0]))]
    pub look_time: MinecraftRangeType<f64>,
    #[serde_inline_default(0.02)]
    pub probability: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMakeLove {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMeleeAttack {
    #[serde_inline_default(false)]
    pub attack_once: bool,
    #[serde(default)]
    pub attack_types: String,
    #[serde_inline_default(1.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(0.25)]
    pub inner_boundary_time_increase: f64,
    #[serde_inline_default(0.0)]
    pub max_dist: f64,
    #[serde_inline_default(0.55)]
    pub max_path_time: f64,
    #[serde_inline_default(90.0)]
    pub melee_fov: f64,
    #[serde_inline_default(0.2)]
    pub min_path_time: f64,
    #[serde(default)]
    pub on_attack: MinecraftTrigger,
    #[serde(default)]
    pub on_kill: MinecraftTrigger,
    #[serde_inline_default(0.5)]
    pub outer_boundary_time_increase: f64,
    #[serde_inline_default(0.75)]
    pub path_fail_time_increase: f64,
    #[serde_inline_default(16.0)]
    pub path_inner_boundary: f64,
    #[serde_inline_default(32.0)]
    pub path_outer_boundary: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub random_stop_interval: i32,
    #[serde_inline_default(2.0)]
    pub reach_multiplier: f64,
    #[serde_inline_default(false)]
    pub require_complete_path: bool,
    #[serde_inline_default(false)]
    pub set_persistent: bool,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(0.0)]
    pub target_dist: f64,
    #[serde_inline_default(false)]
    pub track_target: bool,
    #[serde_inline_default(30.0)]
    pub x_max_rotation: f64,
    #[serde_inline_default(30.0)]
    pub y_max_head_rotation: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMeleeBoxAttack {
    #[serde_inline_default(false)]
    pub attack_once: bool,
    #[serde(default)]
    pub attack_types: String,
    #[serde_inline_default(1.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(0.25)]
    pub inner_boundary_time_increase: f64,
    #[serde_inline_default(0.0)]
    pub max_dist: f64,
    #[serde_inline_default(0.55)]
    pub max_path_time: f64,
    #[serde_inline_default(90.0)]
    pub melee_fov: f64,
    #[serde_inline_default(0.2)]
    pub min_path_time: f64,
    #[serde(default)]
    pub on_attack: MinecraftTrigger,
    #[serde(default)]
    pub on_kill: MinecraftTrigger,
    #[serde_inline_default(0.5)]
    pub outer_boundary_time_increase: f64,
    #[serde_inline_default(0.75)]
    pub path_fail_time_increase: f64,
    #[serde_inline_default(16.0)]
    pub path_inner_boundary: f64,
    #[serde_inline_default(32.0)]
    pub path_outer_boundary: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub random_stop_interval: i32,
    #[serde_inline_default(2.0)]
    pub reach_multiplier: f64,
    #[serde_inline_default(false)]
    pub require_complete_path: bool,
    #[serde_inline_default(false)]
    pub set_persistent: bool,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(0.0)]
    pub target_dist: f64,
    #[serde_inline_default(false)]
    pub track_target: bool,
    #[serde_inline_default(30.0)]
    pub x_max_rotation: f64,
    #[serde_inline_default(30.0)]
    pub y_max_head_rotation: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMingle {
    #[serde_inline_default(0.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(1.0)]
    pub duration: f64,
    #[serde_inline_default(2.0)]
    pub mingle_distance: f64,
    #[serde(default)]
    pub mingle_partner_type: String,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMountPathing {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(0.0)]
    pub target_dist: f64,
    #[serde_inline_default(false)]
    pub track_target: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveIndoors {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.800000)]
    pub speed_multiplier: f64,
    #[serde_inline_default(8.000000)]
    pub timeout_cooldown: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveOutdoors {
    #[serde_inline_default(0.500000)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.000000)]
    pub search_count: f64,
    #[serde_inline_default(0.000000)]
    pub search_height: f64,
    #[serde_inline_default(0.000000)]
    pub search_range: f64,
    #[serde_inline_default(0.500000)]
    pub speed_multiplier: f64,
    #[serde_inline_default(8.000000)]
    pub timeout_cooldown: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveThroughVillage {
    #[serde_inline_default(false)]
    pub only_at_night: bool,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveToBlock {
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde(default)]
    pub on_reach: MinecraftTrigger,
    #[serde(default)]
    pub on_stay_completed: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub search_height: f64,
    #[serde_inline_default(0.0)]
    pub search_range: f64,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(1.0)]
    pub start_chance: f64,
    #[serde_inline_default(0.0)]
    pub stay_duration: f64,
    #[serde(default)]
    pub target_blocks: Vec<String>,
    #[serde_inline_default([0.0, 0.0, 0.0])]
    pub target_offset: [f64; 3],
    #[serde_inline_default("nearest".to_string())]
    pub target_selection_method: String,
    #[serde_inline_default(20)]
    pub tick_interval: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveToLand {
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub search_height: f64,
    #[serde_inline_default(0.0)]
    pub search_range: f64,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveToLiquid {
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde(default)]
    pub material_type: String,
    #[serde_inline_default(10)]
    pub search_count: i32,
    #[serde_inline_default(1)]
    pub search_height: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveToRandomBlock {
    #[serde_inline_default(16.0)]
    pub block_distance: f64,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(-1)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveToVillage {
    #[serde_inline_default(0.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveToWater {
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(10)]
    pub search_count: i32,
    #[serde_inline_default(1)]
    pub search_height: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveTowardsDwellingRestriction {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveTowardsHomeRestriction {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveTowardsTarget {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorNap {
    #[serde_inline_default(0.0)]
    pub cooldown_max: f64,
    #[serde_inline_default(0.0)]
    pub cooldown_min: f64,
    #[serde_inline_default(6.0)]
    pub mob_detect_dist: f64,
    #[serde_inline_default(6.0)]
    pub mob_detect_height: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorNearestAttackableTarget {
    #[serde_inline_default(0)]
    pub attack_interval: i32,
    #[serde_inline_default(0)]
    pub attack_interval_min: i32,
    #[serde_inline_default(false)]
    pub attack_owner: bool,
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(false)]
    pub must_reach: bool,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde_inline_default(false)]
    pub must_forget_duration: bool,
    #[serde_inline_default(0.0)]
    pub persist_time: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    #[serde_inline_default(false)]
    pub reselect_targets: bool,
    #[serde_inline_default(10)]
    pub scan_interval: i32,
    #[serde_inline_default(false)]
    pub set_persistent: bool,
    #[serde_inline_default(0.70)]
    pub target_invisible_multiplier: f64,
    #[serde_inline_default(-1.0)]
    pub target_search_height: f64,
    #[serde_inline_default(0.80)]
    pub target_sneak_visibility_multiplier: f64,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorNearestPrioritizedAttackableTarget {
    #[serde_inline_default(0)]
    pub attack_interval: i32,
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde_inline_default(false)]
    pub must_reach: bool,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde_inline_default(3.0)]
    pub must_see_forget_duration: f64,
    #[serde_inline_default(0.0)]
    pub persist_time: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    #[serde_inline_default(false)]
    pub reselect_targets: bool,
    #[serde_inline_default(10)]
    pub scan_interval: i32,
    #[serde_inline_default(false)]
    pub set_persistent: bool,
    #[serde_inline_default(-1.0)]
    pub target_search_height: f64,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorOcelotSitOnBlock {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorOcelotAttack {
    #[serde_inline_default(1.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(15.0)]
    pub max_distance: f64,
    #[serde_inline_default(15.0)]
    pub max_sneak_range: f64,
    #[serde_inline_default(4.0)]
    pub max_sprint_range: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(2.0)]
    pub reach_multiplier: f64,
    #[serde_inline_default(0.6)]
    pub sneak_speed_multiplier: f64,
    #[serde_inline_default(1.33)]
    pub sprint_speed_multiplier: f64,
    #[serde_inline_default(0.8)]
    pub walk_speed_multiplier: f64,
    #[serde_inline_default(30.0)]
    pub x_max_rotation: f64,
    #[serde_inline_default(30.0)]
    pub y_max_head_rotation: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorOfferFlower {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorOpenDoor {
    #[serde_inline_default(true)]
    pub close_door_after: bool,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorOwnerHurtByTarget {
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(16.0)]
    pub max_dist: f64,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde_inline_default(3.0)]
    pub must_see_forget_duration: f64,
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    #[serde_inline_default(1.0)]
    pub sprint_speed_multiplier: f64,
    #[serde_inline_default(1.0)]
    pub walk_speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorOwnerHurtTarget {
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(16.0)]
    pub max_dist: f64,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde_inline_default(3.0)]
    pub must_see_forget_duration: f64,
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    #[serde_inline_default(1.0)]
    pub sprint_speed_multiplier: f64,
    #[serde_inline_default(1.0)]
    pub walk_speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorPanic {
    #[serde(default)]
    pub damage_sources: Vec<String>,
    #[serde_inline_default(false)]
    pub force: bool,
    #[serde_inline_default(false)]
    pub ignore_mob_damage: bool,
    #[serde_inline_default(false)]
    pub prefer_water: bool,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub sound_interval: f64,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorPeek {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorPetSleepWithOwner {
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1)]
    pub search_height: i32,
    #[serde_inline_default(10)]
    pub search_radius: i32,
    #[serde_inline_default(10)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorPickupItems {
    #[serde_inline_default(false)]
    pub can_pickup_any_item: bool,
    #[serde_inline_default(true)]
    pub can_pickup_to_hand_or_equipment: bool,
    #[serde_inline_default(true)]
    pub pickup_same_items_as_in_hand: bool,
    #[serde_inline_default(20.0)]
    pub cooldown_after_being_attacked: f64,
    #[serde(default)]
    pub excluded_items: Vec<String>,
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0.0)]
    pub max_dist: f64,
    #[serde_inline_default(false)]
    pub pickup_based_on_chance: bool,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(false)]
    pub track_target: bool,
    #[serde(default)]
    pub search_height: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorPlay {
    #[serde_inline_default(0.0)]
    pub chance_to_start: f64,
    #[serde_inline_default(2)]
    pub follow_distance: i32,
    #[serde_inline_default([6.0, 3.0, 6.0])]
    pub friend_search_area: [f64; 3],
    #[serde(default)]
    pub friend_types: MinecraftEntityTypes,
    #[serde_inline_default(50.0)]
    pub max_play_duration_seconds: f64,
    #[serde_inline_default(3)]
    pub random_pos_search_height: i32,
    #[serde_inline_default(16)]
    pub random_pos_search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorPlayDead {
    #[serde_inline_default(true)]
    pub apply_regeneration: bool,
    #[serde(default)]
    pub damage_sources: Vec<String>,
    #[serde_inline_default(1.0)]
    pub duration: f64,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(0)]
    pub force_below_health: i32,
    #[serde(default)]
    pub random_damage_range: MinecraftRangeType<f64>,
    #[serde_inline_default(1.0)]
    pub random_start_chance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorPlayerRideTamed {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRaidGarden {
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde_inline_default(2)]
    pub eat_delay: i32,
    #[serde_inline_default(100)]
    pub full_delay: i32,
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub initial_eat_delay: i32,
    #[serde_inline_default(6)]
    pub max_to_eat: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub search_height: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRamAttack {
    #[serde_inline_default(0.333333)]
    pub baby_knockback_modifier: f64,
    #[serde(default)]
    pub cooldown_range: MinecraftRangeType<f64>,
    #[serde_inline_default(5.0)]
    pub knockback_force: f64,
    #[serde_inline_default(0.1)]
    pub knockback_height: f64,
    #[serde_inline_default(0.0)]
    pub min_ram_distance: f64,
    #[serde(default)]
    pub on_start: MinecraftTrigger,
    #[serde(default)]
    pub pre_ram_sound: String,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub ram_distance: f64,
    #[serde(default)]
    pub ram_impact_sound: String,
    #[serde_inline_default(2.0)]
    pub ram_speed: f64,
    #[serde_inline_default(1.0)]
    pub run_speed: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRandomBreach {
    #[serde_inline_default(0.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(120)]
    pub interval: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(10)]
    pub xz_dist: i32,
    #[serde_inline_default(7)]
    pub y_dist: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRandomFly {
    #[serde_inline_default(true)]
    pub can_land_on_trees: bool,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(10)]
    pub xz_dist: i32,
    #[serde_inline_default(7)]
    pub y_dist: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRandomHover {
    #[serde(default)]
    pub hover_height: MinecraftRangeType<f64>,
    #[serde_inline_default(120)]
    pub interval: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(10)]
    pub xz_dist: i32,
    #[serde_inline_default(7)]
    pub y_dist: i32,
    #[serde_inline_default(0.0)]
    pub y_offset: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRandomLookAroundAndSit {
    #[serde_inline_default(false)]
    pub continue_if_leashed: bool,
    #[serde_inline_default(false)]
    pub continue_sitting_on_reload: bool,
    #[serde_inline_default(30.0)]
    pub max_angle_of_view_horizontal: f64,
    #[serde_inline_default(2)]
    pub max_look_count: i32,
    #[serde_inline_default(40)]
    pub max_look_time: i32,
    #[serde_inline_default(-30.0)]
    pub min_angle_of_view_horizontal: f64,
    #[serde_inline_default(1)]
    pub min_look_count: i32,
    #[serde_inline_default(20)]
    pub min_look_time: i32,
    #[serde_inline_default(0.02)]
    pub probability: f64,
    #[serde_inline_default(0)]
    pub random_look_around_cooldown: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRandomLookAround {
    #[serde_inline_default(360)]
    pub angle_of_view_horizontal: i32,
    #[serde_inline_default(360)]
    pub angle_of_view_vertical: i32,
    #[serde_inline_default(8.0)]
    pub look_distance: f64,
    #[serde(default)]
    pub look_time: MinecraftRangeType<f64>,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.2)]
    pub probability: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRandomSearchAndDig {
    #[serde(default)]
    pub cooldown_range: MinecraftRangeType<f64>,
    #[serde(default)]
    pub digging_duration_range: MinecraftRangeType<f64>,
    #[serde_inline_default(0)]
    pub find_valid_position_retries: i32,
    #[serde_inline_default(1.5)]
    pub goal_radius: f64,
    #[serde(default)]
    pub item_table: String,
    #[serde(default)]
    pub on_digging_start: MinecraftTrigger,
    #[serde(default)]
    pub on_fail_during_digging: MinecraftTrigger,
    #[serde(default)]
    pub on_fail_during_searching: MinecraftTrigger,
    #[serde(default)]
    pub on_item_found: MinecraftTrigger,
    #[serde(default)]
    pub on_searching_start: MinecraftTrigger,
    #[serde(default)]
    pub on_success: MinecraftTrigger,
    #[serde(default)]
    pub search_range_xz: f64,
    #[serde(default)]
    pub search_range_y: f64,
    #[serde_inline_default(0.0)]
    pub spawn_item_after_seconds: f64,
    #[serde_inline_default(2.25)]
    pub spawn_item_pos_offset: f64,
    #[serde_inline_default(0.0)]
    pub speed_multiplier: f64,
    #[serde(default)]
    pub target_blocks: Vec<String>,
    #[serde_inline_default(2.25)]
    pub target_dig_position_offset: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRandomSitting {
    #[serde_inline_default(0.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(10)]
    pub min_sit_time: i32,
    #[serde_inline_default(0.1)]
    pub start_chance: f64,
    #[serde_inline_default(0.3)]
    pub stop_chance: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRandomStroll {
    #[serde_inline_default(120)]
    pub interval: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(10)]
    pub xz_dist: i32,
    #[serde_inline_default(7)]
    pub y_dist: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRandomSwim {
    #[serde_inline_default(true)]
    pub avoid_surface: bool,
    #[serde_inline_default(120)]
    pub interval: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(10)]
    pub xz_dist: i32,
    #[serde_inline_default(7)]
    pub y_dist: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRangedAttack {
    #[serde_inline_default(0.0)]
    pub attack_interval: f64,
    #[serde_inline_default(0.0)]
    pub attack_interval_max: f64,
    #[serde_inline_default(0.0)]
    pub attack_interval_min: f64,
    #[serde_inline_default(0.0)]
    pub attack_radius: f64,
    #[serde_inline_default(0.0)]
    pub attack_radius_min: f64,
    #[serde_inline_default(0.0)]
    pub burst_interval: f64,
    #[serde_inline_default(1)]
    pub burst_shots: i32,
    #[serde_inline_default(0.0)]
    pub charge_charged_trigger: f64,
    #[serde_inline_default(0.0)]
    pub charge_shoot_trigger: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(90.0)]
    pub ranged_fov: f64,
    #[serde_inline_default(false)]
    pub set_persistent: bool,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(false)]
    pub swing: bool,
    #[serde_inline_default(1.0)]
    pub target_in_sight_time: f64,
    #[serde_inline_default(30.0)]
    pub x_max_rotation: f64,
    #[serde_inline_default(30.0)]
    pub y_max_head_rotation: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRestrictSun {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRiseToLiquidLevel {
    #[serde_inline_default(0.0)]
    pub liquid_y_offset: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub rise_delta: f64,
    #[serde_inline_default(0.0)]
    pub sink_delta: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRoar {
    #[serde_inline_default(0.0)]
    pub duration: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRoll {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub probability: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRunAroundLikeCrazy {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorScared {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub sound_interval: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSendEvent {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub cast_duration: f64,
    #[serde_inline_default(true)]
    pub look_at_target: bool,
    #[serde(default)]
    pub sequence: Vec<String>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorShareItems {
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0.0)]
    pub max_dist: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSilverfishMergeWithStone {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSilverfishWakeUpFriends {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSkeletonHorseTrap {
    #[serde_inline_default(1.0)]
    pub duration: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSleep {
    #[serde_inline_default(false)]
    pub can_sleep_while_riding: bool,
    #[serde_inline_default(0.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub sleep_collider_height: f64,
    #[serde_inline_default(1.0)]
    pub sleep_collider_width: f64,
    #[serde_inline_default(1.0)]
    pub sleep_y_offset: f64,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(8.0)]
    pub timeout_cooldown: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSlimeAttack {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(false)]
    pub set_persistent: bool,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(10.0)]
    pub x_max_rotation: f64,
    #[serde_inline_default(10.0)]
    pub y_max_rotation: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSlimeFloat {
    #[serde_inline_default(0.8)]
    pub jump_chance_percentage: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.2)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSlimeKeepOnJumping {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSlimeRandomDirection {
    #[serde_inline_default(3)]
    pub add_random_time_range: i32,
    #[serde_inline_default(2.0)]
    pub min_change_direction_time: f64,
    #[serde_inline_default(360)]
    pub turn_range: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSnacking {
    #[serde(default)]
    pub items: Vec<String>,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(7.5)]
    pub snacking_cooldown: f64,
    #[serde_inline_default(0.5)]
    pub snacking_cooldown_min: f64,
    #[serde_inline_default(0.0017)]
    pub snacking_stop_chance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSneeze {
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(1.0)]
    pub drop_item_chance: f64,
    #[serde(default)]
    pub loot_table: String,
    #[serde(default)]
    pub prepare_sound: String,
    #[serde_inline_default(1.0)]
    pub prepare_time: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.02)]
    pub probability: f64,
    #[serde(default)]
    pub sound: String,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSniff {
    #[serde_inline_default(0.0)]
    pub cooldown_range: f64,
    #[serde_inline_default(1.0)]
    pub duration: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(5.0)]
    pub sniffing_radius: f64,
    #[serde_inline_default(3.0)]
    pub suspicion_radius_horizontal: f64,
    #[serde_inline_default(3.0)]
    pub suspicion_radius_vertical: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSonicBoom {
    #[serde_inline_default(5.0)]
    pub attack_cooldown: f64,
    #[serde_inline_default(30.0)]
    pub attack_damage: f64,
    #[serde_inline_default(15.0)]
    pub attack_range_horizontal: f64,
    #[serde_inline_default(20.0)]
    pub attack_range_vertical: f64,
    #[serde(default)]
    pub attack_sound: String,
    #[serde(default)]
    pub charge_sound: String,
    #[serde_inline_default(3.0)]
    pub duration: f64,
    #[serde_inline_default(1.7)]
    pub duration_until_attack_sound: f64,
    #[serde_inline_default(0.0)]
    pub knockback_height_cap: f64,
    #[serde_inline_default(0.0)]
    pub knockback_horizontal_strength: f64,
    #[serde_inline_default(0.0)]
    pub knockback_vertical_strength: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidDive {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidFlee {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidIdle {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidMoveAwayFromGround {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidOutOfWater {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorStalkAndPounceOnTarget {
    #[serde_inline_default(2.0)]
    pub interest_time: f64,
    #[serde_inline_default(0.8)]
    pub leap_distance: f64,
    #[serde_inline_default(0.9)]
    pub leap_height: f64,
    #[serde_inline_default(10.0)]
    pub max_stalk_dist: f64,
    #[serde_inline_default(5.0)]
    pub pounce_max_dist: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(false)]
    pub set_persistent: bool,
    #[serde_inline_default(1.2)]
    pub stalk_speed: f64,
    #[serde_inline_default(2.0)]
    pub strike_dist: f64,
    #[serde_inline_default(2.0)]
    pub stuck_time: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorStayNearNoteblock {
    #[serde_inline_default(0)]
    pub listen_time: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed: f64,
    #[serde_inline_default(10.0)]
    pub start_distance: f64,
    #[serde_inline_default(2.0)]
    pub stop_distance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorStayWhileSitting {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorStompAttack {
    #[serde_inline_default(false)]
    pub attack_once: bool,
    #[serde(default)]
    pub attack_types: String,
    #[serde_inline_default(1)]
    pub cooldown_time: i32,
    #[serde_inline_default(0.25)]
    pub inner_boundary_time_increase: f64,
    #[serde_inline_default(0)]
    pub max_dist: i32,
    #[serde_inline_default(0.55)]
    pub max_path_time: f64,
    #[serde_inline_default(90.0)]
    pub melee_fov: f64,
    #[serde_inline_default(0.2)]
    pub min_path_time: f64,
    #[serde_inline_default(2.0)]
    pub no_damage_range_multiplier: f64,
    #[serde(default)]
    pub on_attack: MinecraftTrigger,
    #[serde_inline_default(0.5)]
    pub outer_boundary_time_increase: f64,
    #[serde_inline_default(0.75)]
    pub path_fail_time_increase: f64,
    #[serde_inline_default(16.0)]
    pub path_inner_boundary: f64,
    #[serde_inline_default(32.0)]
    pub path_outer_boundary: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub random_stop_interval: i32,
    #[serde_inline_default(2.0)]
    pub reach_multiplier: f64,
    #[serde_inline_default(false)]
    pub require_complete_path: bool,
    #[serde_inline_default(false)]
    pub set_persistent: bool,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(2.0)]
    pub stomp_range_multiplier: f64,
    #[serde_inline_default(0.0)]
    pub target_dist: f64,
    #[serde_inline_default(false)]
    pub track_target: bool,
    #[serde_inline_default(30.0)]
    pub x_max_rotation: f64,
    #[serde_inline_default(30.0)]
    pub y_max_head_rotation: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorStompTurtleEgg {
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(120)]
    pub interval: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1)]
    pub search_height: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorStrollTowardsVillage {
    #[serde_inline_default(0.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(0.5)]
    pub goal_radius: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub search_range: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(0.1)]
    pub start_chance: f64,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct MinecraftSummonEntitySequence {
    #[serde_inline_default(0.0)]
    pub base_delay: f64,
    #[serde_inline_default(0.0)]
    pub delay_per_summon: f64,
    #[serde_inline_default(-1.0)]
    pub entity_lifespan: f64,
    #[serde(default)]
    pub entity_type: String,
    #[serde_inline_default(1)]
    pub num_entities_spawned: i32,
    #[serde_inline_default("line".to_string())]
    pub shape: String,
    #[serde_inline_default(0.0)]
    pub size: f64,
    #[serde(default)]
    pub sound_event: String,
    #[serde_inline_default(0)]
    pub summon_cap: i32,
    #[serde_inline_default(0.0)]
    pub summon_cap_radius: f64,
    #[serde_inline_default("self".to_string())]
    pub target: String,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct BehaviorSummonEntitySummonChoices {
    #[serde(default)]
    pub cast_duration: f64,
    #[serde(default)]
    pub cooldown_time: f64,
    #[serde_inline_default(true)]
    pub do_casting: bool,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(32.0)]
    pub max_activation_range: f64,
    #[serde_inline_default(1.0)]
    pub min_activation_range: f64,
    #[serde_inline_default(0)]
    pub particle_color: i32,
    #[serde(default)]
    pub start_sound_event: String,
    #[serde_inline_default(0.0)]
    pub weight: f64,
    #[serde(default)]
    pub sequence: Vec<MinecraftSummonEntitySequence>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSummonEntity {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde(default)]
    pub summon_choices: BehaviorSummonEntitySummonChoices,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSwell {
    #[serde_inline_default(2.0)]
    pub stop_distance: f64,
    #[serde_inline_default(10.0)]
    pub start_distance: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSwimIdle {
    #[serde_inline_default(5.0)]
    pub idle_time: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.1)]
    pub success_rate: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSwimWander {
    #[serde_inline_default(0.00833)]
    pub interval: f64,
    #[serde_inline_default(5.0)]
    pub look_ahead: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde_inline_default(5.0)]
    pub wander_time: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSwimWithEntity {
    #[serde_inline_default(2.5)]
    pub catch_up_multiplier: f64,
    #[serde_inline_default(12.0)]
    pub catch_up_threshold: f64,
    #[serde_inline_default(0.0333)]
    pub chance_to_stop: f64,
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(2.0)]
    pub match_direction_threshold: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(20.0)]
    pub search_range: f64,
    #[serde_inline_default(1.5)]
    pub speed_multiplier: f64,
    #[serde_inline_default(0.5)]
    pub state_check_interval: f64,
    #[serde_inline_default(5.0)]
    pub stop_distance: f64,
    #[serde_inline_default(0.1)]
    pub success_rate: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSwoopAttack {
    #[serde_inline_default(0.2)]
    pub damage_reach: f64,
    #[serde(default)]
    pub delay_range: MinecraftRangeType<f64>,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorTakeFlower {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorTargetWhenPushed {
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(5.0)]
    pub percent_chance: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorTempt {
    #[serde_inline_default(false)]
    pub can_get_scared: bool,
    #[serde_inline_default(false)]
    pub can_tempt_vertically: bool,
    #[serde_inline_default(false)]
    pub can_tempt_while_ridden: bool,
    #[serde(default)]
    pub items: Vec<String>,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde(default)]
    pub sound_interval: MinecraftRangeType<f64>,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
    #[serde(default)]
    pub tempt_sound: String,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorTimerFlag1 {
    #[serde_inline_default(MinecraftRangeType::Vec([10.0, 10.0]))]
    pub cooldown_range: MinecraftRangeType<f64>,
    #[serde_inline_default(MinecraftRangeType::Vec([2.0, 2.0]))]
    pub duration_range: MinecraftRangeType<f64>,
    #[serde(default)]
    pub on_end: MinecraftTrigger,
    #[serde(default)]
    pub on_start: MinecraftTrigger,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorTimerFlag2 {
    #[serde_inline_default(MinecraftRangeType::Vec([10.0, 10.0]))]
    pub cooldown_range: MinecraftRangeType<f64>,
    #[serde_inline_default(MinecraftRangeType::Vec([2.0, 2.0]))]
    pub duration_range: MinecraftRangeType<f64>,
    #[serde(default)]
    pub on_end: MinecraftTrigger,
    #[serde(default)]
    pub on_start: MinecraftTrigger,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorTimerFlag3 {
    #[serde_inline_default(MinecraftRangeType::Vec([10.0, 10.0]))]
    pub cooldown_range: MinecraftRangeType<f64>,
    #[serde_inline_default(MinecraftRangeType::Vec([2.0, 2.0]))]
    pub duration_range: MinecraftRangeType<f64>,
    #[serde(default)]
    pub on_end: MinecraftTrigger,
    #[serde(default)]
    pub on_start: MinecraftTrigger,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorTradeInterest {
    #[serde_inline_default(2.0)]
    pub carried_item_switch_time: f64,
    #[serde_inline_default(2.0)]
    pub cooldown: f64,
    #[serde_inline_default(45.0)]
    pub interest_time: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub remove_item_time: f64,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorTradeWithPlayer {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(8.0)]
    pub max_distance_from_player: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorVexCopyOwnerTarget {
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorVexRandomMove {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorWitherRandomAttackPosGoal {
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorWitherTargetHighestDamage {
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(16.0)]
    pub max_dist: f64,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde_inline_default(3.0)]
    pub must_see_forget_duration: f64,
    #[serde_inline_default(false)]
    pub reevaluate_description: bool,
    #[serde_inline_default(1.0)]
    pub sprint_speed_multiplier: f64,
    #[serde_inline_default(1.0)]
    pub walk_speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorWork {
    #[serde_inline_default(0)]
    pub active_time: i32,
    #[serde_inline_default(false)]
    pub can_work_in_rain: bool,
    #[serde_inline_default(0)]
    pub goal_cooldown: i32,
    #[serde(default)]
    pub on_arrival: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub sound_delay_max: i32,
    #[serde_inline_default(0)]
    pub sound_delay_min: i32,
    #[serde_inline_default(0.5)]
    pub speed_multiplier: f64,
    #[serde_inline_default(-1)]
    pub work_in_rain_tolerance: i32,
    #[serde_inline_default(0)]
    pub priority: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorWorkComposter {
    #[serde_inline_default(0)]
    pub active_time: i32,
    #[serde_inline_default(1)]
    pub block_interaction_max: i32,
    #[serde_inline_default(true)]
    pub can_empty_composter: bool,
    #[serde_inline_default(true)]
    pub can_fill_composter: bool,
    #[serde_inline_default(false)]
    pub can_work_in_rain: bool,
    #[serde_inline_default(0)]
    pub goal_cooldown: i32,
    #[serde_inline_default(20)]
    pub items_per_use_max: i32,
    #[serde_inline_default(10)]
    pub min_item_count: i32,
    #[serde(default)]
    pub on_arrival: MinecraftTrigger,
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub sound_delay_max: i32,
    #[serde_inline_default(0)]
    pub sound_delay_min: i32,
    #[serde_inline_default(0.5)]
    pub speed_multiplier: f64,
    #[serde_inline_default(200)]
    pub use_block_max: i32,
    #[serde_inline_default(100)]
    pub use_block_min: i32,
    #[serde_inline_default(-1)]
    pub work_in_rain_tolerance: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorReceiveLove {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRestrictOpenDoor {
    #[serde_inline_default(0)]
    pub priority: i32,
}

//todo: minecraft:behavior.summon_entity
