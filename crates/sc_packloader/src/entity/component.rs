//! 实体组件数据模型：与官方 Bedrock 实体 JSON 组件参考一一对应的 serde
//! 反序列化结构体，用于把行为包实体定义加载为 ECS 组件。
//!
//! Entity component data model: serde deserialization structs mirroring the
//! official Bedrock entity JSON component reference, used to load behavior
//! pack entity definitions as ECS components.

use crate::ident::block_ident::MinecraftBlockIdent;
use crate::types::event::MinecraftEvent;
use crate::types::filter::{MinecraftFilter, MinecraftFilterType};
use crate::types::molang::MolangExpression;
use crate::types::range::{MinecraftRange, MinecraftRangeType};
use crate::types::trigger::MinecraftTrigger;
use sc_ecs::component::Component;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

/// 实体组件：附加骑乘者——实体生成时额外生成一只骑乘它的实体。
///
/// Entity component: add_rider — spawns an additional entity riding on top
/// of this entity when it spawns.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AddRider {
    pub entity_type: String,
    pub spawn_event: String,
}

/// 实体组件：欣赏物品——实体停下并注视手持的物品（如幼年村民看着礼物，
/// 或手持三文鱼的北极熊）。
///
/// Entity component: admire_item — the entity stops and admires the item it
/// is holding (e.g. baby villagers looking at gifts).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AdmireItem {
    /// 被攻击后重新进入欣赏状态前需要等待的冷却时间。
    // Cooldown before admiring again after being attacked.
    #[serde(default)]
    pub cooldown_after_being_attacked: i32,
    #[serde_inline_default(10)]
    pub duration: i32,
}

/// 实体组件：可成长——实体从幼年状态随时间成长为成年状态。
///
/// Entity component: ageable — the entity grows from baby to adult over time.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Ageable {
    pub cooldown_after_being_attacked: i32,
    pub duration: i32,
}

/// 环境音事件名嵌套结构：环境音间隔组件的单个事件项，包含事件名与触发条件。
///
/// Nested struct for ambient sound interval: a single event entry with its
/// event name and trigger condition.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct AmbientSoundIntervalEventNames {
    #[serde(default)]
    pub event_name: String,
    #[serde(default)]
    pub condition: MolangExpression,
}

/// 实体组件：环境音间隔——按设定间隔播放实体周身的环境音效。
///
/// Entity component: ambient_sound_interval — plays ambient sound effects
/// around the entity at a configured interval.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AmbientSoundInterval {
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde_inline_default("ambient".to_string())]
    event_name: String,
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde(default)]
    event_names: Vec<AmbientSoundIntervalEventNames>,
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde_inline_default(16.0)]
    range: f64,
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde_inline_default(8.0)]
    value: f64,
}

/// 愤怒音效嵌套结构：愤怒值上升时播放的音效及其触发条件。
///
/// Nested struct for anger level: a sound played when anger increases and
/// its trigger condition.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct AngerLevelSound {
    pub sound: String,
    pub condition: String,
}

/// 实体组件：愤怒等级——追踪实体对各目标的愤怒值，达到阈值后将其转为
/// 敌对目标（如僵尸猪灵的愤怒机制）。
///
/// Entity component: anger_level — tracks the entity's anger toward targets
/// and turns them hostile once the threshold is reached (e.g. zombified
/// piglin anger).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AngerLevel {
    /// 愤怒值随时间递减的间隔（秒）。
    // Interval in seconds at which anger decays over time.
    #[serde_inline_default(1.0)]
    pub anger_decrement_interval: f64,
    /// 单次惹恼事件增加的愤怒值。
    // Anger added by a single annoying event.
    #[serde_inline_default(20)]
    pub angry_boost: u32,
    /// 愤怒阈值：愤怒值达到该值后实体进入愤怒状态。
    // Anger threshold: once reached, the entity becomes angry.
    #[serde_inline_default(80)]
    pub angry_threshold: u32,
    #[serde(default)]
    pub default_annoyingness: i32,
    #[serde(default)]
    pub default_projectile_annoyingness: i32,
    #[serde_inline_default(100)]
    pub max_anger: u32,
    /// 惹恼过滤器：匹配哪些实体会增加本实体的愤怒值。
    // Nuisance filter: matches entities that can increase this entity's anger.
    #[serde(default)]
    pub nuisance_filter: MinecraftFilterType,
    #[serde(default)]
    pub on_increase_sounds: Vec<AngerLevelSound>,
    /// 是否移除愤怒值低于阈值的目标。
    // Whether to drop targets whose anger is below the threshold.
    #[serde_inline_default(true)]
    pub remove_targets_below_angry_threshold: bool,
}

/// 实体组件：愤怒——实体在一段时间内保持愤怒状态，并可向附近实体广播
/// 愤怒（如狼、末影人的敌对表现）。
///
/// Entity component: angry — keeps the entity angry for a duration and can
/// broadcast anger to nearby entities (e.g. hostile wolves, endermen).
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
    /// 愤怒时长的随机浮动量。
    // Random variance applied to the anger duration.
    #[serde(default)]
    pub duration_delta: i32,
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde(default)]
    pub sound_interval: MinecraftRangeType<f64>,
}

/// 实体组件：注解破坏门——困难难度下实体可以破坏木门（如僵尸围攻）。
///
/// Entity component: annotation_break_door — the entity can break wooden
/// doors on hard difficulty (e.g. zombies during sieges).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AnnotationBreakDoor {
    #[serde_inline_default(12.0)]
    pub break_time: f64,
    /// 允许破坏门的最低游戏难度。
    // Minimum game difficulty required to break doors.
    #[serde_inline_default("hard".to_string())]
    pub min_difficulty: String,
}

/// 实体组件：注解开门——实体可以打开木门（如村民通过门）。
///
/// Entity component: annotation_open_door — the entity can open wooden doors
/// (e.g. villagers pathing through doors).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AnnotationOpenDoor {}

/// 实体组件：范围攻击——对周围范围内的实体持续造成伤害。
///
/// Entity component: area_attack — deals damage over time to entities
/// within a radius.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AreaAttack {
    /// 伤害原因文本，用于死亡信息。
    // Damage cause text used in death messages.
    #[serde(default)]
    pub cause: String,
    #[serde(default)]
    pub entity_filter: MinecraftFilterType,
    #[serde_inline_default(2)]
    pub damage_per_tick: i32,
    #[serde_inline_default(0.2)]
    pub damage_range: f64,
}

/// 实体组件：攻击——定义实体的基础近战攻击行为与伤害。
///
/// Entity component: attack — defines the entity's basic melee attack
/// behavior and damage.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Attack {
    pub damage: MinecraftRangeType<f64>,
    #[serde(default)]
    pub effect_duration: f64,
    #[serde(default)]
    pub effect_name: String,
}

/// 实体组件：攻击冷却——控制实体两次攻击之间的冷却时间。
///
/// Entity component: attack_cooldown — controls the cooldown between the
/// entity's attacks.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AttackCooldown {
    /// 攻击冷却结束时触发的事件。
    // Event fired when the attack cooldown completes.
    #[serde(default)]
    pub attack_cooldown_complete_event: MinecraftEvent,
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(0.0, 1.0)))]
    pub attack_cooldown_time: MinecraftRangeType<f64>,
}

/// 实体组件：攻击伤害——定义实体攻击造成的伤害值。
///
/// Entity component: attack_damage — defines the damage dealt by the
/// entity's attacks.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AttackDamage {
    pub value: i32,
}

/// 实体组件：可气球化——实体可被气球吊起并随气球飘浮。
///
/// Entity component: balloonable — the entity can be lifted by a balloon
/// and floats along with it.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Balloonable {
    /// 气球绳的柔性距离：超过该距离实体开始被拉动。
    // Soft distance of the balloon leash before the entity is pulled.
    #[serde_inline_default(2.0)]
    pub soft_distance: f64,
    /// 气球脱落的最大距离：超过该距离气球破裂。
    // Maximum distance before the balloon pops and detaches.
    #[serde_inline_default(2.0)]
    pub max_distance: f64,
    #[serde(default)]
    pub on_balloon: String,
    #[serde(default)]
    pub on_unballoon: String,
    /// 实体质量，影响被气球吊起时的运动表现。
    // Entity mass, affecting motion while carried by a balloon.
    #[serde_inline_default(1.0)]
    pub mass: f64,
}

/// 实体组件：以物易物——实体接受玩家给予的物品后按交易表掉落战利品
/// （如猪灵）。
///
/// Entity component: barter — the entity takes an item from the player and
/// drops loot according to a barter table (e.g. piglins).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Barter {
    pub barter_table: String,
    #[serde(default)]
    pub cooldown_after_being_attacked: i32,
}

/// 实体组件：方块攀爬者——实体可以攀爬方块（如蜘蛛）。
///
/// Entity component: block_climber — the entity can climb blocks (e.g.
/// spiders).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BlockClimber {}

/// 方块感知破坏事件嵌套结构：定义被感知破坏的方块列表及方块被破坏时
/// 触发的事件。
///
/// Nested struct for block sensor: the blocks being watched and the event
/// fired when one of them is broken.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct BlockSensorOnBreak {
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    block_list: Vec<String>,
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    on_block_broken: String,
}

/// 实体组件：方块感知——感知附近特定方块的破坏事件并触发响应（如唤魔者
/// 感知羊被染色）。
///
/// Entity component: block_sensor — senses when nearby blocks of interest
/// are broken and fires events in response (e.g. evokers sensing sheep).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BlockSensor {
    pub on_break: Vec<BlockSensorOnBreak>,
    #[serde_inline_default(16.0)]
    pub sensor_radius: f64,
    /// 感知来源列表：限定哪些方块或标签的破坏会被感知。
    // Source list limiting which block or tag breaks are sensed.
    #[serde(default)]
    pub sources: Vec<String>,
}

/// 实体组件：身体旋转阻止——阻止实体身体随头部朝向转动。
///
/// Entity component: body_rotation_blocked — prevents the entity's body
/// from rotating with its head.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BodyRotationBlocked {}

/// 可加速物品嵌套结构：定义可作为加速剂的物品及其被消耗后替换成的物品。
///
/// Nested struct for boostable: an item usable as a boost and the item it
/// is replaced with after use.
#[serde_inline_default]
#[derive(Clone, Deserialize, Default, Debug)]
pub struct BoostableBoostItem {
    /// 每次加速对该物品耐久造成的损耗。
    // Durability damage dealt to the item per boost.
    #[serde_inline_default(1)]
    pub damage: i32,
    pub item: String,
    pub replace_item: String,
}

/// 实体组件：可加速——实体可通过持有/食用特定物品获得加速（如马吃金萝卜、
/// 马吃甘蔗变快）。
///
/// Entity component: boostable — the entity can be boosted by holding or
/// eating specific items (e.g. horses fed golden carrots).
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

/// 实体组件：Boss——将实体标记为 Boss，显示 Boss 血条并播放 Boss 音乐。
///
/// Entity component: boss — marks the entity as a boss, showing a boss
/// health bar and playing boss music.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Boss {
    #[serde_inline_default(55)]
    pub hud_range: i32,
    pub name: String,
    #[serde(default)]
    pub should_darken_sky: bool,
}

/// 实体组件：破坏方块——实体能够破坏指定的可破坏方块（如蠹虫破坏方块）。
///
/// Entity component: break_blocks — the entity can break the specified
/// breakable blocks.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BreakBlocks {
    #[serde(default)]
    pub breakable_blocks: Vec<String>,
}

/// 实体组件：可呼吸——定义实体的呼吸规则（在空气、水、岩浆、固体方块中
/// 是否能呼吸，以及供氧/窒息时间）。
///
/// Entity component: breathable — defines the entity's breathing rules
/// (whether it breathes in air, water, lava, or solids) and its air
/// supply/suffocation timers.
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
    // Vanilla ships float seconds here (player.json: 3.75); plain ints
    // still deserialize into f64, so this stays compatible both ways.
    #[serde_inline_default(0.0)]
    pub inhale_time: f64,
    #[serde(default)]
    pub non_breathe_blocks: Vec<String>,
    #[serde_inline_default(-20)]
    #[serde(alias = "suffocateTime")]
    pub suffocate_time: i32,
    #[serde_inline_default(15)]
    #[serde(alias = "totalSupply")]
    pub total_supply: i32,
}

/// 繁殖配对嵌套结构：定义可与之繁殖的其他实体类型及其事件。
///
/// Nested struct for breedable: another entity type this entity can breed
/// with and related events.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct BreedableBreedsWith {
    #[serde(default)]
    pub baby_type: String,
    #[serde(default)]
    pub breed_event: String,
    #[serde(default)]
    pub mate_type: String,
}

/// 繁殖父母变体拒绝嵌套结构：控制后代变体与父母变体相同时的拒绝概率。
///
/// Nested struct for breedable: chance to deny a baby inheriting the same
/// variant as its parents.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct BreedableDenyParentsVariant {
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde(default)]
    chance: f64,
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde(default)]
    max_variant: i32,
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde(default)]
    min_variant: i32,
}

/// 繁殖环境需求嵌套结构：繁殖所需的环境条件（指定方块、数量、半径）。
///
/// Nested struct for breedable: environmental requirements for breeding
/// (required blocks, count, radius).
#[derive(Clone, Deserialize, Debug, Default)]
pub struct BreedableEnvironmentRequirements {
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default)]
    pub count: i32,
    #[serde(default)]
    pub radius: f64,
}

/// 繁殖变异因子嵌套结构：控制后代的毛色、变体等属性的变异权重。
///
/// Nested struct for breedable: weights controlling mutation of the baby's
/// color, variant, and related attributes.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct BreedableMutationFactor {
    /// 毛色变异权重。
    // Weight for color mutation.
    #[serde_inline_default(0.0)]
    pub color: f64,
    /// 额外产仔概率的变异权重。
    // Weight for extra baby chance mutation.
    #[serde_inline_default(0.0)]
    pub extra_baby_chance: f64,
    /// 额外变体的变异权重。
    // Weight for extra variant mutation.
    #[serde_inline_default(0.0)]
    pub extra_variant: f64,
    #[serde_inline_default(true)]
    pub inherit_tamed: bool,
    /// 变体变异权重。
    // Weight for variant mutation.
    #[serde_inline_default(0.0)]
    pub variant: f64,
}

/// 实体组件：可繁殖——实体可通过喂食进入求偶状态并交配繁殖后代。
///
/// Entity component: breedable — the entity can enter love mode when fed
/// and breed to produce offspring.
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

/// 实体组件：可贿赂——实体可通过给予特定物品被安抚（如美西螈用桶装
/// 热带鱼贿赂）。
///
/// Entity component: bribeable — the entity can be bribed with specific
/// items (e.g. axolotls bribed with tropical fish buckets).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Bribeable {
    #[serde_inline_default(2.0)]
    pub bribe_cooldown: f64,
    #[serde(default)]
    pub bribe_items: Vec<String>,
}

/// 实体组件：浮力——实体在液体中上浮，并模拟波浪起伏（如船、浮漂）。
///
/// Entity component: buoyant — the entity floats up in liquids and
/// simulates wave bobbing (e.g. boats, buoys).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Buoyant {
    #[serde_inline_default(true)]
    pub apply_gravity: bool,
    /// 基础浮力大小。
    // Base buoyancy magnitude.
    #[serde_inline_default(1.0)]
    pub base_buoyancy: f64,
    #[serde_inline_default(0.03)]
    pub big_wave_probability: f64,
    #[serde_inline_default(10.0)]
    pub big_wave_speed: f64,
    /// 浮力被移除时施加的下沉拉力。
    // Downward drag applied when buoyancy is removed.
    #[serde_inline_default(0.0)]
    pub drag_down_on_buoyancy_removed: f64,
    #[serde(default)]
    pub liquid_blocks: Vec<String>,
    /// 是否模拟水面波浪起伏。
    // Whether to simulate wave bobbing on the surface.
    #[serde_inline_default(true)]
    pub simulate_waves: bool,
}

/// 实体组件：日光燃烧——实体暴露在日光下会燃烧（如僵尸、骷髅）。
///
/// Entity component: burns_in_daylight — the entity burns when exposed to
/// daylight (e.g. zombies, skeletons).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BurnsInDaylight {}

/// 实体组件：可攀爬——实体可以攀爬梯子等可攀爬方块。
///
/// Entity component: can_climb — the entity can climb climbable blocks
/// such as ladders.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanClimb {}

/// 实体组件：可飞行——实体可以飞行。
///
/// Entity component: can_fly — the entity can fly.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanFly {}

/// 实体组件：可参与袭击——实体可以加入村庄袭击事件（如灾厄村民）。
///
/// Entity component: can_join_raid — the entity can join village raids
/// (e.g. illagers).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanJoinRaid {}

/// 实体组件：强力跳跃——实体可以进行蓄力强力跳跃（如马跳跃栏障）。
///
/// Entity component: can_power_jump — the entity can perform a charged
/// power jump (e.g. horses jumping obstacles).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanPowerJump {}

/// 实体组件：庆祝狩猎——实体成功捕猎后播放庆祝行为与音效（如狼群捕到
/// 猎物后摇尾庆祝）。
///
/// Entity component: celebrate_hunt — the entity celebrates after a
/// successful hunt, playing sounds and animations (e.g. wolves wagging
/// tails after a kill).
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

/// 实体组件：颜色——定义实体的颜色值（如羊、狼的毛色索引）。
///
/// Entity component: color — defines the entity's color value (e.g. sheep
/// or wolf color index).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Color {
    #[serde_inline_default(0)]
    pub value: i32,
}

/// 实体组件：颜色 2——定义实体的第二种颜色值（如热带鱼的体色/鳍色）。
///
/// Entity component: color2 — defines the entity's second color value
/// (e.g. tropical fish body/fin color).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Color2 {
    #[serde_inline_default(0)]
    pub value: i32,
}

/// 实体组件：战斗回复——实体在脱离战斗一段时间后自动恢复生命值。
///
/// Entity component: combat_regeneration — the entity regenerates health
/// after being out of combat for a while.
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

/// 条件带宽优化条件值嵌套结构：满足条件时采用的网络同步优化参数。
///
/// Nested struct for conditional bandwidth optimization: sync optimization
/// values applied when the condition matches.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ConditionalBandwidthOptimizationConditionalValues {
    #[serde(default)]
    pub max_dropped_ticks: i32,
    #[serde(default)]
    pub max_optimized_distance: i32,
    #[serde(default)]
    pub use_motion_prediction_hints: bool,
}

/// 条件带宽优化默认值嵌套结构：条件不满足时采用的默认网络同步优化参数。
///
/// Nested struct for conditional bandwidth optimization: default sync
/// optimization values used when no condition matches.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ConditionalBandwidthOptimizationDefaultValues {
    #[serde(default)]
    pub max_dropped_ticks: i32,
    #[serde(default)]
    pub max_optimized_distance: i32,
    #[serde(default)]
    pub use_motion_prediction_hints: bool,
}

/// 实体组件：条件带宽优化——按距离等条件动态调整实体网络同步频率，优化
/// 服务端带宽。
///
/// Entity component: conditional_bandwidth_optimization — dynamically
/// adjusts entity network sync frequency based on distance and other
/// conditions to save server bandwidth.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ConditionalBandwidthOptimization {
    #[serde(default)]
    pub conditional_values: Vec<ConditionalBandwidthOptimizationConditionalValues>,
    #[serde(default)]
    pub default_values: ConditionalBandwidthOptimizationDefaultValues,
}

/// 自定义命中测试碰撞箱嵌套结构：定义自定义命中测试用的单个碰撞箱
/// （高度、宽度、轴心点）。
///
/// Nested struct for custom hit test: a single hit test box defined by its
/// height, width, and pivot.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct CustomHitTestHitboxes {
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub pivot: Vec<f64>,
}

/// 实体组件：自定义命中测试——使用自定义碰撞箱而非默认实体碰撞箱进行
/// 射线命中测试。
///
/// Entity component: custom_hit_test — performs ray hit tests against custom
/// hitboxes instead of the default entity bounds.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CustomHitTest {
    #[serde(default)]
    pub hitboxes: Vec<CustomHitTestHitboxes>,
}

/// 实体组件：持续伤害——对实体按固定时间间隔持续造成伤害（如中毒、
/// 着火效果）。
///
/// Entity component: damage_over_time — deals damage to the entity at a
/// fixed interval over time (e.g. poison, burning).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct DamageOverTime {
    #[serde_inline_default(1)]
    pub damage_per_hurt: i32,
    #[serde_inline_default(0.0)]
    pub time_between_hurt: f64,
}

/// 伤害传感器触发事件嵌套结构：满足过滤条件时触发的事件。
///
/// Nested struct for damage sensor triggers: the event fired when the
/// filter matches.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct DamageSensorTriggersOnDamage {
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde(default)]
    pub event: String,
}

/// 伤害传感器触发器嵌套结构：按伤害来源过滤并修改伤害，同时可触发事件
/// 与音效。
///
/// Nested struct for damage sensor: filters by damage cause, modifies the
/// damage, and can fire events and sounds.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct DamageSensorTriggers {
    #[serde(default)]
    pub cause: String,
    /// 伤害的加性修正值（在倍率计算后叠加）。
    // Additive damage modifier applied after multipliers.
    #[serde(default)]
    pub damage_modifier: f64,
    /// 伤害的乘性倍率。
    // Multiplicative damage multiplier.
    #[serde(default)]
    pub damage_multiplier: f64,
    #[serde(default)]
    pub deals_damage: bool,
    #[serde(default)]
    pub on_damage: DamageSensorTriggersOnDamage,
    #[serde(default)]
    pub on_damage_sound_event: String,
}

/// 实体组件：伤害传感器——按伤害来源拦截并修改实体受到的伤害。
///
/// Entity component: damage_sensor — intercepts and modifies damage dealt
/// to the entity based on its cause.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct DamageSensor {
    #[serde(default)]
    pub triggers: Vec<DamageSensorTriggers>,
}

/// 实体组件：冲刺——实体向视线方向冲刺（如山羊、美西螈的闪避冲刺）。
///
/// Entity component: dash — the entity dashes in its look direction (e.g.
/// goats, axolotl death rolls).
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

/// 实体组件：默认朝向角——定义实体生成时的初始水平朝向角。
///
/// Entity component: default_look_angle — sets the entity's initial
/// horizontal look angle on spawn.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct DefaultLookAngle {
    #[serde_inline_default(0.0)]
    pub value: f64,
}

/// 按距离消失嵌套结构：实体距玩家过近或过远时消失。
///
/// Nested struct for despawn: the entity despawns when too close to or too
/// far from players.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct DespawnFromDistance {
    #[serde_inline_default(128)]
    pub max_distance: i32,
    #[serde_inline_default(32)]
    pub min_distance: i32,
}

/// 实体组件：消失——定义实体消失的条件（距离、概率、不活跃、模拟边缘）。
///
/// Entity component: despawn — defines when the entity despawns (distance,
/// chance, inactivity, simulation edge).
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
    /// 不活跃多久后允许因不活跃而消失（秒）。
    // Seconds of inactivity before inactivity despawn is allowed.
    pub min_range_inactivity_timer: i32,
    #[serde_inline_default(800)]
    /// 随机消失的最小判定范围。
    // Minimum range at which random-chance despawn may occur.
    pub min_range_random_chance: i32,
    #[serde_inline_default(false)]
    pub remove_child_entities: bool,
}

/// 实体组件：干涸计时器——实体离水后逐渐干涸并可恢复（如美西螈）。
///
/// Entity component: drying_out_timer — the entity dries out when out of
/// water and can recover (e.g. axolotls).
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
    /// 用水瓶为其补水的耗时（秒）。
    // Seconds required to refill with a water bottle.
    #[serde_inline_default(0.0)]
    pub water_bottle_refill_time: f64,
}

/// 实体组件：聚居者——定义参与村庄/聚居点行为的实体（如村民的职业、
/// 迁徙、寻找兴趣点等）。
///
/// Entity component: dweller — defines an entity that participates in
/// village/dwelling behavior (profession, migration, POI finding).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Dweller {
    #[serde(default)]
    pub dwelling_type: String,
    #[serde(default)]
    pub dweller_role: String,
    /// 行为更新间隔的基数（秒）。
    // Base behavior update interval in seconds.
    #[serde_inline_default(0.0)]
    pub update_interval_base: f64,
    /// 行为更新间隔的个体浮动量（秒）。
    // Per-entity variance applied to the update interval.
    #[serde_inline_default(0.0)]
    pub update_interval_variant: f64,
    #[serde(default)]
    pub can_find_poi: bool,
    /// 首次建立聚居点时获得的奖励。
    // Reward given for founding a dwelling for the first time.
    #[serde_inline_default(0)]
    pub first_founding_reward: i32,
    #[serde(default)]
    pub can_migrate: bool,
    /// 居所边界判定的容差。
    // Tolerance when checking dwelling bounds.
    #[serde_inline_default(0.0)]
    pub dwelling_bounds_tolerance: f64,
    #[serde(default)]
    pub preferred_profession: String,
}

/// 实体组件：经济交易表——定义实体的交易表与经济折扣规则（如村民治愈、
/// 英雄折扣）。
///
/// Entity component: economy_trade_table — defines the entity's trade table
/// and economy discount rules (e.g. cured villager and hero discounts).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct EconomyTradeTable {
    #[serde(default)]
    pub convert_trades_economy: bool,
    /// 治愈村民带来的价格折扣范围。
    // Price discount range from curing villagers.
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(-5.0, -20.0)))]
    pub cured_discount: MinecraftRangeType<f64>,
    #[serde(default)]
    pub display_name: String,
    /// 英雄之誉村民的额外需求折扣。
    // Extra demand discount for villagers with Hero of the Village.
    #[serde_inline_default(-4)]
    pub hero_demand_discount: i32,
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(-25.0, -63.0)))]
    pub max_cured_discount: MinecraftRangeType<f64>,
    #[serde_inline_default(-200)]
    pub max_nearby_cured_discount: i32,
    /// 附近存在治愈村民时的价格折扣。
    // Price discount when cured villagers are nearby.
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
    /// 是否使用旧版价格公式。
    // Whether to use the legacy price formula.
    #[serde(default)]
    pub use_legacy_price_formula: bool,
}

/// 实体组件：实体传感器——感知附近符合条件的实体并在数量达到范围时触发
/// 事件。
///
/// Entity component: entity_sensor — senses nearby entities matching
/// filters and fires an event when their count falls within a range.
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
    /// 是否要求所有子传感器的条件都满足。
    // Whether all subsensor conditions must be met.
    #[serde(default)]
    pub require_all: bool,
    /// 子传感器列表：可嵌套组合更复杂的感知条件。
    // Subsensor list: allows nesting more complex sensing conditions.
    #[serde(default)]
    pub subsensors: Vec<Self>,
}

/// 环境传感器触发器嵌套结构：过滤器满足时对指定目标触发事件。
///
/// Nested struct for environment sensor: fires an event at a target when
/// the filter matches.
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

/// 实体组件：环境传感器——按环境条件（如天气、时间、附近方块）触发事件。
///
/// Entity component: environment_sensor — fires events based on
/// environmental conditions (e.g. weather, time, nearby blocks).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct EnvironmentSensor {
    // Vanilla ships both shapes: a single trigger map (llama/pillager)
    // and a trigger sequence. Accept both; gameplay reads the vec.
    #[serde(default, deserialize_with = "single_or_vec_trigger")]
    pub triggers: Vec<EnvironmentSensorTrigger>,
}

/// `Vec<T>` that also accepts a single bare value (vanilla JSON duality).
fn single_or_vec_trigger<'de, D>(deserializer: D) -> Result<Vec<EnvironmentSensorTrigger>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(EnvironmentSensorTrigger),
        Many(Vec<EnvironmentSensorTrigger>),
    }
    match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(trigger) => Ok(vec![trigger]),
        OneOrMany::Many(triggers) => Ok(triggers),
    }
}

/// 实体组件：装备物品——实体生成时按定义装备物品。
///
/// Entity component: equip_item — the entity is equipped with items on
/// spawn.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct EquipItem {}

/// 装备槽掉落率嵌套结构：定义单个装备槽的物品掉落概率。
///
/// Nested struct for equipment: drop chance for a single equipment slot.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct EquipmentSlotDropChance {
    #[serde(default)]
    pub slot: String,
    #[serde_inline_default(0.0)]
    pub chance: f64,
}

/// 实体组件：装备——定义实体的装备掉落规则（装备表与各槽位掉落率）。
///
/// Entity component: equipment — defines the entity's equipment drop rules
/// (equipment table and per-slot drop chances).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Equipment {
    #[serde(default)]
    pub slot_drop_chance: Vec<EquipmentSlotDropChance>,
    #[serde(default)]
    pub table: String,
}

/// 可装备槽位嵌套结构：定义单个可装备槽位接受的物品及装备/卸下事件。
///
/// Nested struct for equippable: a single equippable slot with its
/// accepted items and equip/unequip events.
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

/// 实体组件：可装备——定义实体可被玩家交互装备物品的槽位（如给马套鞍）。
///
/// Entity component: equippable — defines slots where players can interact
/// to equip items on the entity (e.g. saddling a horse).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Equippable {
    #[serde(default)]
    pub slots: Vec<EquippableSlot>,
}

/// 实体组件：饥饿度消耗值——定义各行动（攻击、跳跃、挖掘、奔跑等）对
/// 饥饿度的消耗量。
///
/// Entity component: exhaustion_values — defines how much hunger each
/// action (attack, jump, mining, sprinting, etc.) costs.
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

/// 实体组件：经验奖励——定义实体死亡或被繁殖时掉落的经验值。
///
/// Entity component: experience_reward — defines the experience the entity
/// drops on death or when bred.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ExperienceReward {
    #[serde(default)]
    pub on_bred: MolangExpression,
    #[serde(default)]
    pub on_death: MolangExpression,
}

/// 实体组件：爆炸——实体可爆炸（如苦力怕、TNT 点燃后的行为）。
///
/// Entity component: explode — the entity can explode (e.g. creepers,
/// ignited TNT).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Explode {
    #[serde_inline_default(true)]
    pub breaks_blocks: bool,
    #[serde_inline_default(false)]
    pub causes_fire: bool,
    /// 破坏方块是否受游戏规则 doMobGriefing 影响。
    // Whether block destruction respects the mobGriefing game rule.
    #[serde_inline_default(false)]
    pub destroy_affected_by_griefing: bool,
    /// 引发火焰是否受游戏规则 doMobGriefing 影响。
    // Whether fire ignition respects the mobGriefing game rule.
    #[serde_inline_default(false)]
    pub fire_affected_by_griefing: bool,
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(0.0, 0.0)))]
    pub fuse_length: MinecraftRangeType<f64>,
    #[serde_inline_default(false)]
    pub fuse_lit: bool,
    /// 可被爆炸破坏的方块的最大爆炸抗性。
    // Maximum blast resistance of blocks this explosion can destroy.
    #[serde_inline_default(3.40282e+38)]
    pub max_resistance: f64,
    #[serde_inline_default(3.0)]
    pub power: f64,
}

/// 实体组件：火焰免疫——实体免疫火焰与岩浆伤害。
///
/// Entity component: fire_immune — the entity is immune to fire and lava
/// damage.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct FireImmune {}

/// 实体组件：液体中漂浮——实体在液体中上浮至液面。
///
/// Entity component: floats_in_liquid — the entity floats up to the surface
/// when in a liquid.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct FloatsInLiquid {}

/// 实体组件：群集——实体以 Boids 算法成群移动（如鱼群、蝙蝠群）。
///
/// Entity component: flocking — the entity moves in groups using boids
/// behavior (e.g. fish schools, bat flocks).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Flocking {
    #[serde_inline_default(0.0)]
    pub block_distance: f64,
    #[serde_inline_default(0.0)]
    pub block_weight: f64,
    /// 越过群体边界时受到的转向影响权重。
    // Steering influence weight when breaching flock bounds.
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
    /// 个体脱离群体的概率。
    // Chance for an individual to break away from the flock.
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

/// 实体组件：飞行速度——定义实体的飞行速度。
///
/// Entity component: flying_speed — defines the entity's flying speed.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct FlyingSpeed {
    #[serde_inline_default(0.0)]
    pub value: f64,
}

/// 实体组件：追踪范围——定义实体能够追踪/锁定目标的距离。
///
/// Entity component: follow_range — defines the distance at which the
/// entity can track or lock onto targets.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct FollowRange {
    #[serde_inline_default(0.0)]
    pub value: f64,
    #[serde_inline_default(0.0)]
    pub max: f64,
}

/// 实体组件：摩擦修饰符——修改实体与地面接触时的摩擦系数。
///
/// Entity component: friction_modifier — modifies the entity's friction
/// against the ground.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct FrictionModifier {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

/// 实体组件：游戏事件移动追踪——控制实体移动时发出哪些游戏事件振动
/// （如挥翅、移动、游泳）。
///
/// Entity component: game_event_movement_tracking — controls which game
/// event vibrations the entity emits while moving (flap, move, swim).
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

/// 实体组件：遗传——定义实体的基因遗传与突变规则（如悦灵的遗传繁殖）。
///
/// Entity component: genetics — defines the entity's gene inheritance and
/// mutation rules.
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

/// 实体组件：可给予——玩家可以向实体赠予物品（如村民接受礼物）。
///
/// Entity component: giveable — players can give items to the entity (e.g.
/// villagers accepting gifts).
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

/// 实体组件：地面偏移——定义实体模型相对于地面的偏移量。
///
/// Entity component: ground_offset — defines the entity model's offset
/// above the ground.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct GroundOffset {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

/// 实体组件：群体规模——感知附近同类实体的数量并约束群体大小。
///
/// Entity component: group_size — senses the number of nearby same-type
/// entities and constrains group size.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct GroupSize {
    #[serde(default)]
    pub filters: MinecraftFilterType,
    #[serde_inline_default(16.0)]
    pub radius: f64,
}

/// 实体组件：催熟作物——实体可被用于催熟附近作物（如蜜蜂加速作物生长）。
///
/// Entity component: grows_crop — the entity can be used to accelerate
/// nearby crop growth (e.g. bees pollinating crops).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct GrowsCrop {
    #[serde_inline_default(0.0)]
    pub chance: f64,
    #[serde_inline_default(10)]
    pub charges: i32,
}

/// 可治疗物品嵌套结构：定义可治疗该实体的物品及其治疗量。
///
/// Nested struct for healable: an item that heals the entity and the
/// amount it heals.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct HealableItem {
    #[serde_inline_default(1)]
    pub heal_amount: i32,
    #[serde(default)]
    pub item: String,
}

/// 实体组件：可治疗——实体可被特定物品治疗（如用治疗药水治疗马）。
///
/// Entity component: healable — the entity can be healed with specific
/// items.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Healable {
    #[serde(default)]
    pub filters: MinecraftFilterType,
    /// 即使实体满血也强制消耗治疗物品。
    // Force consuming the healing item even at full health.
    #[serde(default)]
    pub force_use: bool,
    #[serde(default)]
    pub items: Vec<HealableItem>,
}

/// 实体组件：生命值——定义实体的最大生命值与当前生命值。
///
/// Entity component: health — defines the entity's maximum and current
/// health.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Health {
    #[serde_inline_default(0.0)]
    pub max: f64,
    #[serde_inline_default(0.0)]
    pub value: f64,
}

/// 实体组件：心跳——按间隔播放心跳音效（如监守者/幽匿感测体的心跳）。
///
/// Entity component: heartbeat — plays a heartbeat sound at an interval
/// (e.g. the warden's heartbeat).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Heartbeat {
    #[serde(default)]
    pub interval: MolangExpression,
    #[serde_inline_default("heartbeat".to_string())]
    pub sound_event: String,
}

/// 实体组件：隐藏——隐藏实体的血条与名称显示。
///
/// Entity component: hide — hides the entity's health bar and name display.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Hide {}

/// 实体组件：家——定义实体的家（限定活动范围的方块列表与半径）。
///
/// Entity component: home — defines the entity's home (blocks that count as
/// home and the restriction radius).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Home {
    #[serde(default)]
    pub home_block_list: Vec<String>,
    #[serde_inline_default(-1)]
    pub restriction_radius: i32,
}

/// 实体组件：马跳跃力量——定义马的跳跃力量范围。
///
/// Entity component: horse_jump_strength — defines the range of the horse's
/// jump strength.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct HorseJumpStrength {
    #[serde(default)]
    pub value: MinecraftRangeType<f64>,
}

/// 条件伤害伤害条件嵌套结构：按过滤条件与伤害来源定义持续伤害。
///
/// Nested struct for hurt on condition: damage applied while the filter
/// and damage cause match.
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

/// 实体组件：条件受伤——满足条件时实体受到持续伤害（如海龟在泡沫中
/// 受伤、绵羊吃仙人掌受伤）。
///
/// Entity component: hurt_on_condition — the entity takes damage over time
/// while a condition holds (e.g. turtles hurt by pufferfish).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct HurtOnCondition {
    #[serde(default)]
    pub damage_conditions: Vec<HurtOnConditionDamageCondition>,
}

/// 实体组件：地面输入控制——实体在地面上的移动由输入控制（如玩家坐骑）。
///
/// Entity component: input_ground_controlled — the entity's ground movement
/// is controlled by input (e.g. player-ridden mounts).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct InputGroundControlled {}

/// 方块内通知方块列表嵌套结构：定义被监听的方块及其进入/离开事件。
///
/// Nested struct for inside block notifier: a watched block and the events
/// fired when the entity enters or exits it.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct InsideBlockNotifierBlockList {
    pub block: MinecraftBlockIdent,
    #[serde(default)]
    pub entered_block_event: MinecraftEvent,
    #[serde(default)]
    pub exited_block_event: MinecraftEvent,
}

/// 实体组件：方块内通知——实体进入或离开指定方块时触发事件（如进入
/// 蜘蛛网、岩浆）。
///
/// Entity component: inside_block_notifier — fires events when the entity
/// enters or exits specified blocks (e.g. cobwebs, lava).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct InsideBlockNotifier {
    #[serde(default)]
    pub block_list: Vec<InsideBlockNotifierBlockList>,
}

/// 实体组件：失眠——实体连续多天不睡觉会触发失眠机制（如幻翼生成）。
///
/// Entity component: insomnia — the entity triggers insomnia mechanics after
/// too many days without sleep (e.g. phantom spawning).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Insomnia {
    #[serde_inline_default(3.0)]
    pub days_until_insomnia: f64,
}

/// 实体组件：立即消失——实体被移除时立即消失，不播放下落动画。
///
/// Entity component: instant_despawn — the entity disappears immediately
/// when removed, without playing the despawn animation.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct InstantDespawn {
    #[serde_inline_default(false)]
    pub remove_child_entities: bool,
}

/// 交互添加物品嵌套结构：定义交互时从战利品表添加给交互者的物品。
///
/// Nested struct for interact: items added to the interactor from a loot
/// table on interaction.
#[derive(Clone, Deserialize, Debug, Default)]
pub struct InteractAddItems {
    #[serde(default)]
    pub table: String,
}

/// 交互粒子嵌套结构：定义交互开始时播放的粒子效果及其偏移。
///
/// Nested struct for interact: the particle effect played on interaction
/// and its offsets.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct InteractParticleOnStart {
    /// 粒子位置是否向交互者方向偏移。
    // Whether the particle position is offset toward the interactor.
    #[serde_inline_default(false)]
    pub particle_offset_towards_interactor: bool,
    #[serde(default)]
    pub particle_type: String,
    #[serde_inline_default(0.0)]
    pub particle_y_offset: f64,
}

/// 交互生成物品嵌套结构：定义交互时从战利品表生成掉落的物品。
///
/// Nested struct for interact: items dropped from a loot table on
/// interaction.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct InteractSpawnItems {
    #[serde(default)]
    pub table: String,
}

/// 交互修复实体物品嵌套结构：定义交互时用于修复实体的物品与修复量。
///
/// Nested struct for interact: the item used to repair the entity on
/// interaction and the repair amount.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct InteractRepairEntityItem {
    #[serde(default)]
    pub item_slot: String,
    #[serde_inline_default(0.0)]
    pub repair_amount: f64,
}

/// 交互列表项嵌套结构：定义单个交互行为的完整效果（治疗、物品交换、
/// 事件、粒子等）。
///
/// Nested struct for interact: a single interaction entry with its full
/// effects (healing, item exchange, events, particles).
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
    /// 交互对所使用物品造成的耐久损耗。
    // Durability damage dealt to the used item per interaction.
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
    pub repair_entity_item: InteractRepairEntityItem,
    #[serde(default)]
    pub spawn_entities: String,
    #[serde(default)]
    pub spawn_items: InteractSpawnItems,
    /// 交互时是否播放手臂挥动动画。
    // Whether to play the arm swing animation on interaction.
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

/// 实体组件：交互——定义玩家与实体交互时可触发的多种行为（如喂食、
/// 骑乘、交易）。
///
/// Entity component: interact — defines the interactions players can have
/// with the entity (feeding, riding, trading).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Interact {
    #[serde(default)]
    pub interactions: Vec<InteractList>,
}

/// 实体组件：物品栏——为实体提供可存储物品的物品栏（如驴、羊驼的背包）。
///
/// Entity component: inventory — gives the entity an item inventory (e.g.
/// donkey or llama storage).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Inventory {
    /// 每点力量属性提供的额外槽位数。
    // Extra slots granted per point of strength attribute.
    #[serde_inline_default(0)]
    pub additional_slots_per_strength: i32,
    /// 物品栏内容是否可被漏斗等装置抽出。
    // Whether inventory contents can be siphoned by hoppers etc.
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

/// 实体组件：幼年——标记实体为幼年状态。
///
/// Entity component: is_baby — marks the entity as a baby.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsBaby {}

/// 实体组件：已充能——标记实体为充能状态（如闪电苦力怕）。
///
/// Entity component: is_charged — marks the entity as charged (e.g.
/// charged creepers).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsCharged {}

/// 实体组件：携带箱子——标记实体携带箱子（如驮箱子状态的行军羊驼）。
///
/// Entity component: is_chested — marks the entity as carrying a chest
/// (e.g. trader llamas with chests).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsChested {}

/// 实体组件：可染色——实体可以被染料染色（如羊、狼的项圈）。
///
/// Entity component: is_dyeable — the entity can be dyed (e.g. sheep, wolf
/// collars).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsDyeable {
    #[serde(default)]
    pub interact_text: String,
}

/// 实体组件：隐身时隐藏——实体处于隐身状态时连带其装备一并隐藏。
///
/// Entity component: is_hidden_when_invisible — hides the entity together
/// with its equipment while invisible.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsHiddenWhenInvisible {}

/// 实体组件：已点燃——标记实体处于点燃状态（如被火焰附加的苦力怕）。
///
/// Entity component: is_ignited — marks the entity as ignited (e.g. a
/// flint-and-steel lit creeper).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsIgnited {}

/// 实体组件：灾厄队长——标记实体为袭击队长（带不祥旗帜的灾厄村民）。
///
/// Entity component: is_illager_captain — marks the entity as a raid
/// captain (illagers with ominous banners).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsIllagerCaptain {}

/// 实体组件：已上鞍——标记实体已被装上鞍（如马、猪、羊驼）。
///
/// Entity component: is_saddled — marks the entity as saddled (e.g.
/// horses, pigs, llamas).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsSaddled {}

/// 实体组件：颤抖中——标记实体正在颤抖（如被狼围攻的羊、受惊的动物）。
///
/// Entity component: is_shaking — marks the entity as shaking (e.g. sheep
/// surrounded by wolves).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsShaking {}

/// 实体组件：已剪毛——标记实体已被剪毛（如剪过毛的绵羊）。
///
/// Entity component: is_sheared — marks the entity as sheared (e.g.
/// sheared sheep).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsSheared {}

/// 实体组件：可堆叠——标记实体可以被堆叠（如小船堆叠、矿车堆叠）。
///
/// Entity component: is_stackable — marks the entity as stackable (e.g.
/// stacked boats, minecarts).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsStackable {}

/// 实体组件：已眩晕——标记实体处于眩晕状态（如被拴球炫目）。
///
/// Entity component: is_stunned — marks the entity as stunned (e.g.
/// dazed by a wind charge).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsStunned {}

/// 实体组件：已驯服——标记实体为已驯服状态（如被驯服的狼、猫）。
///
/// Entity component: is_tamed — marks the entity as tamed (e.g. tamed
/// wolves, cats).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IsTamed {}

/// 实体组件：物品可操控——持有指定物品的实体可被玩家操控移动（如用
/// 胡萝卜钓竿控制猪）。
///
/// Entity component: item_controllable — the entity can be controlled by a
/// player holding specific items (e.g. pigs on a carrot on a stick).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ItemControllable {
    #[serde(default)]
    pub control_items: Vec<String>,
}

/// 实体组件：物品漏斗——实体可作为漏斗吸取上方掉落物（如漏斗矿车行为）。
///
/// Entity component: item_hopper — the entity acts as a hopper picking up
/// items above it (e.g. hopper minecart behavior).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ItemHopper {}

/// 实体组件：动态跳跃——跳跃参数由移动系统动态决定。
///
/// Entity component: jump_dynamic — jump parameters are determined
/// dynamically by the movement system.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct JumpDynamic {}

/// 实体组件：静态跳跃——以固定力量原地起跳（如史莱姆、岩浆怪）。
///
/// Entity component: jump_static — jumps in place with a fixed power (e.g.
/// slimes, magma cubes).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct JumpStatic {
    #[serde_inline_default(0.42)]
    pub jump_power: f64,
}

/// 实体组件：击退抗性——定义实体抵抗击退的程度。
///
/// Entity component: knockback_resistance — defines how much the entity
/// resists knockback.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct KnockbackResistance {
    #[serde(default)]
    pub value: f64,
    #[serde(default)]
    pub max: f64,
}

/// 实体组件：岩浆移动——定义实体在岩浆中移动的速度。
///
/// Entity component: lava_movement — defines the entity's movement speed
/// in lava.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct LavaMovement {
    #[serde(default)]
    pub value: f64,
}

/// 实体组件：可拴住——实体可以被拴绳牵引（如用拴绳牵走动物）。
///
/// Entity component: leashable — the entity can be held on a leash (e.g.
/// leading animals with a lead).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Leashable {
    #[serde_inline_default(false)]
    pub can_be_stolen: bool,
    /// 绳索完全绷直的距离。
    // Distance at which the leash becomes fully taut.
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

/// 实体组件：注视——实体注视附近的玩家或其他实体（如哞菇注视玩家）。
///
/// Entity component: look_at — the entity looks at nearby players or
/// entities (e.g. mooshrooms staring at players).
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
    /// 注视目标后是否将其设为实体的攻击/跟随目标。
    // Whether the looked-at target becomes the entity's attack/follow target.
    #[serde_inline_default(true)]
    pub set_target: bool,
}

/// 实体组件：战利品——定义实体死亡时掉落的战利品表。
///
/// Entity component: loot — defines the loot table the entity drops on
/// death.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Loot {
    #[serde(default)]
    pub table: String,
}

/// 实体组件：流浪商人管理——将实体标记为受管理的流浪商人。
///
/// Entity component: managed_wandering_trader — marks the entity as a
/// managed wandering trader.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ManagedWanderingTrader {}

/// 实体组件：标记变体——定义实体的标记性变体值（如不同品种）。
///
/// Entity component: mark_variant — defines the entity's marker variant
/// value (e.g. different breeds).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MarkVariant {
    #[serde_inline_default(0)]
    pub value: i32,
}

/// 实体组件：生物效果——实体攻击时可对目标施加状态效果（如深渊行者
/// 附加效果）。
///
/// Entity component: mob_effect — applies a status effect to targets the
/// entity attacks.
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

/// 实体组件：两栖移动——实体可在水中与陆地间移动（如海龟、青蛙）。
///
/// Entity component: movement_amphibious — the entity moves in both water
/// and on land (e.g. turtles, frogs).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementAmphibious {
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

/// 实体组件：基础移动——实体的基础陆地移动参数。
///
/// Entity component: movement_basic — the entity's basic ground movement
/// parameters.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementBasic {
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

/// 实体组件：海豚移动——海豚式的游泳移动。
///
/// Entity component: movement_dolphin — dolphin-style swimming movement.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementDolphin {}

/// 实体组件：飞行移动——实体的空中飞行移动参数（如末影龙、幻翼）。
///
/// Entity component: movement_fly — the entity's airborne flight movement
/// parameters (e.g. ender dragons, phantoms).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementFly {
    #[serde_inline_default(0.1)]
    pub start_speed: f64,
    #[serde_inline_default(0.2)]
    pub speed_when_turning: f64,
}

/// 实体组件：通用移动——实体的通用飞行移动参数（如末影人、蜜蜂）。
///
/// Entity component: movement_generic — the entity's generic flight
/// movement parameters (e.g. endermen, bees).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementGeneric {
    #[serde_inline_default(0.1)]
    pub start_speed: f64,
    #[serde_inline_default(0.2)]
    pub speed_when_turning: f64,
}

/// 实体组件：滑翔移动——实体的滑翔飞行移动参数（如劫掠兽、末影龙）。
///
/// Entity component: movement_glide — the entity's gliding flight movement
/// parameters (e.g. ravagers, ender dragons).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementGlide {
    #[serde_inline_default(0.1)]
    pub start_speed: f64,
    #[serde_inline_default(0.2)]
    pub speed_when_turning: f64,
}

/// 实体组件：悬停移动——实体原地悬停的移动参数（如悦灵、蜂巢附近的
/// 蜜蜂）。
///
/// Entity component: movement_hover — the entity's hovering movement
/// parameters (e.g. allays).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementHover {
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

/// 实体组件：跳跃移动——实体以跳跃方式移动（如兔子、史莱姆）。
///
/// Entity component: movement_jump — the entity moves by jumping (e.g.
/// rabbits, slimes).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementJump {
    #[serde_inline_default(MinecraftRangeType::Range(MinecraftRange::new(0.0, 0.0)))]
    pub jump_delay: MinecraftRangeType<f64>,
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

/// 实体组件：跳过移动——实体移动时跳过高度变化（如蠹虫钻入方块）。
///
/// Entity component: movement_skip — the entity skips height changes while
/// moving.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementSkip {
    #[serde_inline_default(30.0)]
    pub max_turn: f64,
}

/// 实体组件：移动音效距离偏移——调整实体移动音效被听到的距离。
///
/// Entity component: movement_sound_distance_offset — adjusts the distance
/// at which the entity's movement sound is heard.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MovementSoundDistanceOffset {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

/// 实体组件：摇摆移动——实体移动时左右摇摆（如鸡、潜影贝的摇摆）。
///
/// Entity component: movement_sway — the entity sways side to side while
/// moving.
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

/// 命名动作嵌套结构：按名称过滤器在实体被命名时触发事件。
///
/// Nested struct for nameable: fires an event when the entity is named,
/// filtered by the given name.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct NameableNameActions {
    #[serde(default)]
    pub name_filter: String,
    #[serde(default)]
    pub on_named: String,
}

/// 实体组件：可命名——定义实体的命名规则（命名牌重命名、名称显示等）。
///
/// Entity component: nameable — defines the entity's naming rules (name
/// tag renaming, name display).
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

/// 实体组件：攀爬寻路——实体以攀爬方式寻路（如蜘蛛）。
///
/// Entity component: navigation_climb — the entity paths by climbing (e.g.
/// spiders).
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

/// 实体组件：漂浮寻路——实体以漂浮方式寻路（如恶魂、烈焰人）。
///
/// Entity component: navigation_float — the entity paths by floating (e.g.
/// ghasts, blazes).
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

/// 实体组件：飞行寻路——实体以飞行方式寻路（如幻翼、末影龙）。
///
/// Entity component: navigation_fly — the entity paths by flying (e.g.
/// phantoms, ender dragons).
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

/// 实体组件：通用寻路——实体的通用寻路参数。
///
/// Entity component: navigation_generic — the entity's generic pathfinding
/// parameters.
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

/// 实体组件：悬停寻路——实体以悬停方式寻路（如悦灵）。
///
/// Entity component: navigation_hover — the entity paths by hovering (e.g.
/// allays).
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

/// 实体组件：游泳寻路——实体以游泳方式寻路（如鱼、鱿鱼）。
///
/// Entity component: navigation_swim — the entity paths by swimming (e.g.
/// fish, squids).
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

/// 实体组件：行走寻路——实体以行走方式寻路（绝大多数陆地生物）。
///
/// Entity component: navigation_walk — the entity paths by walking (most
/// land mobs).
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

/// 实体组件：失控——标记实体处于失控状态（如被移除拴绳后的船）。
///
/// Entity component: out_of_control — marks the entity as out of control
/// (e.g. a boat after its leash is removed).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct OutOfControl {}

/// 实体组件：窥视——定义潜影贝开合时的行为事件。
///
/// Entity component: peek — defines the events fired when a shulker opens
/// and closes.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Peek {
    #[serde(default)]
    pub on_close: MinecraftEvent,
    #[serde(default)]
    pub on_open: MinecraftEvent,
    #[serde(default)]
    pub on_target_open: MinecraftEvent,
}

/// 实体组件：持久化——实体在区块卸载后仍然保留，不会因距离消失。
///
/// Entity component: persistent — the entity persists after chunk unload
/// and never despawns due to distance.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Persistent {}

/// 实体组件：物理——定义实体的碰撞与重力等物理属性。
///
/// Entity component: physics — defines the entity's physical properties
/// such as collision and gravity.
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

/// 实体组件：玩家饥饿度——定义玩家实体的饥饿度上限与当前值。
///
/// Entity component: player_exhaustion — defines the player entity's
/// maximum and current exhaustion (hunger drain).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PlayerExhaustion {
    #[serde(default)]
    pub max: i32,
    #[serde(default)]
    pub value: i32,
}

/// 实体组件：玩家经验——定义玩家实体的经验值。
///
/// Entity component: player_experience — defines the player entity's
/// experience value.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PlayerExperience {
    #[serde_inline_default(5)]
    pub max: i32,
    #[serde_inline_default(1)]
    pub value: i32,
}

/// 实体组件：玩家等级——定义玩家实体的经验等级。
///
/// Entity component: player_level — defines the player entity's
/// experience level.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PlayerLevel {
    #[serde(default)]
    pub max: i32,
    #[serde(default)]
    pub value: i32,
}

/// 实体组件：玩家饱和度——定义玩家实体的食物饱和度。
///
/// Entity component: player_saturation — defines the player entity's food
/// saturation.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PlayerSaturation {
    #[serde(default)]
    pub max: i32,
    #[serde(default)]
    pub value: i32,
}

/// 实体组件：优先路径——定义实体寻路时优先使用的方块及其成本。
///
/// Entity component: preferred_path — defines the blocks the entity
/// prefers when pathing and their costs.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PreferredPath {
    /// 其他方块的默认通行成本。
    // Default pathing cost for other blocks.
    #[serde(default)]
    pub default_block_cost: f64,
    /// 跳跃经过方格的额外通行成本。
    // Extra pathing cost for jumping over a block.
    #[serde(default)]
    pub jump_cost: i32,
    #[serde_inline_default(3)]
    pub max_fall_blocks: i32,
    #[serde(default)]
    pub preferred_path_blocks: Vec<String>,
}

/// 抛射物命中定义事件嵌套结构：命中时对抛射物自身、发射者、命中目标
/// 或溅射区域分别触发事件。
///
/// Nested struct for projectile on hit: fires events affecting the
/// projectile, the shooter, the hit target, or the splash area.
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
    pub event_trigger: MinecraftTrigger,
}

/// 抛射物命中伤害嵌套结构：命中时造成的伤害与附加效果。
///
/// Nested struct for projectile on hit: damage and side effects applied
/// on hit.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ProjectileOnHitImpactDamage {
    #[serde(default)]
    pub catch_fire: bool,
    /// 是否触发引雷（雷暴天气下的三叉戟引雷）。
    // Whether to trigger channeling (trident lightning in thunderstorms).
    #[serde_inline_default(true)]
    pub channeling: bool,
    #[serde_inline_default(1.0)]
    pub damage: f64,
    #[serde(default)]
    pub destroy_on_hit: bool,
    /// 仅在造成伤害时才销毁抛射物。
    // Only destroy the projectile if it dealt damage.
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
    /// 力量附魔等带来的威力倍率。
    // Power multiplier from enchantments such as Power.
    #[serde_inline_default(2.0)]
    pub power_multiplier: f64,
    /// 是否按难度使用半随机伤害。
    // Whether to use semi-random damage scaling by difficulty.
    #[serde(default)]
    pub semi_random_diff_damage: bool,
    #[serde(default)]
    pub set_last_hurt_requires_damage: bool,
}

/// 抛射物命中效果嵌套结构：命中目标时施加的状态效果及其时长。
///
/// Nested struct for projectile on hit: the status effect applied to the
/// hit target and its duration.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ProjectileOnHitMobEffect {
    #[serde_inline_default(1)]
    pub amplifier: i32,
    #[serde_inline_default(false)]
    pub ambient: bool,
    #[serde_inline_default(1)]
    pub duration: i32,
    /// 简单难度下的效果时长。
    // Effect duration on easy difficulty.
    #[serde(default)]
    pub duration_easy: i32,
    /// 困难难度下的效果时长。
    // Effect duration on hard difficulty.
    #[serde_inline_default(800)]
    pub duration_hard: i32,
    /// 普通难度下的效果时长。
    // Effect duration on normal difficulty.
    #[serde_inline_default(200)]
    pub duration_normal: i32,
    #[serde(default)]
    pub effect: String,
    #[serde(default)]
    pub visible: bool,
}

/// 抛射物命中粒子嵌套结构：命中时播放的粒子效果。
///
/// Nested struct for projectile on hit: particle effects played on hit.
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

/// 抛射物命中生成药水云嵌套结构：命中时生成的效果云（如滞留药水）。
///
/// Nested struct for projectile on hit: spawns an area effect cloud on hit
/// (e.g. lingering potions).
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

/// 抛射物命中生成概率嵌套结构：命中时按概率生成实体。
///
/// Nested struct for projectile on hit: spawns entities with a chance on
/// hit.
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

/// 抛射物命中效果嵌套结构：抛射物命中时的完整效果定义（伤害、效果、
/// 事件、生成物等）。
///
/// Nested struct for projectile: the full effect definition applied when a
/// projectile hits (damage, effects, events, spawns).
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
    pub freeze_on_hit: bool,
    #[serde(default)]
    pub grant_xp: bool,
    #[serde(default)]
    pub hurt_owner: bool,
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
    pub remove_on_hit: ProjectileOnHitParticleOnHit,
    #[serde(default)]
    pub spawn_aoe_cloud: ProjectileOnHitSpawnAoeCloud,
    #[serde(default)]
    pub spawn_chance: ProjectileOnHitSpawnChance,
    #[serde(default)]
    pub stick_in_ground: bool,
    #[serde(default)]
    pub teleport_owner: bool,
    #[serde(default)]
    pub thrown_potion_effect: bool,
}

/// 实体组件：抛射物——定义实体发射的抛射物的物理与命中行为（如骷髅的
/// 箭、烈焰人的火球）。
///
/// Entity component: projectile — defines the physics and hit behavior of
/// projectiles the entity shoots (e.g. skeleton arrows, blaze fireballs).
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
    /// 抛射物散射的基础误差。
    // Base inaccuracy of the projectile spread.
    #[serde(default)]
    pub uncertainty_base: f64,
    /// 抛射物散射误差随难度/移动的放大倍率。
    // Multiplier scaling projectile spread by difficulty or motion.
    #[serde(default)]
    pub uncertainty_multiplier: f64,
}

/// 实体组件：穿透推动——定义实体推动其他实体穿过的能力。
///
/// Entity component: push_through — defines how the entity pushes through
/// other entities.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct PushThrough {
    #[serde(default)]
    pub value: f64,
}

/// 实体组件：可推动——定义实体是否可被其他实体或活塞推动。
///
/// Entity component: pushable — defines whether the entity can be pushed
/// by other entities or pistons.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Pushable {
    #[serde_inline_default(true)]
    pub is_pushable: bool,
    #[serde_inline_default(true)]
    pub is_pushable_by_piston: bool,
}

/// 实体组件：袭击触发器——实体被敲响钟等方式触发时开启袭击并触发事件。
///
/// Entity component: raid_trigger — starts a raid when triggered (e.g. by
/// ringing a bell) and fires an event.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RaidTrigger {
    #[serde(default)]
    pub triggered_event: String,
}

/// 实体组件：铁轨移动——定义实体在铁轨上移动的最大速度（如矿车）。
///
/// Entity component: rail_movement — defines the entity's maximum speed on
/// rails (e.g. minecarts).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RailMovement {
    #[serde_inline_default(0.4)]
    pub max_speed: f64,
}

/// 实体组件：铁轨传感器——定义实体经过激活/关闭铁轨时的行为（如矿车
/// 触发探测铁轨弹出乘客）。
///
/// Entity component: rail_sensor — defines behavior when the entity passes
/// activator rails (e.g. minecarts ejecting riders on powered rails).
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

/// 劫掠兽受阻反应选项嵌套结构：受阻时按权重随机选择的反应事件。
///
/// Nested struct for ravager blocked: a weighted reaction event chosen at
/// random when blocked.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct RavagerBlockedReactionChoice {
    #[serde_inline_default(1)]
    pub weight: i32,
    #[serde(default)]
    pub value: MinecraftEvent,
}

/// 实体组件：劫掠兽受阻——实体被阻挡时播放反应（如劫掠兽撞墙时的行为）。
///
/// Entity component: ravager_blocked — plays a reaction when the entity is
/// blocked (e.g. a ravager ramming a wall).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct RavagerBlocked {
    #[serde_inline_default(3.0)]
    pub knockback_strength: f64,
    #[serde(default)]
    pub reaction_choices: Vec<RavagerBlockedReactionChoice>,
}

/// 可骑乘座位嵌套结构：定义单个座位的骑乘参数。
///
/// Nested struct for rideable: the riding parameters of a single seat.
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

/// 实体组件：可骑乘——定义实体的骑乘行为（座位、控制席位等）。
///
/// Entity component: rideable — defines the entity's riding behavior (seats,
/// controlling seat, etc.).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Rideable {
    #[serde(default)]
    pub controlling_seat: i32,
    /// 骑乘者下蹲时是否跳过交互。
    // Whether sneaking riders skip interaction.
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
    /// 是否将附近符合家族条件的实体拉上座位。
    // Whether to pull in nearby entities matching the family types.
    #[serde(default)]
    pub pull_in_entities: bool,
    #[serde(default)]
    pub rider_can_interact: bool,
    #[serde_inline_default(1)]
    pub seat_count: i32,
    #[serde(default)]
    pub seats: Vec<RideableSeat>,
}

/// 实体组件：脚手架攀爬者——实体可以攀爬脚手架。
///
/// Entity component: scaffolding_climber — the entity can climb
/// scaffolding.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ScaffoldingClimber {}

/// 实体组件：缩放——定义实体的模型缩放倍率。
///
/// Entity component: scale — defines the entity's model scale multiplier.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Scale {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

/// 实体组件：随年龄缩放——实体模型大小随年龄在起始与结束缩放之间过渡。
///
/// Entity component: scale_by_age — scales the entity model between a start
/// and end scale as it ages.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ScaleByAge {
    #[serde_inline_default(1.0)]
    pub end_scale: f64,
    #[serde_inline_default(1.0)]
    pub start_scale: f64,
}

/// 实体组件：调度器——按设定的时间范围定时触发实体事件。
///
/// Entity component: scheduler — fires entity events on a schedule within
/// the configured delay range.
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

/// 可分享物品嵌套结构：定义实体对单个物品的分享规则（村民分享食物等）。
///
/// Nested struct for shareables: the entity's sharing rules for a single
/// item (e.g. villagers sharing food).
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct ShareablesItem {
    #[serde(default)]
    pub admire: bool,
    #[serde(default)]
    pub barter: bool,
    /// 实体是否会自己吃掉该物品。
    // Whether the entity consumes the item itself.
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
    /// 是否每次只拾取一个该物品。
    // Whether to pick up only one of this item at a time.
    #[serde(default)]
    pub singular_pickup: bool,
    #[serde(default)]
    pub stored_in_inventory: bool,
    /// 超出 want_amount 后愿意交出的数量。
    // Amount the entity is willing to give away beyond want_amount.
    #[serde(default)]
    pub surplus_amount: i32,
    /// 实体希望拥有的该物品数量。
    // Amount of this item the entity wants to have.
    #[serde(default)]
    pub want_amount: i32,
}

/// 实体组件：可分享——定义实体愿意拾取、持有并与玩家分享的物品（如
/// 幼年村民索要食物）。
///
/// Entity component: shareables — defines the items the entity picks up,
/// holds, and shares with players (e.g. baby villagers asking for food).
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

/// 实体组件：射手——定义实体发射抛射物的能力（如骷髅射箭、烈焰人发
/// 火球）。
///
/// Entity component: shooter — defines the entity's ability to shoot
/// projectiles (e.g. skeletons shooting arrows, blazes firing fireballs).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Shooter {
    #[serde_inline_default(-1)]
    /// 抛射物的附加值（用于区分变体）。
    // Auxiliary value of the projectile (variant discriminator).
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

/// 实体组件：可坐——定义实体坐下与站起时触发的事件（如猫、狼被命令
/// 坐下）。
///
/// Entity component: sittable — defines the events fired when the entity
/// sits or stands (e.g. commanded cats, wolves).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Sittable {
    #[serde(default)]
    pub sit_event: MinecraftEvent,
    #[serde(default)]
    pub stand_event: MinecraftEvent,
}

/// 实体组件：皮肤 ID——定义实体的皮肤标识值。
///
/// Entity component: skin_id — defines the entity's skin identifier value.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SkinId {
    #[serde_inline_default(0)]
    pub value: i32,
}

/// 实体组件：音量——调整实体发出声音的音量。
///
/// Entity component: sound_volume — adjusts the volume of sounds the entity
/// makes.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SoundVolume {
    #[serde_inline_default(1.0)]
    pub value: f64,
}

/// 生成实体项嵌套结构：定义单个可生成实体的规则（如海龟产卵、哞菇
/// 被剪蘑菇后生成）。
///
/// Nested struct for spawn entity: the rule for spawning a single entity
/// (e.g. turtles laying eggs).
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

/// 实体组件：生成实体——定义实体在特定条件下生成其他实体（如海龟产卵、
/// 兔子生成小兔）。
///
/// Entity component: spawn_entity — defines entities spawned by this entity
/// under certain conditions (e.g. turtles laying eggs).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SpawnEntity {
    #[serde(default)]
    pub entities: Vec<SpawnEntityEntity>,
}

/// 法术附加效果嵌套结构：施法时施加的状态效果。
///
/// Nested struct for spell effects: a status effect applied while casting.
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

/// 实体组件：法术效果——实体施法时可施加与移除的状态效果（如唤魔者
/// 的咒术）。
///
/// Entity component: spell_effects — status effects added and removed when
/// the entity casts spells (e.g. evoker magic).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct SpellEffects {
    #[serde(default)]
    pub add_effects: Vec<SpellEffectsAddEffect>,
    pub remove_effects: Vec<String>,
}

/// 实体组件：力量——定义实体的力量属性（影响攻击力）。
///
/// Entity component: strength — defines the entity's strength attribute
/// (affects attack damage).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Strength {
    #[serde_inline_default(5)]
    pub max: i32,
    #[serde_inline_default(1)]
    pub value: i32,
}

/// 实体组件：可驯服——定义实体被驯服的规则（概率、物品、事件）。
///
/// Entity component: tameable — defines how the entity is tamed
/// (probability, items, events).
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

/// 驯服坐骑自动拒绝物品嵌套结构：驯服坐骑会拒绝的食物列表。
///
/// Nested struct for tamemount: items the mount automatically rejects.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct TamemountAutoRejectItems {
    #[serde(default)]
    pub items: Vec<String>,
}

/// 驯服坐骑喂食物品嵌套结构：可用于喂食坐骑的物品列表。
///
/// Nested struct for tamemount: items usable to feed the mount.
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct TamemountFeedItems {
    #[serde(default)]
    pub items: Vec<String>,
}

/// 实体组件：驯服坐骑——定义坐骑的驯服机制（喂食、性格值等，如马）。
///
/// Entity component: tamemount — defines the mount's taming mechanics
/// (feeding, temper, etc., e.g. horses).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Tamemount {
    #[serde_inline_default(5)]
    /// 每次骑乘尝试对性格值的改变量。
    // Temper change per mounting attempt.
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

/// 实体组件：附近目标传感器——感知目标进入/离开感知范围并触发事件。
///
/// Entity component: target_nearby_sensor — senses targets entering or
/// leaving the detection range and fires events.
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

/// 实体组件：传送——定义实体的传送行为（如末影人的随机与定向传送）。
///
/// Entity component: teleport — defines the entity's teleport behavior
/// (e.g. endermen's random and targeted teleportation).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Teleport {
    /// 处于黑暗中时的传送概率。
    // Teleport chance while in darkness.
    #[serde_inline_default(0.01)]
    pub dark_teleport_chance: f64,
    /// 处于光亮处时的传送概率。
    // Teleport chance while in light.
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

/// 实体组件：世界持续 tick——实体即使在远离玩家处也持续 tick（如
/// 末影水晶、盔甲架）。
///
/// Entity component: tick_world — the entity keeps ticking even far from
/// players (e.g. end crystals, armor stands).
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

/// 实体组件：计时器——按固定或随机间隔触发倒计时事件。
///
/// Entity component: timer — fires countdown events at fixed or random
/// intervals.
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

/// 实体组件：交易补货——允许村民的交易在被消耗后重新补货。
///
/// Entity component: trade_resupply — allows villager trades to restock
/// after being used up.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct TradeResupply {}

/// 实体组件：交易表——定义实体的交易列表（如村民的交易）。
///
/// Entity component: trade_table — defines the entity's trade list (e.g.
/// villager trades).
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

/// 实体组件：轨迹——实体移动时沿路径生成方块痕迹。
///
/// Entity component: trail — leaves block trails as the entity moves.
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

/// 转变延迟嵌套结构：实体转变被方块环境延迟的规则（如村民被闪电击中
/// 延迟转变为女巫）。
///
/// Nested struct for transformation: rules delaying the transformation by
/// block conditions (e.g. villagers delayed turning into witches).
#[serde_inline_default]
#[derive(Clone, Deserialize, Debug, Default)]
pub struct TransformationDelay {
    /// 被附近方块延迟转变的概率。
    // Chance the transformation is delayed by nearby blocks.
    #[serde_inline_default(0.0)]
    pub block_assist_chance: f64,
    /// 因方块条件而延迟转变的概率。
    // Chance of the transformation being delayed due to block conditions.
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

/// 实体组件：转变——定义实体转变为另一实体的规则（如村民被雷击变为
/// 女巫、猪灵变为僵尸猪灵）。
///
/// Entity component: transformation — defines how the entity transforms
/// into another entity (e.g. villagers into witches when struck by
/// lightning).
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

/// 实体组件：信任——标记实体信任玩家（如被喂食后信任玩家的狐狸）。
///
/// Entity component: trust — marks the entity as trusting players (e.g.
/// foxes that trust the player after being fed).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Trust {}

/// 实体组件：获取信任——定义实体获得玩家信任的规则（概率、物品、事件）。
///
/// Entity component: trusting — defines how the entity gains a player's
/// trust (probability, items, events).
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

/// 实体组件：实体家族——定义实体所属的家族类型标签。
///
/// Entity component: type_family — defines the family type tags the entity
/// belongs to.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct TypeFamily {
    #[serde(default)]
    pub family: Vec<String>,
}

/// 实体组件：水下移动——定义实体在水下移动的速度。
///
/// Entity component: underwater_movement — defines the entity's movement
/// speed underwater.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct UnderwaterMovement {
    #[serde(default)]
    pub value: f64,
}

/// 实体组件：变体——定义实体的变体值（如马、猫的品种）。
///
/// Entity component: variant — defines the entity's variant value (e.g.
/// horse or cat breeds).
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Variant {
    #[serde_inline_default(0)]
    pub value: i32,
}

/// 实体组件：行走动画速度——定义实体行走动画的播放速度。
///
/// Entity component: walk_animation_speed — defines the playback speed of
/// the entity's walk animation.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct WalkAnimationSpeed {
    #[serde_inline_default(0.0)]
    pub value: f64,
}

/// 实体组件：想要骑乘——标记实体希望被骑乘（如幼年僵尸寻找可骑乘的
/// 实体）。
///
/// Entity component: wants_jockey — marks the entity as wanting to be
/// ridden (e.g. baby zombies looking for mounts).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct WantsJockey {}

/// 实体组件：水中移动——定义实体在水中的移动与阻力。
///
/// Entity component: water_movement — defines the entity's movement and
/// drag in water.
#[serde_inline_default]
#[derive(Clone, Component, Deserialize, Debug)]
pub struct WaterMovement {
    #[serde_inline_default(0.8)]
    pub drag_factor: f64,
}

/// 实体组件：可变自动步进——按状态（基础、被控制、被阻止跳跃）分别定义
/// 实体自动跨越的台阶高度。
///
/// Entity component: variable_max_auto_step — defines the auto step height
/// separately for base, controlled, and jump-prevented states.
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

/// 实体组件：不可被攻击——标记实体免疫攻击（如盔甲架、画）。
///
/// Entity component: cannot_be_attacked — marks the entity as immune to
/// attacks (e.g. armor stands, paintings).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CannotBeAttacked {}

/// 实体组件：忽略不可被攻击——允许该实体攻击本不可被攻击的实体（如
/// 创造模式玩家破坏盔甲架）。
///
/// Entity component: ignore_cannot_be_attacked — allows this entity to
/// attack entities that normally cannot be attacked (e.g. creative players
/// breaking armor stands).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct IgnoreCannotBeAttacked {}

#[cfg(test)]
mod component_regression_tests {
    use super::{Breathable, EnvironmentSensor};

    #[test]
    fn breathable_accepts_float_inhale_time() {
        // Verbatim player.json shape (float seconds).
        let component: Breathable = serde_json::from_str(
            r#"{"breathes_lava": false, "generates_bubbles": false,
                "inhale_time": 3.75, "suffocate_time": -1, "total_supply": 15}"#,
        )
        .expect("float inhale_time parses");
        assert_eq!(component.inhale_time, 3.75);
        assert_eq!(component.suffocate_time, -1);
        // Plain ints keep working.
        let component: Breathable =
            serde_json::from_str(r#"{"inhale_time": 0}"#).expect("int inhale_time parses");
        assert_eq!(component.inhale_time, 0.0);
    }

    #[test]
    fn environment_sensor_accepts_single_map_and_sequence() {
        // Verbatim pillager.json shape (single trigger map).
        let single: EnvironmentSensor = serde_json::from_str(
            r#"{"triggers": {"event": "minecraft:ranged_mode",
                "filters": {"operator": "==", "test": "in_water",
                            "subject": "self", "value": false}}}"#,
        )
        .expect("single trigger map parses");
        assert_eq!(single.triggers.len(), 1);
        assert_eq!(single.triggers[0].event, "minecraft:ranged_mode");
        // Sequence shape keeps working.
        let many: EnvironmentSensor = serde_json::from_str(
            r#"{"triggers": [{"event": "a"}, {"event": "b", "target": "self"}]}"#,
        )
        .expect("trigger sequence parses");
        assert_eq!(many.triggers.len(), 2);
        // Missing triggers default to empty.
        let empty: EnvironmentSensor = serde_json::from_str("{}").expect("missing triggers parses");
        assert!(empty.triggers.is_empty());
    }
}
