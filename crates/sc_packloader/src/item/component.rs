use crate::ident::block_ident::MinecraftBlockIdent;
use crate::ident::entity_ident::MinecraftEntityIdent;
use crate::ident::item_ident::MinecraftItemIdent;
use crate::types::range::MinecraftRangeType;
use sc_ecs::component::Component;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

/*
 warning: All code below was AI-generated and has not been verified for correctness
 Official docs:
 https://learn.microsoft.com/en-us/minecraft/creator/reference/content/itemreference/examples/itemcomponentlist?view=minecraft-bedrock-stable
*/

#[derive(Clone, Component, Deserialize, Debug)]
pub struct AllowOffHand(pub bool);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct BlockPlacer {
    pub block: MinecraftBlockIdent,
    pub use_on: Vec<MinecraftBlockIdent>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanDestroyInCreative(pub bool);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Cooldown {
    pub category: String,
    pub duration: f32,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Damage(pub i32);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Digger {
    pub destroy_speeds: Vec<MinecraftBlockIdent>,
    pub use_efficiency: bool,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Durability {
    pub damage_chance: MinecraftRangeType<f64>,
    pub max_durability: i32,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Dyeable {
    pub default_color: Option<String>,
    pub dyed: Option<String>,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Enchantable {
    pub slot: String,
    pub value: i32,
}
#[derive(Clone, Component, Deserialize, Debug)]
pub struct EntityPlacer {
    pub dispense_on: Vec<MinecraftBlockIdent>,
    pub entity: MinecraftEntityIdent,
    pub use_on: Vec<MinecraftBlockIdent>,
}

#[derive(Clone, Component, Deserialize, Debug)]
#[serde_inline_default]
pub struct Food {
    pub can_always_eat: bool,
    #[serde(default)]
    pub nutrition: i32,
    pub saturation_modifier: String,
    pub using_converts_to: Option<MinecraftItemIdent>,
}
#[derive(Clone, Component, Deserialize, Debug)]
pub struct Fuel {
    pub duration: f32,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Glint(pub bool);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct HandEquipped(pub bool);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct HoverTextColor(
    pub String, //??? JSON object
);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Icon {
    pub textures: String, //textures
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct InteractButton(pub bool);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct LiquidClipped(pub bool);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct MaxStackSize(pub u8);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Projectile {
    pub minimum_critical_power: f32,
    pub projectile_entity: String,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Rarity(pub String);

#[derive(Clone, Component, Deserialize, Debug)]
#[serde_inline_default]
pub struct Record {
    #[serde_inline_default(1)]
    pub comparator_signal: i32,
    pub duration: f32,
    pub sound_event: String,
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Repairable {
    pub on_repaired: String,
    pub repair_items: Vec<MinecraftItemIdent>,
}

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

#[derive(Clone, Component, Deserialize, Debug)]
pub struct ShouldDespawn(pub bool);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct StackedByData(pub bool);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct Tags {
    pub tags: Vec<String>,
}

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

#[derive(Clone, Component, Deserialize, Debug)]
pub struct UseAnimation(pub String);

#[derive(Clone, Component, Deserialize, Debug)]
pub struct UseModifiers {
    pub movement_modifier: f32,
    pub use_duration: f32,
}

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

#[derive(Clone, Component, Deserialize, Debug)]
pub struct BlockItem {}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanPlaceOn {
    pub place_on: Vec<String>, //identifiers
}

#[derive(Clone, Component, Deserialize, Debug)]
pub struct CanDestroy {
    pub destroy: Vec<String>, //identifiers
}
