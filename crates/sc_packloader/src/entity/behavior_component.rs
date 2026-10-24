//! 实体行为组件数据模型：与官方 Bedrock 实体 JSON 行为组件参考一一对应的
//! serde 反序列化结构体，描述实体的 AI 行为目标与触发器。
//!
//! Entity behavior component data model: serde deserialization structs
//! mirroring the official Bedrock entity JSON behavior component reference,
//! describing entity AI goals and triggers.

use crate::types::entity_types::MinecraftEntityTypes;
use crate::types::event::MinecraftEvent;
use crate::types::filter::MinecraftFilterType;
use crate::types::range::MinecraftRangeType;
use crate::types::trigger::MinecraftTrigger;
use sc_ecs::component::Component;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

/// 行为组件：停步欣赏（注视）附近的掉落物。
///
/// Behavior component: stop and admire nearby dropped items.
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
/// 行为组件：避开指定类型的方块。
///
/// Behavior component: avoid blocks of the specified types.
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

/// 行为组件：躲避符合过滤条件的实体类型。
///
/// Behavior component: flee from entity types matching the filters.
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

/// 行为组件：以物易物（猪灵与玩家交换物品）。
///
/// Behavior component: barter (piglin trades items with players).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorBarter {
    #[serde(default)]
    pub priority: i32,
}

/// 行为组件：乞食（狼停步仰望玩家索要食物）。
///
/// Behavior component: beg (wolf stops and looks up at the player for food).
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

/// 行为组件：繁殖（进入繁殖模式并靠近同类）。
///
/// Behavior component: enter breeding mode and move toward the same species.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorBreed {
    #[serde(default)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

/// 行为组件：庆祝（跳跃并播放庆祝音效）。
///
/// Behavior component: celebrate by jumping and playing a celebration sound.
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

/// 行为组件：劫后庆祝（袭击/战斗结束后庆祝存活）。
///
/// Behavior component: celebrate surviving after a raid or fight.
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

/// 行为组件：蓄力冲撞攻击（后退蓄力后向前冲撞目标）。
///
/// Behavior component: charge attack (back up and charge forward at the target).
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

/// 行为组件：捡起并举起指定物品（如幼年海龟被捡起）。
///
/// Behavior component: pick up and hold the specified items.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorChargeHeldItem {
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub priority: i32,
}

/// 行为组件：绕锚点盘旋飞行（悦灵绕物品栏容器盘旋）。
///
/// Behavior component: circle around an anchor while flying.
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

/// 行为组件：受玩家控制（坐骑接受玩家移动指令）。
///
/// Behavior component: controlled by the player (mount obeys movement input).
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

/// 行为组件：呱呱鸣叫（青蛙发出叫声）。
///
/// Behavior component: croak (frog emits its call sound).
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

/// 行为组件：保卫信任的目标（如羊驼保护主人）。
///
/// Behavior component: defend a trusted target from attackers.
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

/// 行为组件：保卫村庄目标（铁傀儡反击袭击者）。
///
/// Behavior component: defend the village target (iron golem counterattacks raiders).
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

/// 行为组件：延迟攻击（带前摇后摇与寻路边界控制的近战攻击）。
///
/// Behavior component: delayed melee attack with wind-up, recovery and pathing bounds.
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

/// 行为组件：挖掘（潜行/掘地行为，受惊时钻入地下）。
///
/// Behavior component: dig (burrow behavior when startled or suspicious).
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

/// 行为组件：喝奶（清除自身药效并进入冷却）。
///
/// Behavior component: drink milk to clear own effects and enter cooldown.
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

/// 行为组件：末影龙向玩家俯冲冲撞。
///
/// Behavior component: ender dragon charges at the player.
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

/// 行为组件：末影龙死亡（播放死亡序列）。
///
/// Behavior component: ender dragon death sequence.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragondeath {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：末影龙喷吐烈焰（火球与地面火焰）。
///
/// Behavior component: ender dragon flaming (fireball and ground flames).
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

/// 行为组件：末影龙保持盘旋（维持飞行高度与位置）。
///
/// Behavior component: ender dragon holding pattern (maintain flight position).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonHoldingPattern {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：末影龙着陆（飞向传送门并降落）。
///
/// Behavior component: ender dragon landing.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonLanding {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：末影龙盘旋扫描（搜索目标）。
///
/// Behavior component: ender dragon scanning for targets.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonScanning {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：末影龙侧移扫射玩家（火球扫射）。
///
/// Behavior component: ender dragon strafes the player with fireballs.
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

/// 行为组件：末影龙起飞（从基岩柱起飞）。
///
/// Behavior component: ender dragon takeoff.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorDragonTakeoff {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：饮用治疗/增益药水（女巫战斗中喝药）。
///
/// Behavior component: drink a healing or buff potion (witch drinks in combat).
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

/// 行为组件：从药水列表中按概率饮用一种药水。
///
/// Behavior component: drink a random potion from the potion list by chance.
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

/// 行为组件：向指定实体投掷掉落物作为祭品。
///
/// Behavior component: drop items as an offering to the target entity.
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

/// 行为组件：吃掉一种方块后替换为另一种方块的配对。
///
/// Behavior component: pair of a block to eat and the block to replace it with.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct BehaviorEatBlockEatAndReplaceBlockPair {
    #[serde(default)]
    pub eat_block: String,
    #[serde(default)]
    pub replace_block: String,
}

/// 行为组件：吃掉方块并按配对表替换（绵羊吃草）。
///
/// Behavior component: eat a block and replace it per the pair list (sheep graze).
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

/// 行为组件：吃掉叼在口中的物品（狐狸吃完浆果）。
///
/// Behavior component: eat the item held in the mouth.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEatCarriedItem {
    #[serde_inline_default(0.0)]
    pub delay_before_eating: f64,
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：捕食其他实体（青蛙吃史莱姆）。
///
/// Behavior component: prey on and eat other mobs (frog eats slime).
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

/// 行为组件：从地面钻出（监守者破土而出）。
///
/// Behavior component: emerge from the ground (warden breaks through).
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

/// 行为组件：末影人放下携带的方块。
///
/// Behavior component: enderman places the block it is carrying.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEndermanLeaveBlock {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：末影人捡起方块。
///
/// Behavior component: enderman picks up a block.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEndermanTakeBlock {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：装备物品（捡起装备并穿戴）。
///
/// Behavior component: equip an item (pick up gear and wear it).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorEquipItem {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：探索村庄外围区域。
///
/// Behavior component: explore the outskirts of a village.
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

/// 行为组件：为农田施肥（村民催熟作物）。
///
/// Behavior component: fertilize farm blocks (villager speeds up crops).
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

/// 行为组件：寻找掩体躲避（受到攻击时寻找遮蔽）。
///
/// Behavior component: find cover to hide from attackers.
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

/// 行为组件：寻找可骑乘的坐骑。
///
/// Behavior component: find a mount to ride.
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

/// 行为组件：寻找水下宝藏（海豚引导玩家寻宝）。
///
/// Behavior component: find underwater treasure (dolphin leads to treasure).
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

/// 行为组件：逃离阳光（亡灵生物白天躲避日晒）。
///
/// Behavior component: flee from sunlight (undead mobs avoid daylight).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFleeSun {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

/// 行为组件：在水中漂浮（避免下沉溺水）。
///
/// Behavior component: float in water to avoid sinking.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFloat {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(false)]
    pub sink_with_passengers: bool,
}

/// 行为组件：漂浮游荡（在水中随机漂移）。
///
/// Behavior component: float and wander aimlessly in water.
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

/// 行为组件：跟随商队（羊驼跟随其他羊驼移动）。
///
/// Behavior component: follow the caravan (llamas follow other llamas).
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

/// 行为组件：跟随同类实体。
///
/// Behavior component: follow another mob of the same type.
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

/// 行为组件：跟随主人（距离过远时传送回来）。
///
/// Behavior component: follow the owner (teleport back when too far away).
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

/// 行为组件：跟随父母（幼年个体跟随成年个体）。
///
/// Behavior component: follow the parent (young follow adults).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorFollowParent {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

/// 行为组件：跟随袭击队长（卫道士跟随灾厄旗帜持有者）。
///
/// Behavior component: follow the raid captain (vindicator follows the banner carrier).
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

/// 行为组件：前往音符盒并投掷物品（悦灵收到音符后献上物品）。
///
/// Behavior component: go to a noteblock and throw items (allay offers items on sound).
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

/// 行为组件：奔向主人并把物品投给主人（悦灵献上物品）。
///
/// Behavior component: run to the owner and throw items to the owner (allay offering).
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
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    pubrun_speed: f64,
    #[serde_inline_default(0.2)]
    pub throw_force: f64,
    #[serde(default)]
    pub throw_sound: String,
    #[serde_inline_default(1.5)]
    pub vertical_throw_mul: f64,
}

/// 行为组件：返回巢穴/家园位置。
///
/// Behavior component: return to the home position.
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

/// 行为组件：守卫者激光攻击（远古守卫者伤害更高）。
///
/// Behavior component: guardian laser attack (elder guardians hit harder).
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

/// 行为组件：收获成熟的农作物。
///
/// Behavior component: harvest fully grown crops.
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

/// 行为组件：躲藏到兴趣点/掩体中（村民遇袭时逃跑躲藏）。
///
/// Behavior component: hide at a POI or cover when threatened (villager fleeing).
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

/// 行为组件：固守阵地（保持在半径内不后退，可广播警报）。
///
/// Behavior component: hold ground within a radius and optionally broadcast alarms.
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

/// 行为组件：反击伤害来源（受击后报复攻击者）。
///
/// Behavior component: retaliate against the source of damage.
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

/// 行为组件：查看书架（图书管理员整理工作站）。
///
/// Behavior component: inspect the bookshelf (librarian tidies the workstation).
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

/// 行为组件：调查可疑地点（循声传来的声响/振动的来源）。
///
/// Behavior component: investigate a suspicious location (source of a sound/vibration).
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

/// 行为组件：跳跃到方块上（青蛙跳上睡莲等目标方块）。
///
/// Behavior component: jump onto blocks (frog jumps onto lily pads and targets).
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

/// 行为组件：击退咆哮（监守者震退范围内实体）。
///
/// Behavior component: knockback roar (warden pushes entities away).
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

/// 行为组件：躺下休息（猫、熊猫等趴下）。
///
/// Behavior component: lay down and rest (cat, panda, etc.).
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

/// 行为组件：产卵（海龟返回出生地沙滩下蛋）。
///
/// Behavior component: lay eggs (turtle returns to its home beach to lay eggs).
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

/// 行为组件：向目标扑跃（猫扑向玩家）。
///
/// Behavior component: leap at the target (cat pounces on a player).
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

/// 行为组件：注视符合过滤条件的实体。
///
/// Behavior component: look at entities matching the filters.
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

/// 行为组件：注视附近的玩家。
///
/// Behavior component: look at a nearby player.
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

/// 行为组件：注视当前攻击目标。
///
/// Behavior component: look at the current target.
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

/// 行为组件：注视正在交易的玩家。
///
/// Behavior component: look at the trading player.
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

/// 行为组件：求偶（被喂食后进入恋爱模式）。
///
/// Behavior component: make love (enter love mode after being fed).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMakeLove {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：近战攻击目标。
///
/// Behavior component: melee attack the target.
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

/// 行为组件：箱形范围近战攻击（可同时命中箱体内多个目标）。
///
/// Behavior component: melee box attack (hits targets inside a box around the entity).
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

/// 行为组件：与同伴嬉戏（村民社交走动）。
///
/// Behavior component: mingle with fellow villagers (social walking).
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

/// 行为组件：坐骑寻路（载着骑手时的寻路行为）。
///
/// Behavior component: mount pathing while carrying a rider.
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

/// 行为组件：移动到室内（村民下雨/夜晚进屋）。
///
/// Behavior component: move indoors (villagers shelter from rain or night).
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

/// 行为组件：移动到室外。
///
/// Behavior component: move outdoors.
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

/// 行为组件：穿过村庄移动（僵尸围城时径直穿过）。
///
/// Behavior component: move through the village.
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

/// 行为组件：移动到指定方块处并可停留。
///
/// Behavior component: move to a target block and optionally stay there.
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

/// 行为组件：移动到陆地（水生生物上岸）。
///
/// Behavior component: move to land (aquatic mob goes ashore).
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

/// 行为组件：移动到液体旁。
///
/// Behavior component: move to a liquid.
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

/// 行为组件：移动到随机方块处。
///
/// Behavior component: move to a random block.
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

/// 行为组件：移动到村庄。
///
/// Behavior component: move to a village.
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

/// 行为组件：移动到水源。
///
/// Behavior component: move to water.
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

/// 行为组件：向住所限制区域移动（蜜蜂回巢）。
///
/// Behavior component: move towards the dwelling restriction (bee returns to its hive).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveTowardsDwellingRestriction {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

/// 行为组件：向家园限制区域移动。
///
/// Behavior component: move towards the home restriction.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveTowardsHomeRestriction {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

/// 行为组件：向当前目标移动。
///
/// Behavior component: move towards the current target.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorMoveTowardsTarget {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0.0)]
    pub within_radius: f64,
}

/// 行为组件：打盹小睡（猫在白天睡觉）。
///
/// Behavior component: nap (cats sleep during the day).
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

/// 行为组件：寻找并攻击最近的可攻击目标。
///
/// Behavior component: find and attack the nearest attackable target.
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

/// 行为组件：寻找并攻击最近的优先排序目标。
///
/// Behavior component: find and attack the nearest prioritized attackable target.
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

/// 行为组件：豹猫坐在方块上（潜行靠近玩家）。
///
/// Behavior component: ocelot sits on a block.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorOcelotSitOnBlock {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

/// 行为组件：豹猫攻击（潜行接近后突袭鸡等猎物）。
///
/// Behavior component: ocelot attack (sneak up and pounce on prey like chickens).
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

/// 行为组件：向村民献花（铁傀儡的安抚动作）。
///
/// Behavior component: offer a flower (iron golem calms villagers).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorOfferFlower {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：打开与关闭门。
///
/// Behavior component: open and close doors.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorOpenDoor {
    #[serde_inline_default(true)]
    pub close_door_after: bool,
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：反击伤害主人的实体。
///
/// Behavior component: retaliate against whoever hurt the owner.
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

/// 行为组件：攻击主人当前注视/选定的目标。
///
/// Behavior component: attack whatever the owner targets.
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

/// 行为组件：惊慌逃跑（受伤时高速逃离）。
///
/// Behavior component: panic and flee when hurt.
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

/// 行为组件：探头张望（潜影贝开合探头）。
///
/// Behavior component: peek (shulker opens and closes its head).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorPeek {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：宠物与主人一起睡觉（猫跳上床陪睡）。
///
/// Behavior component: pet sleeps with the owner (cat hops onto the bed).
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

/// 行为组件：拾取地上的掉落物。
///
/// Behavior component: pick up items from the ground.
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

/// 行为组件：与同伴玩耍（幼年动物互相追逐嬉戏）。
///
/// Behavior component: play with friends (baby animals chase each other).
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

/// 行为组件：装死（嗅梦兽受伤后倒地装死）。
///
/// Behavior component: play dead (sniffer lies down when hurt).
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

/// 行为组件：骑乘已驯服的坐骑。
///
/// Behavior component: player rides the tamed mount.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorPlayerRideTamed {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：袭击菜园（兔子啃食胡萝卜田）。
///
/// Behavior component: raid the garden (rabbit eats carrot crops).
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

/// 行为组件：冲撞攻击（山羊蓄力冲撞目标）。
///
/// Behavior component: ram attack (goat charges and knocks back the target).
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

/// 行为组件：随机跃出水面（海豚跃出水面又落回）。
///
/// Behavior component: randomly breach out of the water (dolphin jumps out).
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

/// 行为组件：随机飞行（可在树木上停落）。
///
/// Behavior component: fly randomly (can land on trees).
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

/// 行为组件：随机悬停（在范围内上下浮动）。
///
/// Behavior component: hover randomly within a height range.
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

/// 行为组件：随机张望并坐下（狼、猫的待机行为）。
///
/// Behavior component: look around randomly and sit (idle behavior of wolves and cats).
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

/// 行为组件：随机环顾四周。
///
/// Behavior component: look around randomly.
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

/// 行为组件：随机搜索并挖掘（嗅梦兽挖出种子）。
///
/// Behavior component: search randomly and dig (sniffer digs up seeds).
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

/// 行为组件：随机坐下（猫、狼随机蹲坐）。
///
/// Behavior component: sit down randomly (cats and wolves sit at random).
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

/// 行为组件：随机漫步。
///
/// Behavior component: stroll around randomly.
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

/// 行为组件：随机游动（在水中随机巡游）。
///
/// Behavior component: swim around randomly in water.
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

/// 行为组件：远程攻击目标（骷髅射箭、流浪者射箭）。
///
/// Behavior component: ranged attack the target (skeleton or stray shooting arrows).
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

/// 行为组件：限制在阳光下活动（亡灵生物躲避日晒）。
///
/// Behavior component: restrict movement to sunlight (undead avoid daylight).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRestrictSun {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：随液体液面升降（鱼随水位浮沉）。
///
/// Behavior component: rise and sink with the liquid level.
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

/// 行为组件：咆哮（敌对生物的威吓动作）。
///
/// Behavior component: roar (intimidating action of a hostile mob).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRoar {
    #[serde_inline_default(0.0)]
    pub duration: f64,
}

/// 行为组件：翻滚（熊猫翻滚动作）。
///
/// Behavior component: roll (panda rolling action).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRoll {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub probability: f64,
}

/// 行为组件：疯狂乱跑（未驯服的马受惊后脱缰奔跑）。
///
/// Behavior component: run around like crazy (untamed horse bolts in panic).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRunAroundLikeCrazy {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

/// 行为组件：受惊逃跑（兔子受惊后窜逃）。
///
/// Behavior component: get scared and flee (rabbit bolts when frightened).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorScared {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(0)]
    pub sound_interval: i32,
}

/// 行为组件：按序列发送事件（唤魔者召唤时播放仪式）。
///
/// Behavior component: send events in a sequence (evoker's summoning ritual).
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

/// 行为组件：与同类分享物品（村民互相分享食物）。
///
/// Behavior component: share items with fellow villagers.
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

/// 行为组件：蠹虫钻回石块中躲藏。
///
/// Behavior component: silverfish merges back into stone to hide.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSilverfishMergeWithStone {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：唤醒附近石块中的蠹虫同伴。
///
/// Behavior component: silverfish wakes up friends hidden in nearby stone.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSilverfishWakeUpFriends {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：骷髅马陷阱（天雷生成骷髅与骷髅骑手）。
///
/// Behavior component: skeleton horse trap (lightning spawns a skeleton rider).
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

/// 行为组件：睡觉（村民、狐狸夜间入睡）。
///
/// Behavior component: sleep (villagers and foxes sleep at night).
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

/// 行为组件：史莱姆扑向目标造成接触伤害。
///
/// Behavior component: slime leaps at the target to deal contact damage.
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

/// 行为组件：史莱姆在水中漂浮并随机跳跃。
///
/// Behavior component: slime floats in water and jumps randomly.
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

/// 行为组件：史莱姆持续向前跳跃。
///
/// Behavior component: slime keeps jumping forward.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSlimeKeepOnJumping {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

/// 行为组件：史莱姆随机改变移动方向。
///
/// Behavior component: slime changes direction randomly.
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

/// 行为组件：吃零食（嗅梦兽蹲下啃食种子）。
///
/// Behavior component: snacking (sniffer crouches and nibbles seeds).
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

/// 行为组件：打喷嚏（嗅梦兽喷出种子）。
///
/// Behavior component: sneeze (sniffer blows out seeds).
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

/// 行为组件：嗅探（嗅梦兽低头嗅闻周围）。
///
/// Behavior component: sniff (sniffer lowers its head to smell around).
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

/// 行为组件：声波冲击（监守者的远程音波攻击）。
///
/// Behavior component: sonic boom (warden's ranged sonic attack).
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

/// 行为组件：鱿鱼向深处下潜。
///
/// Behavior component: squid dives deeper.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidDive {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：鱿鱼受惊后游开逃离。
///
/// Behavior component: the squid swims away to flee.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidFlee {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：鱿鱼原地漂浮待命。
///
/// Behavior component: the squid idles and drifts in place.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidIdle {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：鱿鱼远离地面方块并回到水中。
///
/// Behavior component: the squid moves away from ground blocks and back into water.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidMoveAwayFromGround {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：鱿鱼离水后贴附在地面上。
///
/// Behavior component: the squid sticks to the ground while outside water.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSquidOutOfWater {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：潜行接近目标，进入扑击距离后跃起扑向目标。
///
/// Behavior component: stalk the target and pounce on it once within pounce range.
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

/// 行为组件：待在最近被敲响的音符盒附近，并把背包物品丢给它。
///
/// Behavior component: stay near a recently played note block and toss inventory items to it.
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

/// 行为组件：坐下状态下保持原地不动，阻止其他行为移动实体。
///
/// Behavior component: stay put while sitting, preventing other behaviors from moving the entity.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorStayWhileSitting {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：跺脚造成范围伤害（北极熊）。
///
/// Behavior component: stomp AoE damage attack (polar bear).
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

/// 行为组件：寻找并踩碎海龟蛋（溺尸）。
///
/// Behavior component: find and stomp turtle eggs (drowned).
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

/// 行为组件：走进搜索范围内村庄的随机位置。
///
/// Behavior component: move into a random location within a village in the search range.
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

/// 召唤实体序列：单次召唤的实体类型、数量、延迟、寿命与排列方式。
///
/// Summon entity sequence: entity type, count, delay, lifespan and formation of a single summon.
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

/// 召唤实体选项：可用的召唤序列及其触发条件、权重与施法表现。
///
/// Summon entity choices: available summon sequences with their filters, weight and casting presentation.
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

/// 行为组件：召唤其他实体（唤魔者等）。
///
/// Behavior component: summon other entities (evoker and similar).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorSummonEntity {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde(default)]
    pub summon_choices: BehaviorSummonEntitySummonChoices,
}

/// 行为组件：附近有目标时鼓胀蓄力（苦力怕）。
///
/// Behavior component: swell up when a target is nearby (creeper).
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

/// 行为组件：在水中原地待命。
///
/// Behavior component: go idle while swimming.
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

/// 行为组件：在水中游荡（未寻路时随机漂移）。
///
/// Behavior component: wander around while swimming when not path-finding.
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

/// 行为组件：在水中跟随另一实体一同游动（海豚）。
///
/// Behavior component: follow another entity while both are swimming (dolphin).
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

/// 行为组件：俯冲攻击（幻翼等飞行生物）。
///
/// Behavior component: swoop attack (phantom and other flying mobs).
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

/// 行为组件：接受其他实体递来的花（村民）。
///
/// Behavior component: accept a flower offered by another mob (villager).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorTakeFlower {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：被推挤时反过来锁定推挤者为目标（铁傀儡）。
///
/// Behavior component: target the mob that pushed it when pushed (iron golem).
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

/// 行为组件：被手持指定物品的玩家引诱并跟随。
///
/// Behavior component: be tempted by a player holding the specified items and follow them.
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

/// 行为组件：计时标记 1，开始时触发事件，持续指定时长后结束时触发事件。
///
/// Behavior component: timer flag 1, fires an event on start and another when its duration elapses.
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

/// 行为组件：计时标记 2，开始时触发事件，持续指定时长后结束时触发事件。
///
/// Behavior component: timer flag 2, fires an event on start and another when its duration elapses.
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

/// 行为组件：计时标记 3，开始时触发事件，持续指定时长后结束时触发事件。
///
/// Behavior component: timer flag 3, fires an event on start and another when its duration elapses.
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

/// 行为组件：注视手持可交易物品的玩家并展示商品（村民）。
///
/// Behavior component: look at a player holding a tradable item and show trade items (villager).
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

/// 行为组件：允许玩家与该实体交易，开始时中断其寻路。
///
/// Behavior component: allow the player to trade with this mob, stopping its navigation while active.
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

/// 行为组件：把主人正在攻击的实体作为自己的目标（恼鬼）。
///
/// Behavior component: target the same entity its owner is targeting (vex).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorVexCopyOwnerTarget {
    #[serde(default)]
    pub entity_types: MinecraftEntityTypes,
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：像恼鬼一样随机移动。
///
/// Behavior component: move around randomly like the vex.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorVexRandomMove {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：凋灵随机发起攻击。
///
/// Behavior component: the wither launches random attacks.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorWitherRandomAttackPosGoal {
    #[serde_inline_default(0)]
    pub priority: i32,
}

/// 行为组件：凋灵优先攻击对它造成伤害最多的实体。
///
/// Behavior component: the wither focuses on the mob that dealt it the most damage.
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

/// 行为组件：使用工作点（POI）从事职业工作（村民）。
///
/// Behavior component: use the jobsite POI to perform work (villager).
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

/// 行为组件：使用堆肥桶工作点把多余种子转化为骨粉（村民）。
///
/// Behavior component: use the composter POI to convert excess seeds into bone meal (villager).
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

/// 行为组件：停下脚步以便另一只村民与之繁殖。
///
/// Behavior component: stop so another villager can breed with it.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorReceiveLove {
    #[serde_inline_default(0)]
    pub priority: i32,
    #[serde_inline_default(1.0)]
    pub speed_multiplier: f64,
}

/// 行为组件：夜间待在室内，限制开门外出。
///
/// Behavior component: stay indoors during night time, restricting door use.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BehaviorRestrictOpenDoor {
    #[serde_inline_default(0)]
    pub priority: i32,
}
