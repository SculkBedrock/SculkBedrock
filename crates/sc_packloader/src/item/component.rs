//! 物品组件数据模型：与官方 Bedrock 物品 JSON 组件参考一一对应的 serde
//! 反序列化结构体，描述物品的组件化属性。
//!
//! Item component data model: serde deserialization structs mirroring the
//! official Bedrock item JSON component reference, describing items'
//! componentized attributes.

use crate::ident::block_ident::MinecraftBlockIdent;
use crate::ident::entity_ident::MinecraftEntityIdent;
use crate::ident::item_ident::MinecraftItemIdent;
use crate::types::range::MinecraftRangeType;
use sc_ecs::component::Component;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

/// 允许物品装备在副手槽位。
///
/// Allows the item to be equipped in the off-hand slot.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct AllowOffHand(pub bool);

/// 方块放置器：作为发射器或投掷器内容物时放置指定方块。
///
/// Block placer: places the specified block when used in a dispenser or dropper.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BlockPlacer {
    pub block: MinecraftBlockIdent,
    pub use_on: Vec<MinecraftBlockIdent>,
}

/// 允许物品在创造模式下破坏方块。
///
/// Allows the item to destroy blocks while in creative mode.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanDestroyInCreative(pub bool);

/// 使用冷却：使用后按类别与持续时间进入冷却。
///
/// Cooldown: category and duration of the cooldown after use.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Cooldown {
    pub category: String,
    pub duration: f32,
}

/// 物品对实体造成的攻击伤害值。
///
/// Attack damage dealt by the item to entities.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Damage(pub i32);

/// 挖掘工具：可快速破坏的方块列表及是否受效率附魔加成。
///
/// Digger: list of blocks destroyed quickly and whether efficiency applies.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Digger {
    pub destroy_speeds: Vec<MinecraftBlockIdent>,
    pub use_efficiency: bool,
}

/// 耐久度：最大耐久值与每次使用的受损几率范围。
///
/// Durability: maximum durability and the damage chance range per use.
///
/// `damage_chance` 缺失时按官方默认（100%）处理。
///
/// `damage_chance` defaults to the official 100% when omitted.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Durability {
    #[serde(default = "default_damage_chance")]
    pub damage_chance: MinecraftRangeType<f64>,
    pub max_durability: i32,
}

/// 受损几率缺省值：恒为 100%（每次尝试都受损）。
///
/// Default damage chance: always 100% (every attempt loses durability).
fn default_damage_chance() -> MinecraftRangeType<f64> {
    MinecraftRangeType::Vec([100.0, 100.0])
}

/// 可染色：默认颜色与是否已染色的标志。
///
/// Dyeable: default color and the dyed flag.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Dyeable {
    pub default_color: Option<String>,
    pub dyed: Option<String>,
}

/// 可附魔：可附魔的槽位与附魔权重值。
///
/// Enchantable: enchant slot and enchantment value (weight).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Enchantable {
    pub slot: String,
    pub value: i32,
}
/// 实体放置器：作为发射器内容物时在指定方块上生成实体。
///
/// Entity placer: spawns the entity on the specified blocks when used in a dispenser.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct EntityPlacer {
    pub dispense_on: Vec<MinecraftBlockIdent>,
    pub entity: MinecraftEntityIdent,
    pub use_on: Vec<MinecraftBlockIdent>,
}

/// 食物：营养值、饱和度修正值及食用后的转换物品。
///
/// Food: nutrition, saturation modifier and the item it converts to when eaten.
#[derive(Clone, Component, Deserialize, Debug)]
#[serde_inline_default]
pub struct Food {
    pub can_always_eat: bool,
    #[serde(default)]
    pub nutrition: i32,
    pub saturation_modifier: String,
    pub using_converts_to: Option<MinecraftItemIdent>,
}
/// 燃料：作为熔炉燃料时的燃烧时长。
///
/// Fuel: burn duration when used as furnace fuel.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Fuel {
    pub duration: f32,
}

/// 物品是否带有附魔光泽特效。
///
/// Whether the item shows the enchantment glint effect.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Glint(pub bool);

/// 物品是否以手持姿态渲染（手臂姿态）。
///
/// Whether the item is hand-equipped (held pose).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct HandEquipped(pub bool);

/// 悬浮提示文本的颜色（JSON 对象）。
///
/// Color of the hover text (JSON object).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct HoverTextColor(
    pub String, //??? JSON object
);

/// 物品图标：纹理名称。
///
/// Item icon: texture name.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Icon {
    pub textures: String, //textures
}

/// 是否显示交互按钮（如骑乘、使用）。
///
/// Whether to show the interact button (e.g. ride, use).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct InteractButton(pub bool);

/// 物品是否被液体裁剪（水下渲染行为）。
///
/// Whether the item is clipped by liquid (underwater rendering behavior).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct LiquidClipped(pub bool);

/// 物品的最大堆叠数量。
///
/// Maximum stack size of the item.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct MaxStackSize(pub u8);

/// 弹射物：最低暴击蓄力与发射的实体。
///
/// Projectile: minimum critical power and the projectile entity to launch.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Projectile {
    pub minimum_critical_power: f32,
    pub projectile_entity: String,
}

/// 物品稀有度（影响名称颜色）。
///
/// Rarity of the item (affects name color).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Rarity(pub String);

/// 音乐唱片：比较器红石信号与播放的音效事件。
///
/// Music record: comparator redstone signal and the sound event to play.
#[derive(Clone, Component, Deserialize, Debug)]
#[serde_inline_default]
pub struct Record {
    #[serde_inline_default(1)]
    pub comparator_signal: i32,
    pub duration: f32,
    pub sound_event: String,
}

/// 可修复：修复所需的物品列表与修复后触发的事件。
///
/// Repairable: list of items that can repair it and the event fired when repaired.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Repairable {
    pub on_repaired: String,
    pub repair_items: Vec<MinecraftItemIdent>,
}

/// 射击器（弓/弩）：可发射的弹药及拉射相关行为。
///
/// Shooter (bow/crossbow): ammunition it can fire and draw-related behavior.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Shooter {
    pub ammunition: Vec<MinecraftEntityIdent>,
    #[serde(default)]
    pub charge_on_draw: bool,
    #[serde(default)]
    pub max_draw_duration: f32,
    #[serde(default)]
    pub scale_power_by_draw_duration: bool,
}

/// 掉落物是否会在超时后消失。
///
/// Whether the dropped item should despawn after timing out.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct ShouldDespawn(pub bool);

/// 物品是否按数据值（NBT）堆叠。
///
/// Whether the item stacks by data value (NBT).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct StackedByData(pub bool);

/// 物品标签列表。
///
/// List of item tags.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Tags {
    pub tags: Vec<String>,
}

/// 可投掷：投掷力度缩放、蓄力时长与挥动动画。
///
/// Throwable: launch power scale, draw duration and swing animation.
#[derive(Clone, Component, Deserialize, Debug)]
#[serde_inline_default]
pub struct Throwable {
    #[serde(default)]
    pub do_swing_animation: bool,
    #[serde_inline_default(1.0)]
    pub launch_power_scale: f32,
    #[serde(default)]
    pub max_draw_duration: f32,
    #[serde_inline_default(1.0)]
    pub max_launch_power: f32,
    #[serde(default)]
    pub min_draw_duration: f32,
    #[serde(default)]
    pub scale_power_by_draw_duration: bool,
}

/// 使用动画（进食、饮用、拉弓等）。
///
/// Use animation (eat, drink, bow, etc.).
#[derive(Clone, Component, Deserialize, Debug)]
pub struct UseAnimation(pub String);

/// 使用修正：使用时的移动速度修正与使用时长。
///
/// Use modifiers: movement modifier while using and the use duration.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct UseModifiers {
    pub movement_modifier: f32,
    pub use_duration: f32,
}

/// 可穿戴：装备槽位、防护值与是否可从发射器分配。
///
/// Wearable: equipment slot, protection value and whether dispensable.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Wearable {
    #[serde(default)]
    pub protection: i32,
    #[serde(default)]
    pub dispensable: bool,
    pub slot: String,
}

/*
 * The following are SC custom components, not usable in BDS
 */

/// 方块物品：SC 自定义组件，BDS 不可用。
///
/// Block item: SC custom component, not usable in BDS.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct BlockItem {}

/// 可放置于：SC 自定义组件，BDS 不可用。
///
/// Can place on: SC custom component, not usable in BDS.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanPlaceOn {
    pub place_on: Vec<String>, //identifiers
}

/// 可破坏：SC 自定义组件，BDS 不可用。
///
/// Can destroy: SC custom component, not usable in BDS.
#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanDestroy {
    pub destroy: Vec<String>, //identifiers
}

/// 耐久损耗量：SC 自定义组件，BDS 不可用。
///
/// Durability costs: SC custom component, not usable in BDS.
///
/// 只声明**官方组件表达不了的策略**：每次动作各扣多少耐久。容量与受损概率
/// 一律由官方 `minecraft:durability` 提供（见 [`Durability`]），本组件不重复声明，
/// 避免同一语义出现两个可写位置。
///
/// Declares only the policy the official component cannot express: how many
/// durability points each action costs. Capacity and damage chance always come
/// from the official [`Durability`] component; this component never repeats them,
/// so no semantic has two writable homes.
///
/// ```json
/// "sc:durability_costs": { "mine_correct": 1, "mine_incorrect": 2 }
/// ```
#[derive(Clone, Copy, Component, Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DurabilityCosts {
    /// 正确工具破坏方块（缺省 1）。
    ///
    /// Breaking a block with the correct tool (defaults to 1).
    #[serde(default = "default_mine_correct_cost")]
    pub mine_correct: u32,
    /// 非正确工具破坏方块（缺省 2）。
    ///
    /// Breaking a block with an incorrect tool (defaults to 2).
    #[serde(default = "default_mine_incorrect_cost")]
    pub mine_incorrect: u32,
    /// 攻击实体（缺省 1；首版不触发）。
    ///
    /// Attacking an entity (defaults to 1; not wired in v1).
    #[serde(default = "default_unit_cost")]
    pub attack: u32,
    /// 一般物品使用（缺省 1；首版不触发）。
    ///
    /// Item use (defaults to 1; not wired in v1).
    #[serde(default = "default_unit_cost")]
    pub use_item: u32,
}

impl Default for DurabilityCosts {
    /// 默认损耗：正确工具 1 点、错误工具 2 点、其余 1 点（原版语义）。
    ///
    /// Default costs: 1 for the correct tool, 2 for an incorrect one, else 1
    /// (vanilla semantics).
    fn default() -> Self {
        Self {
            mine_correct: default_mine_correct_cost(),
            mine_incorrect: default_mine_incorrect_cost(),
            attack: default_unit_cost(),
            use_item: default_unit_cost(),
        }
    }
}

fn default_mine_correct_cost() -> u32 {
    1
}

fn default_mine_incorrect_cost() -> u32 {
    2
}

fn default_unit_cost() -> u32 {
    1
}
