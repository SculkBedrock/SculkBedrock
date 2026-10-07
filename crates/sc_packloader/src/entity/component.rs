use crate::ident::block_ident::MinecraftBlockIdent;
use crate::types::event::MinecraftEvent;
use crate::types::filter::{MinecraftFilter, MinecraftFilterType};
use crate::types::molang::MolangExpression;
use crate::types::range::{MinecraftRange, MinecraftRangeType};
use crate::types::trigger::MinecraftTrigger;
use sc_ecs::component::Component;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

/*
 warning: All code below was AI-generated and has not been verified for correctness
 Official docs:
 https://learn.microsoft.com/en-us/minecraft/creator/reference/content/entityreference/examples/componentlist?view=minecraft-bedrock-stable
*/

#[derive(Clone, Component, Deserialize, Debug)]
pub struct AddRider {
    pub entity_type: String,
    pub spawn_event: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AdmireItem {
    #[serde(default)]
    pub cooldown_after_being_attacked: i32,
    #[serde_inline_default(10)]
    pub duration: i32,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Ageable {
    pub cooldown_after_being_attacked: i32,
    pub duration: i32,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct AmbientSoundIntervalEventNames {
    #[serde(default)]
    pub event_name: String,
    #[serde(default)]
    pub condition: MolangExpression,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AmbientSoundInterval {
    #[serde_inline_default("ambient".to_string())]
    event_name: String,
    #[serde(default)]
    event_names: Vec<AmbientSoundIntervalEventNames>,
    #[serde_inline_default(16.0)]
    range: f64,
    #[serde_inline_default(8.0)]
    value: f64,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct AngerLevelSound {
    pub sound: String,
    pub condition: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AngerLevel {
    #[serde_inline_default(1.0)]
    pub anger_decrement_interval: f64,
    #[serde_inline_default(20)]
    pub angry_boost: u32,
    #[serde_inline_default(80)]
    pub angry_threshold: u32,
    #[serde(default)]
    pub default_annoyingness: i32,
    #[serde(default)]
    pub default_projectile_annoyingness: i32,
    #[serde_inline_default(100)]
    pub max_anger: u32,
    #[serde(default)]
    pub nuisance_filter: MinecraftFilterType,
    #[serde(default)]
    pub on_increase_sounds: Vec<AngerLevelSound>,
    #[serde_inline_default(true)]
    pub remove_targets_below_angry_threshold: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Angry {
    #[serde(default)]
    pub angry_sound: String,
    #[serde(default)]
    pub broadcast_anger: bool,
    #[serde(default)]
    pub broadcast_anger_on_attack: bool,
    #[serde(default)]
    pub broadcast_anger_on_being_attacked: bool,
    #[serde(default)]
    pub broadcast_filters: MinecraftFilterType,
    #[serde_inline_default(20)]
    pub broadcast_range: i32,
    #[serde(default)]
    pub broadcast_targets: Vec<String>,
    #[serde(default)]
    pub calm_event: MinecraftEvent,
    #[serde_inline_default(25)]
    pub duration: i32,
    #[serde(default)]
    pub duration_delta: i32,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde(default)]
    pub sound_interval: MinecraftRangeType<f64>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AnnotationBreakDoor {
    #[serde_inline_default(12.0)]
    pub break_time: f64,
    #[serde_inline_default("hard".to_string())]
    pub min_difficulty: String,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct AnnotationOpenDoor {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AreaAttack {
    #[serde(default)]
    pub cause: String,
    #[serde(default)]
    pub entity_filter: MinecraftFilterType,
    #[serde_inline_default(2)]
    pub damage_per_tick: i32,
    #[serde_inline_default(0.2)]
    pub damage_range: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Attack {
    pub damage: MinecraftRangeType<f64>,
    #[serde(default)]
    pub effect_duration: f64,
    #[serde(default)]
    pub effect_name: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AttackCooldown {
    #[serde(default)]
    pub attack_cooldown_complete_event: MinecraftEvent,
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(0.0, 1.0)))]
    pub attack_cooldown_time: MinecraftRangeType<f64>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct AttackDamage {
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Balloonable {
    #[serde_inline_default(2.0)]
    pub soft_distance: f64,
    #[serde_inline_default(2.0)]
    pub max_distance: f64,
    #[serde(default)]
    pub on_balloon: String,
    #[serde(default)]
    pub on_unballoon: String,
    #[serde_inline_default(1.0)]
    pub mass: f64,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Barter {
    pub barter_table: String,
    #[serde(default)]
    pub cooldown_after_being_attacked: i32,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct BlockClimber {}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct BlockSensorOnBreak {
    block_list: Vec<String>,
    on_block_broken: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BlockSensor {
    pub on_break: Vec<BlockSensorOnBreak>,
    #[serde_inline_default(16.0)]
    pub sensor_radius: f64,
    #[serde(default)]
    pub sources: Vec<String>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct BodyRotationBlocked {}

#[serde_inline_default]
#[derive(Clone, Deserialize, Default, Debug)]
pub struct BoostableBoostItem {
    #[serde_inline_default(1)]
    pub damage: i32,
    pub item: String,
    pub replace_item: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Boostable {
    #[serde(default)]
    pub boost_items: Vec<BoostableBoostItem>,
    #[serde_inline_default(3.0)]
    pub duration: f64,
    #[serde_inline_default(1.35)]
    pub speed_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Boss {
    #[serde_inline_default(55)]
    pub hud_range: i32,
    pub name: String,
    #[serde(default)]
    pub should_darken_sky: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BreakBlocks {
    #[serde(default)]
    pub breakable_blocks: Vec<String>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Breathable {
    #[serde(default)]
    pub breathe_blocks: Vec<String>,
    #[serde_inline_default(true)]
    pub breathes_air: bool,
    #[serde_inline_default(true)]
    pub breathes_lava: bool,
    #[serde_inline_default(false)]
    pub breathes_solids: bool,
    #[serde_inline_default(false)]
    pub breathes_water: bool,
    #[serde_inline_default(true)]
    pub generates_bubbles: bool,
    #[serde_inline_default(0)]
    pub inhale_time: i32,
    #[serde(default)]
    pub non_breathe_blocks: Vec<String>,
    #[serde_inline_default(-20)]
    #[serde(alias = "suffocateTime")]
    pub suffocate_time: i32,
    #[serde_inline_default(15)]
    #[serde(alias = "totalSupply")]
    pub total_supply: i32,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct BreedableBreedsWith {
    #[serde(default)]
    pub baby_type: String,
    #[serde(default)]
    pub breed_event: String,
    #[serde(default)]
    pub mate_type: String,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct BreedableDenyParentsVariant {
    #[serde(default)]
    chance: f64,
    #[serde(default)]
    max_variant: i32,
    #[serde(default)]
    min_variant: i32,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct BreedableEnvironmentRequirements {
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default)]
    pub count: i32,
    #[serde(default)]
    pub radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct BreedableMutationFactor {
    #[serde_inline_default(0.0)]
    pub color: f64,
    #[serde_inline_default(0.0)]
    pub extra_baby_chance: f64,
    #[serde_inline_default(0.0)]
    pub extra_variant: f64,
    #[serde_inline_default(true)]
    pub inherit_tamed: bool,
    #[serde_inline_default(0.0)]
    pub variant: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Breedable {
    #[serde_inline_default(false)]
    pub allow_sitting: bool,
    #[serde_inline_default(true)]
    pub blend_attributes: bool,
    #[serde_inline_default(60)]
    pub breed_cooldown: i32,
    #[serde(default)]
    pub breed_items: Vec<String>,
    #[serde(default)]
    pub breeds_with: Vec<BreedableBreedsWith>,
    #[serde_inline_default(false)]
    pub causes_pregnancy: bool,
    #[serde(default)]
    pub deny_parents_variant: BreedableDenyParentsVariant,
    #[serde(default)]
    pub environment_requirements: BreedableEnvironmentRequirements,
    #[serde_inline_default(0.0)]
    pub extra_baby_chance: f64,
    #[serde_inline_default(true)]
    pub inherit_tamed: bool,
    #[serde(default)]
    pub love_filters: MinecraftFilterType,
    #[serde(default)]
    pub mutation_factor: BreedableMutationFactor,
    #[serde(default)]
    pub mutation_strategy: String,
    #[serde(default)]
    pub parent_centric_attribute_blending: Vec<String>,
    #[serde(default)]
    pub random_extra_variant_mutation_interval: MinecraftRangeType<f64>,
    #[serde(default)]
    pub random_variant_mutation_interval: MinecraftRangeType<f64>,
    #[serde_inline_default(false)]
    pub require_full_health: bool,
    #[serde_inline_default(true)]
    pub require_tame: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Bribeable {
    #[serde_inline_default(2.0)]
    pub bribe_cooldown: f64,
    #[serde(default)]
    pub bribe_items: Vec<String>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Buoyant {
    #[serde_inline_default(true)]
    pub apply_gravity: bool,
    #[serde_inline_default(1.0)]
    pub base_buoyancy: f64,
    #[serde_inline_default(0.03)]
    pub big_wave_probability: f64,
    #[serde_inline_default(10.0)]
    pub big_wave_speed: f64,
    #[serde_inline_default(0.0)]
    pub drag_down_on_buoyancy_removed: f64,
    #[serde(default)]
    pub liquid_blocks: Vec<String>,
    #[serde_inline_default(true)]
    pub simulate_waves: bool,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct BurnsInDaylight {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanClimb {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanFly {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanJoinRaid {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanPowerJump {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CelebrateHunt {
    #[serde_inline_default(true)]
    pub broadcast: bool,
    #[serde(default)]
    pub celebration_targets: MinecraftFilterType,
    #[serde(default)]
    pub celebrate_sound: String,
    #[serde_inline_default(4)]
    pub duration: i32,
    #[serde_inline_default(16.0)]
    pub radius: f64,
    #[serde_inline_default(MinecraftRangeType::Number(0.0))]
    pub sound_interval: MinecraftRangeType<f64>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Color {
    #[serde_inline_default(0)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Color2 {
    #[serde_inline_default(0)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CombatRegeneration {
    #[serde_inline_default(5)]
    pub regeneration_duration: i32,
    #[serde_inline_default(false)]
    pub apply_to_self: bool,
    #[serde_inline_default(false)]
    pub apply_to_family: bool,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct ConditionalBandwidthOptimizationConditionalValues {
    #[serde(default)]
    pub max_dropped_ticks: i32,
    #[serde(default)]
    pub max_optimized_distance: i32,
    #[serde(default)]
    pub use_motion_prediction_hints: bool,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct ConditionalBandwidthOptimizationDefaultValues {
    #[serde(default)]
    pub max_dropped_ticks: i32,
    #[serde(default)]
    pub max_optimized_distance: i32,
    #[serde(default)]
    pub use_motion_prediction_hints: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ConditionalBandwidthOptimization {
    #[serde(default)]
    pub conditional_values: Vec<ConditionalBandwidthOptimizationConditionalValues>,
    #[serde(default)]
    pub default_values: ConditionalBandwidthOptimizationDefaultValues,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct CustomHitTestHitboxes {
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub pivot: Vec<f64>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CustomHitTest {
    #[serde(default)]
    pub hitboxes: Vec<CustomHitTestHitboxes>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct DamageOverTime {
    #[serde_inline_default(1)]
    pub damage_per_hurt: i32,
    #[serde_inline_default(0.0)]
    pub time_between_hurt: f64,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct DamageSensorTriggersOnDamage {
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde(default)]
    pub event: String,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct DamageSensorTriggers {
    #[serde(default)]
    pub cause: String,
    #[serde(default)]
    pub damage_modifier: f64,
    #[serde(default)]
    pub damage_multiplier: f64,
    #[serde(default)]
    pub deals_damage: bool,
    #[serde(default)]
    pub on_damage: DamageSensorTriggersOnDamage,
    #[serde(default)]
    pub on_damage_sound_event: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct DamageSensor {
    #[serde(default)]
    pub triggers: Vec<DamageSensorTriggers>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Dash {
    #[serde_inline_default(1.0)]
    pub cooldown_time: f64,
    #[serde_inline_default(1.0)]
    pub horizontal_momentum: f64,
    #[serde_inline_default(1.0)]
    pub vertical_momentum: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct DefaultLookAngle {
    #[serde_inline_default(0.0)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct DespawnFromDistance {
    #[serde_inline_default(128)]
    pub max_distance: i32,
    #[serde_inline_default(32)]
    pub min_distance: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Despawn {
    #[serde(default)]
    pub despawn_from_distance: DespawnFromDistance,
    #[serde(default)]
    pub despawn_from_chance: bool,
    #[serde(default)]
    pub despawn_from_inactivity: bool,
    #[serde(default)]
    pub despawn_from_simulation_edge: bool,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(30)]
    pub min_range_inactivity_timer: i32,
    #[serde_inline_default(800)]
    pub min_range_random_chance: i32,
    #[serde_inline_default(false)]
    pub remove_child_entities: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct DryingOutTimer {
    #[serde_inline_default(0.0)]
    pub total_time: f64,
    #[serde(default)]
    pub dried_out_event: String,
    #[serde(default)]
    pub recover_after_dried_out_event: String,
    #[serde(default)]
    pub stopped_drying_out_event: String,
    #[serde_inline_default(0.0)]
    pub water_bottle_refill_time: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Dweller {
    #[serde(default)]
    pub dwelling_type: String,
    #[serde(default)]
    pub dweller_role: String,
    #[serde_inline_default(0.0)]
    pub update_interval_base: f64,
    #[serde_inline_default(0.0)]
    pub update_interval_variant: f64,
    #[serde(default)]
    pub can_find_poi: bool,
    #[serde_inline_default(0)]
    pub first_founding_reward: i32,
    #[serde(default)]
    pub can_migrate: bool,
    #[serde_inline_default(0.0)]
    pub dwelling_bounds_tolerance: f64,
    #[serde(default)]
    pub preferred_profession: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct EconomyTradeTable {
    #[serde(default)]
    pub convert_trades_economy: bool,
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(-5.0, -20.0)))]
    pub cured_discount: MinecraftRangeType<f64>,
    #[serde(default)]
    pub display_name: String,
    #[serde_inline_default(-4)]
    pub hero_demand_discount: i32,
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(-25.0, -63.0)))]
    pub max_cured_discount: MinecraftRangeType<f64>,
    #[serde_inline_default(-200)]
    pub max_nearby_cured_discount: i32,
    #[serde_inline_default(-25)]
    pub nearby_cured_discount: i32,
    #[serde(default)]
    pub new_screen: bool,
    #[serde(default)]
    pub persist_trades: bool,
    #[serde(default)]
    pub show_trade_screen: bool,
    #[serde(default)]
    pub table: String,
    #[serde(default)]
    pub use_legacy_price_formula: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct EntitySensor {
    #[serde_inline_default(-1.0)]
    pub cooldown: f64,
    #[serde(default)]
    pub event_filters: MinecraftFilterType,
    #[serde(default)]
    pub event: String,
    #[serde_inline_default(-1)]
    pub maximum_count: i32,
    #[serde_inline_default(1)]
    pub minimum_count: i32,
    #[serde_inline_default([10.0, 10.0])]
    pub range: [f64; 2],
    #[serde(default)]
    pub require_all: bool,
    #[serde(default)]
    pub subsensors: Vec<Self>,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct EnvironmentSensorTrigger {
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default("self".to_string())]
    pub target: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct EnvironmentSensor {
    #[serde(default)]
    pub triggers: Vec<EnvironmentSensorTrigger>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct EquipItem {}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct EquipmentSlotDropChance {
    #[serde(default)]
    pub slot: String,
    #[serde_inline_default(0.0)]
    pub chance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Equipment {
    #[serde(default)]
    pub slot_drop_chance: Vec<EquipmentSlotDropChance>,
    #[serde(default)]
    pub table: String,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct EquippableSlot {
    #[serde(default)]
    pub accepted_items: Vec<String>,
    #[serde(default)]
    pub interact_text: String,
    #[serde(default)]
    pub item: String,
    #[serde(default)]
    pub on_equip: MinecraftEvent,
    #[serde(default)]
    pub on_unequip: MinecraftEvent,
    #[serde_inline_default(0)]
    pub slot: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Equippable {
    #[serde(default)]
    pub slots: Vec<EquippableSlot>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ExhaustionValues {
    #[serde_inline_default(0.1)]
    pub attack: f64,
    #[serde_inline_default(0.1)]
    pub damage: f64,
    #[serde_inline_default(6.0)]
    pub heal: f64,
    #[serde_inline_default(0.005)]
    pub jump: f64,
    #[serde_inline_default(0.01)]
    pub mine: f64,
    #[serde_inline_default(0.01)]
    pub sprint: f64,
    #[serde_inline_default(0.2)]
    pub sprint_jump: f64,
    #[serde_inline_default(0.01)]
    pub swim: f64,
    #[serde_inline_default(0.0)]
    pub walk: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ExperienceReward {
    #[serde(default)]
    pub on_bred: MolangExpression,
    #[serde(default)]
    pub on_death: MolangExpression,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Explode {
    #[serde_inline_default(true)]
    pub breaks_blocks: bool,
    #[serde_inline_default(false)]
    pub causes_fire: bool,
    #[serde_inline_default(false)]
    pub destroy_affected_by_griefing: bool,
    #[serde_inline_default(false)]
    pub fire_affected_by_griefing: bool,
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(0.0, 0.0)))]
    pub fuse_length: MinecraftRangeType<f64>,
    #[serde_inline_default(false)]
    pub fuse_lit: bool,
    #[serde_inline_default(3.40282e+38)]
    pub max_resistance: f64,
    #[serde_inline_default(3.0)]
    pub power: f64,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct FireImmune {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct FloatsInLiquid {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Flocking {
    #[serde_inline_default(0.0)]
    pub block_distance: f64,
    #[serde_inline_default(0.0)]
    pub block_weight: f64,
    #[serde_inline_default(0.0)]
    pub breach_influence: f64,
    #[serde_inline_default(1.0)]
    pub cohesion_threshold: f64,
    #[serde_inline_default(1.0)]
    pub cohesion_weight: f64,
    #[serde_inline_default(0.0)]
    pub goal_weight: f64,
    #[serde_inline_default(0)]
    pub high_flock_limit: i32,
    #[serde_inline_default(false)]
    pub in_water: bool,
    #[serde_inline_default(0.0)]
    pub influence_radius: f64,
    #[serde_inline_default(0.0)]
    pub inner_cohesion_threshold: f64,
    #[serde_inline_default(0.0)]
    pub loner_chance: f64,
    #[serde_inline_default(0)]
    pub low_flock_limit: i32,
    #[serde_inline_default(false)]
    pub match_variants: bool,
    #[serde_inline_default(0.0)]
    pub max_height: f64,
    #[serde_inline_default(0.0)]
    pub min_height: f64,
    #[serde_inline_default(2.0)]
    pub separation_threshold: f64,
    #[serde_inline_default(1.0)]
    pub separation_weight: f64,
    #[serde_inline_default(false)]
    pub use_center_of_mass: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct FlyingSpeed {
    #[serde_inline_default(0.0)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct FollowRange {
    #[serde_inline_default(0.0)]
    pub value: f64,
    #[serde_inline_default(0.0)]
    pub max: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct FrictionModifier {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct GameEventMovementTracking {
    #[serde_inline_default(false)]
    pub emit_flap: bool,
    #[serde_inline_default(true)]
    pub emit_move: bool,
    #[serde_inline_default(true)]
    pub emit_swim: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Genetics {
    #[serde(default)]
    pub genes: Vec<String>,
    #[serde_inline_default(0.03125)]
    pub mutation_rate: f64,
    #[serde(default)]
    pub use_simplified_breeding: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Giveable {
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub on_give: MinecraftEvent,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct GroundOffset {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct GroupSize {
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(16.0)]
    pub radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct GrowsCrop {
    #[serde_inline_default(0.0)]
    pub chance: f64,
    #[serde_inline_default(10)]
    pub charges: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct HealableItem {
    #[serde_inline_default(1)]
    pub heal_amount: i32,
    #[serde(default)]
    pub item: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Healable {
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde(default)]
    pub force_use: bool,
    #[serde(default)]
    pub items: Vec<HealableItem>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Health {
    #[serde_inline_default(0.0)]
    pub max: f64,
    #[serde_inline_default(0.0)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Heartbeat {
    #[serde(default)]
    pub interval: MolangExpression,
    #[serde_inline_default("heartbeat".to_string())]
    pub sound_event: String,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Hide {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Home {
    #[serde(default)]
    pub home_block_list: Vec<String>,
    #[serde_inline_default(-1)]
    pub restriction_radius: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct HorseJumpStrength {
    #[serde(default)]
    pub value: MinecraftRangeType<f64>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct HurtOnConditionDamageCondition {
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde(default)]
    pub cause: String,
    #[serde(default)]
    pub damage_per_tick: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct HurtOnCondition {
    #[serde(default)]
    pub damage_conditions: Vec<HurtOnConditionDamageCondition>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct InputGroundControlled {}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct InsideBlockNotifierBlockList {
    pub block: MinecraftBlockIdent,
    #[serde(default)]
    pub entered_block_event: MinecraftEvent,
    #[serde(default)]
    pub exited_block_event: MinecraftEvent,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct InsideBlockNotifier {
    #[serde(default)]
    pub block_list: Vec<InsideBlockNotifierBlockList>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Insomnia {
    #[serde_inline_default(3.0)]
    pub days_until_insomnia: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct InstantDespawn {
    #[serde_inline_default(false)]
    pub remove_child_entities: bool,
}

#[derive(Clone, Deserialize, Debug, Default)]
pub struct InteractAddItems {
    #[serde(default)]
    pub table: String,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct InteractParticleOnStart {
    #[serde_inline_default(false)]
    pub particle_offset_towards_interactor: bool,
    #[serde(default)]
    pub particle_type: String,
    #[serde_inline_default(0.0)]
    pub particle_y_offset: f64,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct InteractSpawnItems {
    #[serde(default)]
    pub table: String,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct InteractRepairEntityItem {
    #[serde(default)]
    pub item_slot: String,
    #[serde_inline_default(0.0)]
    pub repair_amount: f64,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct InteractList {
    #[serde(default)]
    pub add_items: InteractAddItems,
    #[serde_inline_default(0.0)]
    pub cooldown: f64,
    #[serde_inline_default(0.0)]
    pub cooldown_after_being_attacked: f64,
    #[serde(default)]
    pub drop_item_slot: String,
    #[serde(default)]
    pub equip_item_slot: String,
    #[serde_inline_default(0)]
    pub health_amount: i32,
    #[serde_inline_default(0)]
    pub hurt_item: i32,
    #[serde(default)]
    pub interact_text: String,
    #[serde(default)]
    pub on_interact: MinecraftEvent,
    #[serde(default)]
    pub particle_on_start: InteractParticleOnStart,
    #[serde(default)]
    pub play_sounds: String,
    #[serde(default)]
    pub repair_entity_item: InteractRepairEntityItem, //????
    #[serde(default)]
    pub spawn_entities: String,
    #[serde(default)]
    pub spawn_items: InteractSpawnItems,
    #[serde_inline_default(false)]
    pub swing: bool,
    #[serde(default)]
    pub transform_to_item: String,
    #[serde_inline_default(false)]
    pub give_item: bool,
    #[serde_inline_default(false)]
    pub take_item: bool,
    #[serde_inline_default(false)]
    pub use_item: bool,
    #[serde(default)]
    pub vibration: String,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Interact {
    #[serde(default)]
    pub interactions: Vec<InteractList>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Inventory {
    #[serde_inline_default(0)]
    pub additional_slots_per_strength: i32,
    #[serde_inline_default(false)]
    pub can_be_siphoned_from: bool,
    #[serde_inline_default("none".to_string())]
    pub container_type: String,
    #[serde_inline_default(5)]
    pub inventory_size: i32,
    #[serde_inline_default(false)]
    pub private: bool,
    #[serde_inline_default(false)]
    pub restrict_to_owner: bool,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsBaby {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsCharged {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsChested {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsDyeable {
    #[serde(default)]
    pub interact_text: String,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsHiddenWhenInvisible {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsIgnited {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsIllagerCaptain {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsSaddled {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsShaking {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsSheared {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsStackable {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsStunned {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsTamed {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ItemControllable {
    #[serde(default)]
    pub control_items: Vec<String>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct ItemHopper {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct JumpDynamic {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct JumpStatic {
    #[serde_inline_default(0.42)]
    pub jump_power: f64,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct KnockbackResistance {
    #[serde(default)]
    pub value: f64,
    #[serde(default)]
    pub max: f64,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct LavaMovement {
    #[serde(default)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Leashable {
    #[serde_inline_default(false)]
    pub can_be_stolen: bool,
    #[serde_inline_default(6.0)]
    pub hard_distance: f64,
    #[serde_inline_default(10.0)]
    pub max_distance: f64,
    #[serde(default)]
    pub on_leash: MinecraftEvent,
    #[serde(default)]
    pub on_unleash: MinecraftEvent,
    #[serde_inline_default(4.0)]
    pub soft_distance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct LookAt {
    #[serde_inline_default(false)]
    pub allow_invulnerable: bool,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(0.0, 0.0)))]
    pub look_cooldown: MinecraftRangeType<f64>,
    #[serde(default)]
    pub look_event: String,
    #[serde_inline_default(10.0)]
    pub search_radius: f64,
    #[serde_inline_default(true)]
    pub set_target: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Loot {
    #[serde(default)]
    pub table: String,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct ManagedWanderingTrader {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MarkVariant {
    #[serde_inline_default(0)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MobEffect {
    #[serde_inline_default(0)]
    pub cooldown_time: i32,
    #[serde(default)]
    pub entity_filter: MinecraftFilterType,
    #[serde_inline_default(0.2)]
    pub effect_range: f64,
    #[serde_inline_default(10)]
    pub effect_time: i32,
    #[serde(default)]
    pub mob_effect: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementAmphibious {
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementBasic {
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementDolphin {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementFly {
    #[serde_inline_default(0.1)]
    pub start_speed: f64,
    #[serde_inline_default(0.2)]
    pub speed_when_turning: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementGeneric {
    #[serde_inline_default(0.1)]
    pub start_speed: f64,
    #[serde_inline_default(0.2)]
    pub speed_when_turning: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementGlide {
    #[serde_inline_default(0.1)]
    pub start_speed: f64,
    #[serde_inline_default(0.2)]
    pub speed_when_turning: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementHover {
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementJump {
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(0.0, 0.0)))]
    pub jump_delay: MinecraftRangeType<f64>,
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementSkip {
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementSoundDistanceOffset {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementSway {
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
    #[serde_inline_default(0.05)]
    pub sway_amplitude: f64,
    #[serde_inline_default(0.5)]
    pub sway_frequency: f64,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct NameableNameActions {
    #[serde(default)]
    pub name_filter: String,
    #[serde(default)]
    pub on_named: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Nameable {
    #[serde_inline_default(true)]
    pub allow_name_tag_renaming: bool,
    #[serde_inline_default(false)]
    pub always_show: bool,
    #[serde(default)]
    pub default_trigger: String,
    #[serde(default)]
    pub name_actions: NameableNameActions,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct NavigationClimb {
    #[serde_inline_default(false)]
    pub avoid_damage_blocks: bool,
    #[serde_inline_default(false)]
    pub avoid_portals: bool,
    #[serde_inline_default(false)]
    pub avoid_sun: bool,
    #[serde_inline_default(false)]
    pub avoid_water: bool,
    #[serde(default)]
    pub blocks_to_avoid: Vec<String>,
    #[serde_inline_default(false)]
    pub can_breach: bool,
    #[serde_inline_default(false)]
    pub can_break_doors: bool,
    #[serde_inline_default(true)]
    pub can_jump: bool,
    #[serde_inline_default(false)]
    pub can_open_doors: bool,
    #[serde_inline_default(false)]
    pub can_open_iron_doors: bool,
    #[serde_inline_default(true)]
    pub can_pass_doors: bool,
    #[serde_inline_default(false)]
    pub can_path_from_air: bool,
    #[serde_inline_default(false)]
    pub can_path_over_lava: bool,
    #[serde_inline_default(false)]
    pub can_path_over_water: bool,
    #[serde_inline_default(true)]
    pub can_sink: bool,
    #[serde_inline_default(false)]
    pub can_swim: bool,
    #[serde_inline_default(true)]
    pub can_walk: bool,
    #[serde_inline_default(false)]
    pub can_walk_in_lava: bool,
    #[serde_inline_default(false)]
    pub is_amphibious: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct NavigationFloat {
    #[serde_inline_default(false)]
    pub avoid_damage_blocks: bool,
    #[serde_inline_default(false)]
    pub avoid_portals: bool,
    #[serde_inline_default(false)]
    pub avoid_sun: bool,
    #[serde_inline_default(false)]
    pub avoid_water: bool,
    #[serde(default)]
    pub blocks_to_avoid: Vec<String>,
    #[serde_inline_default(false)]
    pub can_breach: bool,
    #[serde_inline_default(false)]
    pub can_break_doors: bool,
    #[serde_inline_default(true)]
    pub can_jump: bool,
    #[serde_inline_default(false)]
    pub can_open_doors: bool,
    #[serde_inline_default(false)]
    pub can_open_iron_doors: bool,
    #[serde_inline_default(true)]
    pub can_pass_doors: bool,
    #[serde_inline_default(false)]
    pub can_path_from_air: bool,
    #[serde_inline_default(false)]
    pub can_path_over_lava: bool,
    #[serde_inline_default(false)]
    pub can_path_over_water: bool,
    #[serde_inline_default(true)]
    pub can_sink: bool,
    #[serde_inline_default(false)]
    pub can_swim: bool,
    #[serde_inline_default(true)]
    pub can_walk: bool,
    #[serde_inline_default(false)]
    pub can_walk_in_lava: bool,
    #[serde_inline_default(false)]
    pub is_amphibious: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct NavigationFly {
    #[serde_inline_default(false)]
    pub avoid_damage_blocks: bool,
    #[serde_inline_default(false)]
    pub avoid_portals: bool,
    #[serde_inline_default(false)]
    pub avoid_sun: bool,
    #[serde_inline_default(false)]
    pub avoid_water: bool,
    #[serde(default)]
    pub blocks_to_avoid: Vec<String>,
    #[serde_inline_default(false)]
    pub can_breach: bool,
    #[serde_inline_default(false)]
    pub can_break_doors: bool,
    #[serde_inline_default(true)]
    pub can_jump: bool,
    #[serde_inline_default(false)]
    pub can_open_doors: bool,
    #[serde_inline_default(false)]
    pub can_open_iron_doors: bool,
    #[serde_inline_default(true)]
    pub can_pass_doors: bool,
    #[serde_inline_default(false)]
    pub can_path_from_air: bool,
    #[serde_inline_default(false)]
    pub can_path_over_lava: bool,
    #[serde_inline_default(false)]
    pub can_path_over_water: bool,
    #[serde_inline_default(true)]
    pub can_sink: bool,
    #[serde_inline_default(false)]
    pub can_swim: bool,
    #[serde_inline_default(true)]
    pub can_walk: bool,
    #[serde_inline_default(false)]
    pub can_walk_in_lava: bool,
    #[serde_inline_default(false)]
    pub is_amphibious: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct NavigationGeneric {
    #[serde_inline_default(false)]
    pub avoid_damage_blocks: bool,
    #[serde_inline_default(false)]
    pub avoid_portals: bool,
    #[serde_inline_default(false)]
    pub avoid_sun: bool,
    #[serde_inline_default(false)]
    pub avoid_water: bool,
    #[serde(default)]
    pub blocks_to_avoid: Vec<String>,
    #[serde_inline_default(false)]
    pub can_breach: bool,
    #[serde_inline_default(false)]
    pub can_break_doors: bool,
    #[serde_inline_default(true)]
    pub can_jump: bool,
    #[serde_inline_default(false)]
    pub can_open_doors: bool,
    #[serde_inline_default(false)]
    pub can_open_iron_doors: bool,
    #[serde_inline_default(true)]
    pub can_pass_doors: bool,
    #[serde_inline_default(false)]
    pub can_path_from_air: bool,
    #[serde_inline_default(false)]
    pub can_path_over_lava: bool,
    #[serde_inline_default(false)]
    pub can_path_over_water: bool,
    #[serde_inline_default(true)]
    pub can_sink: bool,
    #[serde_inline_default(false)]
    pub can_swim: bool,
    #[serde_inline_default(true)]
    pub can_walk: bool,
    #[serde_inline_default(false)]
    pub can_walk_in_lava: bool,
    #[serde_inline_default(false)]
    pub is_amphibious: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct NavigationHover {
    #[serde_inline_default(false)]
    pub avoid_damage_blocks: bool,
    #[serde_inline_default(false)]
    pub avoid_portals: bool,
    #[serde_inline_default(false)]
    pub avoid_sun: bool,
    #[serde_inline_default(false)]
    pub avoid_water: bool,
    #[serde(default)]
    pub blocks_to_avoid: Vec<String>,
    #[serde_inline_default(false)]
    pub can_breach: bool,
    #[serde_inline_default(false)]
    pub can_break_doors: bool,
    #[serde_inline_default(true)]
    pub can_jump: bool,
    #[serde_inline_default(false)]
    pub can_open_doors: bool,
    #[serde_inline_default(false)]
    pub can_open_iron_doors: bool,
    #[serde_inline_default(true)]
    pub can_pass_doors: bool,
    #[serde_inline_default(false)]
    pub can_path_from_air: bool,
    #[serde_inline_default(false)]
    pub can_path_over_lava: bool,
    #[serde_inline_default(false)]
    pub can_path_over_water: bool,
    #[serde_inline_default(true)]
    pub can_sink: bool,
    #[serde_inline_default(false)]
    pub can_swim: bool,
    #[serde_inline_default(true)]
    pub can_walk: bool,
    #[serde_inline_default(false)]
    pub can_walk_in_lava: bool,
    #[serde_inline_default(false)]
    pub is_amphibious: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct NavigationSwim {
    #[serde_inline_default(false)]
    pub avoid_damage_blocks: bool,
    #[serde_inline_default(false)]
    pub avoid_portals: bool,
    #[serde_inline_default(false)]
    pub avoid_sun: bool,
    #[serde_inline_default(false)]
    pub avoid_water: bool,
    #[serde(default)]
    pub blocks_to_avoid: Vec<String>,
    #[serde_inline_default(false)]
    pub can_breach: bool,
    #[serde_inline_default(false)]
    pub can_break_doors: bool,
    #[serde_inline_default(true)]
    pub can_jump: bool,
    #[serde_inline_default(false)]
    pub can_open_doors: bool,
    #[serde_inline_default(false)]
    pub can_open_iron_doors: bool,
    #[serde_inline_default(true)]
    pub can_pass_doors: bool,
    #[serde_inline_default(false)]
    pub can_path_from_air: bool,
    #[serde_inline_default(false)]
    pub can_path_over_lava: bool,
    #[serde_inline_default(false)]
    pub can_path_over_water: bool,
    #[serde_inline_default(true)]
    pub can_sink: bool,
    #[serde_inline_default(false)]
    pub can_swim: bool,
    #[serde_inline_default(true)]
    pub can_walk: bool,
    #[serde_inline_default(false)]
    pub can_walk_in_lava: bool,
    #[serde_inline_default(false)]
    pub is_amphibious: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct NavigationWalk {
    #[serde_inline_default(false)]
    pub avoid_damage_blocks: bool,
    #[serde_inline_default(false)]
    pub avoid_portals: bool,
    #[serde_inline_default(false)]
    pub avoid_sun: bool,
    #[serde_inline_default(false)]
    pub avoid_water: bool,
    #[serde(default)]
    pub blocks_to_avoid: Vec<String>,
    #[serde_inline_default(false)]
    pub can_breach: bool,
    #[serde_inline_default(false)]
    pub can_break_doors: bool,
    #[serde_inline_default(true)]
    pub can_jump: bool,
    #[serde_inline_default(false)]
    pub can_open_doors: bool,
    #[serde_inline_default(false)]
    pub can_open_iron_doors: bool,
    #[serde_inline_default(true)]
    pub can_pass_doors: bool,
    #[serde_inline_default(false)]
    pub can_path_from_air: bool,
    #[serde_inline_default(false)]
    pub can_path_over_lava: bool,
    #[serde_inline_default(false)]
    pub can_path_over_water: bool,
    #[serde_inline_default(true)]
    pub can_sink: bool,
    #[serde_inline_default(false)]
    pub can_swim: bool,
    #[serde_inline_default(true)]
    pub can_walk: bool,
    #[serde_inline_default(false)]
    pub can_walk_in_lava: bool,
    #[serde_inline_default(false)]
    pub is_amphibious: bool,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct OutOfControl {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Peek {
    #[serde(default)]
    pub on_close: MinecraftEvent,
    #[serde(default)]
    pub on_open: MinecraftEvent,
    #[serde(default)]
    pub on_target_open: MinecraftEvent,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Persistent {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Physics {
    #[serde_inline_default(true)]
    pub has_collision: bool,
    #[serde_inline_default(true)]
    pub has_gravity: bool,
    #[serde_inline_default(false)]
    pub push_towards_closest_space: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PlayerExhaustion {
    #[serde(default)]
    pub max: i32,
    #[serde(default)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PlayerExperience {
    #[serde_inline_default(5)]
    pub max: i32,
    #[serde_inline_default(1)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PlayerLevel {
    #[serde(default)]
    pub max: i32,
    #[serde(default)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PlayerSaturation {
    #[serde(default)]
    pub max: i32,
    #[serde(default)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PreferredPath {
    #[serde(default)]
    pub default_block_cost: f64,
    #[serde(default)]
    pub jump_cost: i32,
    #[serde_inline_default(3)]
    pub max_fall_blocks: i32,
    #[serde(default)]
    pub preferred_path_blocks: Vec<String>,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ProjectileOnHitDefinitionEvent {
    #[serde(default)]
    pub affect_projectile: bool,
    #[serde(default)]
    pub affect_shooter: bool,
    #[serde(default)]
    pub affect_splash_area: bool,
    #[serde(default)]
    pub splash_area: f64,
    #[serde(default)]
    pub affect_target: bool,
    #[serde(default)]
    pub event_trigger: MinecraftTrigger, //trigger
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ProjectileOnHitImpactDamage {
    #[serde(default)]
    pub catch_fire: bool,
    #[serde_inline_default(true)]
    pub channeling: bool,
    #[serde_inline_default(1.0)]
    pub damage: f64,
    #[serde(default)]
    pub destroy_on_hit: bool,
    #[serde(default)]
    pub destroy_on_hit_requires_damage: bool,
    #[serde(default)]
    pub filter: String,
    #[serde_inline_default(true)]
    pub knockback: bool,
    #[serde_inline_default(0)]
    pub max_critical_damage: i32,
    #[serde_inline_default(0)]
    pub min_critical_damage: i32,
    #[serde_inline_default(2.0)]
    pub power_multiplier: f64,
    #[serde(default)]
    pub semi_random_diff_damage: bool,
    #[serde(default)]
    pub set_last_hurt_requires_damage: bool,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ProjectileOnHitMobEffect {
    #[serde_inline_default(1)]
    pub amplifier: i32,
    #[serde_inline_default(false)]
    pub ambient: bool,
    #[serde_inline_default(1)]
    pub duration: i32,
    #[serde(default)]
    pub duration_easy: i32,
    #[serde_inline_default(800)]
    pub duration_hard: i32,
    #[serde_inline_default(200)]
    pub duration_normal: i32,
    #[serde(default)]
    pub effect: String,
    #[serde(default)]
    pub visible: bool,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ProjectileOnHitParticleOnHit {
    #[serde(default)]
    pub on_entity_hit: bool,
    #[serde(default)]
    pub on_other_hit: bool,
    #[serde(default)]
    pub particle_type: String,
    #[serde_inline_default(6)]
    pub num_particles: i32,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ProjectileOnHitSpawnAoeCloud {
    #[serde_inline_default(true)]
    pub affect_owner: bool,
    #[serde(default)]
    pub color: Vec<f64>,
    #[serde(default)]
    pub duration: i32,
    #[serde(default)]
    pub particle: i32,
    #[serde_inline_default(-1)]
    pub potion: i32,
    #[serde(default)]
    pub radius: f64,
    #[serde_inline_default(-1)]
    pub radius_on_use: i32,
    #[serde(default)]
    pub reapplication_delay: i32,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ProjectileOnHitSpawnChance {
    #[serde(default)]
    pub first_spawn_count: i32,
    #[serde(default)]
    pub first_spawn_percent_chance: f64,
    #[serde_inline_default(32.0)]
    pub second_spawn_chance: f64,
    #[serde(default)]
    pub second_spawn_count: i32,
    #[serde(default)]
    pub spawn_definition: String,
    #[serde(default)]
    pub spawn_baby: bool,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ProjectileOnHit {
    #[serde(default)]
    pub catch_fire: bool,
    #[serde(default)]
    pub definition_event: ProjectileOnHitDefinitionEvent,
    #[serde(default)]
    pub douse_fire: bool,
    #[serde(default)]
    pub freeze_on_hit: bool, //??
    #[serde(default)]
    pub grant_xp: bool, //???
    #[serde(default)]
    pub hurt_owner: bool, //???
    #[serde(default)]
    pub ignite: bool,
    #[serde(default)]
    pub impact_damage: ProjectileOnHitImpactDamage,
    #[serde(default)]
    pub mob_effect: ProjectileOnHitMobEffect,
    #[serde(default)]
    pub on_fire_time: f64,
    #[serde(default)]
    pub particle_on_hit: ProjectileOnHitParticleOnHit,
    #[serde_inline_default(-1)]
    pub potion_effect: i32,
    #[serde(default)]
    pub remove_on_hit: ProjectileOnHitParticleOnHit, //???
    #[serde(default)]
    pub spawn_aoe_cloud: ProjectileOnHitSpawnAoeCloud,
    #[serde(default)]
    pub spawn_chance: ProjectileOnHitSpawnChance,
    #[serde(default)]
    pub stick_in_ground: bool, //???
    #[serde(default)]
    pub teleport_owner: bool,
    #[serde(default)]
    pub thrown_potion_effect: bool, //???
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Projectile {
    #[serde(default)]
    pub anchor: i32,
    #[serde(default)]
    pub angle_offset: f64,
    #[serde(default)]
    pub catch_fire: bool,
    #[serde(default)]
    pub crit_particle_on_hurt: bool,
    #[serde(default)]
    pub destroy_on_hurt: bool,
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub fire_affected_by_griefing: bool,
    #[serde_inline_default(0.05)]
    pub gravity: f64,
    #[serde(default)]
    pub hit_sound: String,
    #[serde(default)]
    pub hit_ground_sound: String,
    #[serde(default)]
    pub homing: bool,
    #[serde_inline_default(0.99)]
    pub inertia: f64,
    #[serde(default)]
    pub is_dangerous: bool,
    #[serde_inline_default(true)]
    pub knockback: bool,
    #[serde(default)]
    pub lightning: bool,
    #[serde_inline_default(0.6)]
    pub liquid_inertia: f64,
    #[serde_inline_default(true)]
    pub multiple_targets: bool,
    #[serde_inline_default(vec![0.0, 0.0, 0.0])]
    pub offset: Vec<f64>,
    #[serde(default)]
    pub on_fire_time: f64,
    #[serde(default)]
    pub on_hit: ProjectileOnHit,
    #[serde(default)]
    pub particle: String,
    #[serde_inline_default(1.3)]
    pub power: f64,
    #[serde(default)]
    pub reflect_on_hurt: bool,
    #[serde(default)]
    pub shoot_sound: String,
    #[serde_inline_default(true)]
    pub shoot_target: bool,
    #[serde(default)]
    pub should_bounce: bool,
    #[serde(default)]
    pub splash_potion: bool,
    #[serde_inline_default(4.0)]
    pub splash_range: f64,
    #[serde(default)]
    pub stop_on_hurt: bool,
    #[serde(default)]
    pub uncertainty_base: f64,
    #[serde(default)]
    pub uncertainty_multiplier: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PushThrough {
    #[serde(default)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Pushable {
    #[serde_inline_default(true)]
    pub is_pushable: bool,
    #[serde_inline_default(true)]
    pub is_pushable_by_piston: bool,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RaidTrigger {
    #[serde(default)]
    pub triggered_event: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RailMovement {
    #[serde_inline_default(0.4)]
    pub max_speed: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RailSensor {
    #[serde(default)]
    pub check_block_types: bool,
    #[serde_inline_default(true)]
    pub eject_on_activate: bool,
    #[serde(default)]
    pub eject_on_deactivate: bool,
    #[serde(default)]
    pub on_activate: MinecraftEvent,
    #[serde(default)]
    pub on_deactivate: MinecraftEvent,
    #[serde_inline_default(true)]
    pub tick_command_block_on_activate: bool,
    #[serde(default)]
    pub tick_command_block_on_deactivate: bool,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct RavagerBlockedReactionChoice {
    #[serde_inline_default(1)]
    pub weight: i32,
    #[serde(default)]
    pub value: MinecraftEvent,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RavagerBlocked {
    #[serde_inline_default(3.0)]
    pub knockback_strength: f64,
    #[serde(default)]
    pub reaction_choices: Vec<RavagerBlockedReactionChoice>,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct RideableSeat {
    #[serde_inline_default(181.0)]
    pub lock_rider_rotation: f64,
    #[serde_inline_default(1)]
    pub max_rider_count: i32,
    #[serde_inline_default(0)]
    pub min_rider_count: i32,
    #[serde_inline_default(vec![0.0, 0.0, 0.0])]
    pub position: Vec<f64>,
    #[serde(default)]
    pub rotate_rider_by: MolangExpression,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Rideable {
    #[serde(default)]
    pub controlling_seat: i32,
    #[serde_inline_default(true)]
    pub crouching_skip_interact: bool,
    #[serde(default)]
    pub family_types: Vec<String>,
    #[serde(default)]
    pub interact_text: String,
    #[serde(default)]
    pub passenger_max_width: f64,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub pull_in_entities: bool,
    #[serde(default)]
    pub rider_can_interact: bool,
    #[serde_inline_default(1)]
    pub seat_count: i32,
    #[serde(default)]
    pub seats: Vec<RideableSeat>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct ScaffoldingClimber {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Scale {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ScaleByAge {
    #[serde_inline_default(1.0)]
    pub end_scale: f64,
    #[serde_inline_default(1.0)]
    pub start_scale: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Scheduler {
    #[serde(default)]
    pub scheduled_events: Vec<MinecraftEvent>,
    #[serde_inline_default(0.0)]
    pub min_delay_secs: f64,
    #[serde_inline_default(0.0)]
    pub max_delay_secs: f64,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ShareablesItem {
    #[serde(default)]
    pub admire: bool,
    #[serde(default)]
    pub barter: bool,
    #[serde(default)]
    pub consume_item: bool,
    #[serde(default)]
    pub craft_into: String,
    #[serde(default)]
    pub item: String,
    #[serde(default)]
    pub max_amount: i32,
    #[serde(default)]
    pub pickup_limit: i32,
    #[serde(default)]
    pub pickup_only: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub singular_pickup: bool,
    #[serde(default)]
    pub stored_in_inventory: bool,
    #[serde(default)]
    pub surplus_amount: i32,
    #[serde(default)]
    pub want_amount: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Shareables {
    #[serde_inline_default(false)]
    pub all_items: bool,
    #[serde_inline_default(-1)]
    pub all_items_max_amount: i32,
    #[serde_inline_default(-1)]
    pub all_items_surplus_amount: i32,
    #[serde_inline_default(-1)]
    pub all_items_want_amount: i32,
    #[serde(default)]
    pub items: Vec<ShareablesItem>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Shooter {
    #[serde_inline_default(-1)]
    pub aux_val: i32,
    #[serde(default)]
    pub def: String,
    #[serde(default)]
    pub magic: bool,
    #[serde(default)]
    pub power: f64,
    #[serde(default)]
    pub projectiles: Vec<Projectile>,
    #[serde(default)]
    pub sound: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Sittable {
    #[serde(default)]
    pub sit_event: MinecraftEvent,
    #[serde(default)]
    pub stand_event: MinecraftEvent,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SkinId {
    #[serde_inline_default(0)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SoundVolume {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SpawnEntityEntity {
    #[serde(default)]
    pub filters: MinecraftFilter,
    #[serde_inline_default(600)]
    pub max_wait_time: i32,
    #[serde_inline_default(300)]
    pub min_wait_time: i32,
    #[serde_inline_default(1)]
    pub num_to_spawn: i32,
    #[serde(default)]
    pub should_leash: bool,
    #[serde(default)]
    pub single_use: bool,
    #[serde(default)]
    pub spawn_entity: String,
    #[serde_inline_default("minecraft:entity_born".to_string())]
    pub spawn_event: String,
    #[serde_inline_default("egg".to_string())]
    pub spawn_item: String,
    #[serde(default)]
    pub spawn_item_event: MinecraftEvent,
    #[serde_inline_default("born".to_string())]
    pub spawn_method: String,
    #[serde_inline_default("plop".to_string())]
    pub spawn_sound: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SpawnEntity {
    #[serde(default)]
    pub entities: Vec<SpawnEntityEntity>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SpellEffectsAddEffect {
    #[serde(default)]
    pub effect: String,
    #[serde_inline_default(0.05)]
    pub duration: f64,
    #[serde_inline_default(0)]
    pub amplifier: i32,
    #[serde_inline_default(false)]
    pub ambient: bool,
    #[serde_inline_default(true)]
    pub visible: bool,
    #[serde_inline_default(false)]
    pub display_on_screen_animation: bool,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct SpellEffects {
    #[serde(default)]
    pub add_effects: Vec<SpellEffectsAddEffect>,
    pub remove_effects: Vec<String>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Strength {
    #[serde_inline_default(5)]
    pub max: i32,
    #[serde_inline_default(1)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Tameable {
    #[serde_inline_default(1.0)]
    pub probability: f64,
    #[serde(default)]
    pub tame_event: MinecraftEvent,
    #[serde(default)]
    pub tame_items: Vec<String>,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct TamemountAutoRejectItems {
    #[serde(default)]
    pub items: Vec<String>,
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct TamemountFeedItems {
    #[serde(default)]
    pub items: Vec<String>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Tamemount {
    #[serde_inline_default(5)]
    pub attempt_temper_mod: i32,
    #[serde(default)]
    pub auto_reject_items: TamemountAutoRejectItems,
    #[serde(default)]
    pub feed_items: TamemountFeedItems,
    #[serde(default)]
    pub feed_text: String,
    #[serde_inline_default(100)]
    pub max_temper: i32,
    #[serde_inline_default(0)]
    pub min_temper: i32,
    #[serde(default)]
    pub ride_text: String,
    #[serde(default)]
    pub tame_event: MinecraftEvent,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct TargetNearbySensor {
    #[serde_inline_default(1.0)]
    pub inside_range: f64,
    #[serde_inline_default(false)]
    pub must_see: bool,
    #[serde(default)]
    pub on_inside_range: MinecraftEvent,
    #[serde(default)]
    pub on_outside_range: MinecraftEvent,
    #[serde(default)]
    pub on_vision_lost_inside_range: MinecraftEvent,
    #[serde_inline_default(5.0)]
    pub outside_range: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Teleport {
    #[serde_inline_default(0.01)]
    pub dark_teleport_chance: f64,
    #[serde_inline_default(0.01)]
    pub light_teleport_chance: f64,
    #[serde_inline_default(20.0)]
    pub max_random_teleport_time: f64,
    #[serde_inline_default(0.0)]
    pub min_random_teleport_time: f64,
    #[serde_inline_default([32.0, 16.0, 32.0])]
    pub random_teleport_cube: [f64; 3],
    #[serde_inline_default(true)]
    pub random_teleports: bool,
    #[serde_inline_default(16.0)]
    pub target_distance: f64,
    #[serde_inline_default(1.0)]
    pub target_teleport_chance: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct TickWorld {
    #[serde_inline_default(128.0)]
    pub distance_to_players: f64,
    #[serde_inline_default(true)]
    pub never_despawn: bool,
    #[serde_inline_default(2.0)]
    pub radius: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Timer {
    #[serde_inline_default(true)]
    pub looping: bool,
    #[serde_inline_default(true)]
    pub random_interval: bool,
    #[serde(default)]
    pub random_time_choices: Vec<MinecraftRangeType<f64>>,
    #[serde(default)]
    pub time: MinecraftRangeType<f64>,
    #[serde(default)]
    pub time_down_event: MinecraftEvent,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct TradeResupply {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct TradeTable {
    #[serde_inline_default(false)]
    pub convert_trades_economy: bool,
    #[serde(default)]
    pub display_name: String,
    #[serde_inline_default(false)]
    pub new_screen: bool,
    #[serde_inline_default(false)]
    pub persist_trades: bool,
    #[serde(default)]
    pub table: String,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Trail {
    #[serde(default)]
    pub block_type: String,
    #[serde(default)]
    pub spawn_filter: MinecraftFilter,
    #[serde(default)]
    pub spawn_offset: [f64; 3],
}

#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct TransformationDelay {
    #[serde_inline_default(0.0)]
    pub block_assist_chance: f64,
    #[serde_inline_default(0.0)]
    pub block_chance: f64,
    #[serde_inline_default(0)]
    pub block_max: i32,
    #[serde_inline_default(0)]
    pub block_radius: i32,
    #[serde(default)]
    pub block_types: Vec<String>,
    #[serde_inline_default(0.0)]
    pub range_max: f64,
    #[serde_inline_default(0.0)]
    pub range_min: f64,
    #[serde_inline_default(0.0)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Transformation {
    #[serde(default)]
    pub begin_transform_sound: String,
    #[serde_inline_default(false)]
    pub drop_equipment: bool,
    #[serde_inline_default(false)]
    pub drop_inventory: bool,
    #[serde(default)]
    pub into: String,
    #[serde_inline_default(false)]
    pub keep_level: bool,
    #[serde_inline_default(false)]
    pub keep_owner: bool,
    #[serde_inline_default(false)]
    pub preserve_equipment: bool,
    #[serde(default)]
    pub transformation_sound: String,
    #[serde(default)]
    pub delay: TransformationDelay,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Trust {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Trusting {
    #[serde_inline_default(1.0)]
    pub probability: f64,
    #[serde(default)]
    pub trust_event: MinecraftEvent,
    #[serde(default)]
    pub trust_items: Vec<String>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct TypeFamily {
    #[serde(default)]
    pub family: Vec<String>,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct UnderwaterMovement {
    #[serde(default)]
    pub value: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Variant {
    #[serde_inline_default(0)]
    pub value: i32,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct WalkAnimationSpeed {
    #[serde_inline_default(0.0)]
    pub value: f64,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct WantsJockey {}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct WaterMovement {
    #[serde_inline_default(0.8)]
    pub drag_factor: f64,
}

#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct VariableMaxAutoStep {
    #[serde_inline_default(0.5625)]
    pub base_value: f64,
    #[serde_inline_default(0.5625)]
    pub controlled_value: f64,
    #[serde_inline_default(0.5625)]
    pub jump_prevented_value: f64,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct CannotBeAttacked {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct IgnoreCannotBeAttacked {}
