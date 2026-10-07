pub mod component;
pub mod nbt;

use crate::components_export;
use crate::item::component::*;
use crate::version_control::runtime::MinecraftRuntimeManager;
use parking_lot::RwLock;
use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use sc_utils::components::{DisplayName, RuntimeID};
use serde::Deserialize;
use serde_inline_default::serde_inline_default;
use serde_json::{Map, Value};
use std::sync::Arc;

#[serde_inline_default]
#[derive(Deserialize, Debug, Clone, Default)]
pub struct MinecraftItemMenuCategory {
    #[serde(default)]
    pub group: String,
    #[serde_inline_default("items".to_string())]
    pub category: String,
    #[serde(default)]
    pub is_hidden_in_commands: bool,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct MinecraftItemDescription {
    pub identifier: String,
    #[serde(default)]
    pub menu_category: MinecraftItemMenuCategory,
}

#[derive(Component, Debug)]
pub struct MinecraftItem {
    pub format_version: String,
    pub description: Option<MinecraftItemDescription>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct MinecraftItemSpawner {
    #[serde(skip_deserializing)]
    pub format_version: String,
    pub description: Option<MinecraftItemDescription>,
    pub components: Option<ItemComponents>,
    //pub default_nbt: Option<CompoundNbt>
}

components_export![
    ItemComponents,
    AllowOffHand = "minecraft:allow_off_hand",
    BlockPlacer = "minecraft:block_placer",
    CanDestroyInCreative = "minecraft:can_destroy_in_creative",
    Cooldown = "minecraft:cooldown",
    Damage = "minecraft:damage",
    Digger = "minecraft:digger",
    DisplayName = "minecraft:display_name",
    Durability = "minecraft:durability",
    Dyeable = "minecraft:dyeable",
    Enchantable = "minecraft:enchantable",
    EntityPlacer = "minecraft:entity_placer",
    Food = "minecraft:food",
    Fuel = "minecraft:fuel",
    Glint = "minecraft:glint",
    HandEquipped = "minecraft:hand_equipped",
    HoverTextColor = "minecraft:hover_text_color",
    Icon = "minecraft:icon",
    InteractButton = "minecraft:interact_button",
    LiquidClipped = "minecraft:liquid_clipped",
    MaxStackSize = "minecraft:max_stack_size",
    Projectile = "minecraft:projectile",
    Rarity = "minecraft:rarity",
    Record = "minecraft:record",
    Repairable = "minecraft:repairable",
    Shooter = "minecraft:shooter",
    ShouldDespawn = "minecraft:should_despawn",
    StackedByData = "minecraft:stacked_by_data",
    Tags = "minecraft:tags",
    Throwable = "minecraft:throwable",
    UseAnimation = "minecraft:use_animation",
    UseModifiers = "minecraft:use_modifiers",
    Wearable = "minecraft:wearable",
    //SC Components
    RuntimeID = "ur:runtime_id",
    CanPlaceOn = "ur:can_place_on",
    CanDestroy = "ur:can_destroy",
];

impl ItemComponents {
    /// Build the NBT payload used by Bedrock's ItemRegistry/ItemComponent packet.
    ///
    /// The version pack stores item components as JSON because that is the
    /// authoring format used by behavior packs. Network code must not hardcode
    /// packet bytes, so this method converts the semantic component JSON into
    /// Bedrock network NBT at the packloader boundary.
    ///
    /// SC-only helper components are intentionally filtered out; only
    /// `minecraft:*` keys are visible to the Bedrock client.
    pub fn to_network_component_tag(&self) -> CompoundNbt {
        self.to_network_component_tag_with_item(None)
    }

    pub fn to_network_component_tag_with_item(
        &self,
        item: Option<&MinecraftItemSpawner>,
    ) -> CompoundNbt {
        let mut tag = CompoundNbt::new(None);
        let components = self.to_network_components(item);

        if !components.is_empty() {
            tag.insert("components", NbtValue::Compound(components));
        }
        tag
    }

    fn to_network_components(&self, item: Option<&MinecraftItemSpawner>) -> CompoundNbt {
        let mut components = CompoundNbt::new(None);
        match self {
            ItemComponents::Map(map) => {
                let data_driven = is_data_driven_item(item, map);
                if data_driven {
                    let item_properties = build_item_properties(item, map);
                    if !item_properties.is_empty() {
                        components.insert("item_properties", NbtValue::Compound(item_properties));
                    }

                    if let Some(item_tags) = build_item_tags(map) {
                        components.insert("item_tags", item_tags);
                    }
                }

                for (key, value) in map {
                    if !key.starts_with("minecraft:") {
                        continue;
                    }
                    if component_is_item_property_only(key, data_driven) {
                        continue;
                    }
                    if let Some(value) = component_value_to_network_nbt(key, value, data_driven) {
                        components.insert(key, value);
                    }
                }
            }
        }
        components
    }
}

impl MinecraftItemSpawner {
    /// Build the Bedrock ItemRegistry component tag from the already-loaded
    /// version-pack item JSON.
    ///
    /// Some builds ship a prebuilt `item_components.nbt`, but that file is a compiled
    /// form of item component semantics. SC keeps the version pack as the source
    /// of truth and constructs the same network shape from `items/*.json`.
    pub fn to_network_component_tag(&self) -> CompoundNbt {
        self.components
            .as_ref()
            .map(|components| components.to_network_component_tag_with_item(Some(self)))
            .unwrap_or_else(|| CompoundNbt::new(None))
    }
}

/// Dense item-component table indexed by network runtime id for gameplay queries.
///
/// Same data source as [MinecraftItemSpawner] (version-pack `items/*.json` plus behavior packs),
/// but it compiles components from one-shot packet data into a queryable runtime resource:
/// gameplay systems can call `has::<Food>(runtime_id)` / `get::<Food>(runtime_id)` to ask
/// edibility/durability/fuel semantics without touching network protocol.
///
/// Relation to [sc_item::ItemRegistry]: the registry describes what an item is (id/name/stack limit/
/// linked block); this table describes what an item can do (components). Bootstrap builds both at SCLoad.
#[derive(Resource, Clone, Debug, Default)]
pub struct ItemComponentTable {
    entries: Arc<RwLock<Vec<Option<ItemComponents>>>>,
}

impl ItemComponentTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compile the dense table from [MinecraftRuntimeManager] spawners.
    ///
    /// Only items with component definitions are included; runtime id is the Vec index (air runtime 0 excluded).
    /// Call once during SCLoad (after runtime ids are loaded into the manager).
    pub fn build(&mut self, runtime_manager: &MinecraftRuntimeManager) {
        let mut entries = self.entries.write();
        entries.clear();
        for (name, id) in runtime_manager.runtime_entries() {
            let runtime_id = id as u16;
            if runtime_id == 0 {
                continue;
            }
            let Some(spawner) = runtime_manager.get_item_spawner(id) else {
                continue;
            };
            let Some(components) = spawner.components.clone() else {
                continue;
            };
            if entries.len() <= runtime_id as usize {
                entries.resize(runtime_id as usize + 1, None);
            }
            entries[runtime_id as usize] = Some(components);
        }
    }

    pub fn raw(&self, runtime_id: u16) -> Option<ItemComponents> {
        self.entries
            .read()
            .get(runtime_id as usize)
            .cloned()
            .flatten()
    }

    /// Whether the item has the given component (e.g. `Food`, `Durability`, `Fuel`).
    pub fn has<T>(&self, runtime_id: u16) -> bool
    where
        T: Component + serde::de::DeserializeOwned,
    {
        self.raw(runtime_id)
            .and_then(|components| components.get::<T>())
            .is_some()
    }

    /// Read the given component instance (e.g. `Food { nutrition, saturation_modifier, .. }`).
    pub fn get<T>(&self, runtime_id: u16) -> Option<T>
    where
        T: Component + serde::de::DeserializeOwned,
    {
        self.raw(runtime_id)?.get::<T>()
    }

    pub fn len(&self) -> usize {
        self.entries.read().len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }

    /// Number of items with component definitions (skips hole indices).
    pub fn count(&self) -> usize {
        self.entries
            .read()
            .iter()
            .filter(|entry| entry.is_some())
            .count()
    }
}

fn is_data_driven_item(item: Option<&MinecraftItemSpawner>, map: &Map<String, Value>) -> bool {
    map.contains_key("minecraft:icon")
        || item
            .map(|item| version_at_least(&item.format_version, 1, 20, 0))
            .unwrap_or(false)
}

fn version_at_least(version: &str, major: u32, minor: u32, patch: u32) -> bool {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    let actual = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    actual >= (major, minor, patch)
}

fn component_is_item_property_only(key: &str, data_driven: bool) -> bool {
    data_driven
        && matches!(
            key,
            "minecraft:allow_off_hand"
                | "minecraft:can_destroy_in_creative"
                | "minecraft:glint"
                | "minecraft:icon"
                | "minecraft:liquid_clipped"
                | "minecraft:should_despawn"
                | "minecraft:stacked_by_data"
                | "minecraft:use_duration"
        )
}

fn build_item_properties(
    item: Option<&MinecraftItemSpawner>,
    map: &Map<String, Value>,
) -> CompoundNbt {
    let mut properties = CompoundNbt::new(None);

    properties.insert(
        "allow_off_hand",
        NbtValue::Byte(component_bool(map, "minecraft:allow_off_hand", false) as i8),
    );
    properties.insert(
        "can_destroy_in_creative",
        NbtValue::Byte(component_bool(map, "minecraft:can_destroy_in_creative", true) as i8),
    );
    properties.insert(
        "creative_category",
        NbtValue::Int(item.and_then(item_creative_category).unwrap_or(4)),
    );
    properties.insert(
        "creative_group",
        NbtValue::String(
            item.and_then(|item| item.description.as_ref())
                .map(|description| description.menu_category.group.clone())
                .unwrap_or_default(),
        ),
    );
    properties.insert(
        "damage",
        NbtValue::Int(component_i32(map, "minecraft:damage", 0)),
    );
    properties.insert(
        "enchantable_slot",
        NbtValue::String(component_string_nested(
            map,
            "minecraft:enchantable",
            "slot",
            "none",
        )),
    );
    properties.insert(
        "enchantable_value",
        NbtValue::Int(component_i32_nested(
            map,
            "minecraft:enchantable",
            "value",
            0,
        )),
    );
    properties.insert(
        "foil",
        NbtValue::Byte(component_bool(map, "minecraft:glint", false) as i8),
    );
    properties.insert("frame_count", NbtValue::Int(1));
    properties.insert(
        "hand_equipped",
        NbtValue::Byte(component_bool(map, "minecraft:hand_equipped", false) as i8),
    );
    properties.insert(
        "hidden_in_commands",
        NbtValue::Int(
            item.and_then(|item| item.description.as_ref())
                .map(|description| {
                    if description.menu_category.is_hidden_in_commands {
                        1
                    } else {
                        2
                    }
                })
                .unwrap_or(2),
        ),
    );
    properties.insert(
        "liquid_clipped",
        NbtValue::Byte(component_bool(map, "minecraft:liquid_clipped", false) as i8),
    );
    properties.insert(
        "max_stack_size",
        NbtValue::Int(component_i32(map, "minecraft:max_stack_size", 64)),
    );
    if let Some(icon) = map.get("minecraft:icon").and_then(json_value_to_nbt) {
        properties.insert("minecraft:icon", icon);
    }
    properties.insert("mining_speed", NbtValue::Float(1.0));
    properties.insert(
        "should_despawn",
        NbtValue::Byte(component_bool(map, "minecraft:should_despawn", true) as i8),
    );
    properties.insert(
        "stacked_by_data",
        NbtValue::Byte(component_bool(map, "minecraft:stacked_by_data", false) as i8),
    );
    properties.insert(
        "use_animation",
        NbtValue::Int(component_use_animation_id(map)),
    );
    properties.insert(
        "use_duration",
        NbtValue::Int(component_use_duration_ticks(map)),
    );

    properties
}

fn item_creative_category(item: &MinecraftItemSpawner) -> Option<i32> {
    let category = item
        .description
        .as_ref()?
        .menu_category
        .category
        .to_ascii_lowercase();
    Some(match category.as_str() {
        "construction" => 1,
        "nature" => 2,
        "equipment" => 3,
        "items" => 4,
        "commands" | "item_command_only" => 5,
        "none" => 6,
        _ => 4,
    })
}

fn build_item_tags(map: &Map<String, Value>) -> Option<NbtValue> {
    let tags = map
        .get("minecraft:tags")?
        .as_object()?
        .get("tags")?
        .as_array()?
        .iter()
        .filter_map(|value| value.as_str())
        .map(|value| NbtValue::String(value.to_string()))
        .collect::<Vec<_>>();
    Some(NbtValue::List(tags))
}

fn component_value_to_network_nbt(key: &str, value: &Value, data_driven: bool) -> Option<NbtValue> {
    let mut value = json_value_to_nbt(value)?;

    match key {
        "minecraft:cooldown" => {
            if let Some(compound) = value.as_compound_mut() {
                if !compound.contains_key("type") {
                    compound.insert("type", NbtValue::String("use".to_string()));
                }
            }
            Some(value)
        }
        "minecraft:damage"
        | "minecraft:hand_equipped"
        | "minecraft:max_stack_size"
        | "minecraft:use_animation"
            if data_driven =>
        {
            Some(wrap_scalar_value(value))
        }
        "minecraft:food" => {
            normalize_food(&mut value, data_driven);
            Some(value)
        }
        "minecraft:projectile" => {
            normalize_projectile(&mut value);
            Some(value)
        }
        "minecraft:kinetic_weapon" => Some(wrap_component_name(key, value)),
        "minecraft:repairable" | "minecraft:storage_item" => {
            normalize_item_reference_lists(&mut value);
            Some(value)
        }
        "minecraft:throwable" => {
            normalize_throwable(&mut value);
            Some(value)
        }
        "minecraft:use_modifiers" => {
            normalize_use_modifiers(&mut value);
            Some(value)
        }
        _ => Some(value),
    }
}

fn wrap_scalar_value(value: NbtValue) -> NbtValue {
    match value {
        NbtValue::Compound(_) => value,
        value => {
            let mut compound = CompoundNbt::new(None);
            compound.insert("value", value);
            NbtValue::Compound(compound)
        }
    }
}

fn wrap_component_name(key: &str, value: NbtValue) -> NbtValue {
    match &value {
        NbtValue::Compound(compound) if compound.contains_key(key) => value,
        _ => {
            let mut compound = CompoundNbt::new(None);
            compound.insert(key, value);
            NbtValue::Compound(compound)
        }
    }
}

fn normalize_food(value: &mut NbtValue, data_driven: bool) {
    let Some(compound) = value.as_compound_mut() else {
        return;
    };

    if !compound.contains_key("can_always_eat") {
        compound.insert("can_always_eat", NbtValue::Byte(0));
    }
    if let Some(NbtValue::String(modifier)) = compound.get("saturation_modifier").cloned() {
        compound.insert(
            "saturation_modifier",
            NbtValue::Float(saturation_modifier_value(&modifier)),
        );
    }
    if data_driven && !compound.contains_key("using_converts_to") {
        compound.insert(
            "using_converts_to",
            NbtValue::Compound(CompoundNbt::new(None)),
        );
    } else if !data_driven {
        if !compound.contains_key("cooldown_time") {
            compound.insert("cooldown_time", NbtValue::Int(0));
        }
        if !compound.contains_key("cooldown_type") {
            compound.insert("cooldown_type", NbtValue::String(String::new()));
        }
        if !compound.contains_key("on_use_action") {
            compound.insert("on_use_action", NbtValue::Int(-1));
        }
        if !compound.contains_key("on_use_range") {
            compound.insert(
                "on_use_range",
                NbtValue::List(vec![
                    NbtValue::Float(8.0),
                    NbtValue::Float(8.0),
                    NbtValue::Float(8.0),
                ]),
            );
        }
        if !compound.contains_key("using_converts_to") {
            compound.insert("using_converts_to", NbtValue::String(String::new()));
        }
    }
}

fn saturation_modifier_value(value: &str) -> f32 {
    match value {
        "poor" => 0.1,
        "low" => 0.3,
        "normal" => 0.6,
        "good" => 0.8,
        "max" | "supernatural" => 1.2,
        _ => value.parse::<f32>().unwrap_or(0.6),
    }
}

fn normalize_projectile(value: &mut NbtValue) {
    let Some(compound) = value.as_compound_mut() else {
        return;
    };
    if !compound.contains_key("minimum_critical_power") {
        compound.insert("minimum_critical_power", NbtValue::Float(0.0));
    }
    if let Some(NbtValue::String(projectile)) = compound.get("projectile_entity").cloned() {
        let mut projectile = projectile;
        if !projectile.contains(':') {
            projectile = format!("minecraft:{projectile}");
        }
        if !projectile.ends_with("<>") {
            projectile.push_str("<>");
        }
        compound.insert("projectile_entity", NbtValue::String(projectile));
    }
}

fn normalize_throwable(value: &mut NbtValue) {
    let Some(compound) = value.as_compound_mut() else {
        return;
    };
    insert_default_byte(compound, "do_swing_animation", 0);
    insert_default_float(compound, "launch_power_scale", 1.0);
    insert_default_float(compound, "max_draw_duration", 0.0);
    insert_default_float(compound, "max_launch_power", 1.0);
    insert_default_float(compound, "min_draw_duration", 0.0);
    insert_default_byte(compound, "scale_power_by_draw_duration", 0);
}

fn normalize_use_modifiers(value: &mut NbtValue) {
    let Some(compound) = value.as_compound_mut() else {
        return;
    };
    insert_default_byte(compound, "emit_vibrations", 1);
    insert_default_float(compound, "movement_modifier", 1.0);
    normalize_float_field(compound, "movement_modifier");
    normalize_float_field(compound, "use_duration");
    if !compound.contains_key("start_using") {
        compound.insert("start_using", NbtValue::String("always".to_string()));
    }
}

fn normalize_item_reference_lists(value: &mut NbtValue) {
    let Some(compound) = value.as_compound_mut() else {
        return;
    };
    for key in ["allowed_items", "banned_items"] {
        normalize_named_item_list(compound, key);
    }
    if let Some(NbtValue::List(repair_items)) = compound.get_mut("repair_items") {
        for repair_item in repair_items {
            let Some(repair_compound) = repair_item.as_compound_mut() else {
                continue;
            };
            normalize_named_item_list(repair_compound, "items");
        }
    }
}

fn normalize_named_item_list(compound: &mut CompoundNbt, key: &str) {
    let Some(NbtValue::List(items)) = compound.get_mut(key) else {
        return;
    };
    for item in items {
        let NbtValue::String(name) = item else {
            continue;
        };
        let mut named = CompoundNbt::new(None);
        named.insert("name", NbtValue::String(name.clone()));
        *item = NbtValue::Compound(named);
    }
}

fn insert_default_byte(compound: &mut CompoundNbt, key: &str, value: i8) {
    if !compound.contains_key(key) {
        compound.insert(key, NbtValue::Byte(value));
    }
}

fn insert_default_float(compound: &mut CompoundNbt, key: &str, value: f32) {
    if !compound.contains_key(key) {
        compound.insert(key, NbtValue::Float(value));
    }
}

fn normalize_float_field(compound: &mut CompoundNbt, key: &str) {
    let Some(value) = compound.get(key).cloned() else {
        return;
    };
    match value {
        NbtValue::Byte(value) => {
            compound.insert(key, NbtValue::Float(value as f32));
        }
        NbtValue::Short(value) => {
            compound.insert(key, NbtValue::Float(value as f32));
        }
        NbtValue::Int(value) => {
            compound.insert(key, NbtValue::Float(value as f32));
        }
        NbtValue::Long(value) => {
            compound.insert(key, NbtValue::Float(value as f32));
        }
        NbtValue::Double(value) => {
            compound.insert(key, NbtValue::Float(value as f32));
        }
        _ => {}
    }
}

fn component_bool(map: &Map<String, Value>, key: &str, default: bool) -> bool {
    map.get(key)
        .and_then(value_bool)
        .or_else(|| {
            map.get(key)
                .and_then(|value| value_nested_bool(value, "value"))
        })
        .unwrap_or(default)
}

fn component_i32(map: &Map<String, Value>, key: &str, default: i32) -> i32 {
    map.get(key)
        .and_then(value_i32)
        .or_else(|| {
            map.get(key)
                .and_then(|value| value_nested_i32(value, "value"))
        })
        .unwrap_or(default)
}

fn component_i32_nested(
    map: &Map<String, Value>,
    component_key: &str,
    nested_key: &str,
    default: i32,
) -> i32 {
    map.get(component_key)
        .and_then(|value| value_nested_i32(value, nested_key))
        .unwrap_or(default)
}

fn component_string_nested(
    map: &Map<String, Value>,
    component_key: &str,
    nested_key: &str,
    default: &str,
) -> String {
    map.get(component_key)
        .and_then(|value| value.as_object())
        .and_then(|object| object.get(nested_key))
        .and_then(|value| value.as_str())
        .unwrap_or(default)
        .to_string()
}

fn component_use_animation_id(map: &Map<String, Value>) -> i32 {
    let value = map
        .get("minecraft:use_animation")
        .and_then(value_string)
        .or_else(|| {
            map.get("minecraft:use_animation")
                .and_then(|value| value_nested_string(value, "value"))
        });
    match value.as_deref() {
        Some("eat") => 1,
        Some("drink") => 2,
        Some("bow") => 4,
        Some("block") => 5,
        Some("camera") => 6,
        Some("spear") => 7,
        Some("spyglass") => 8,
        Some("crossbow") => 9,
        Some("brush") => 10,
        _ => 0,
    }
}

fn component_use_duration_ticks(map: &Map<String, Value>) -> i32 {
    if let Some(duration) = map.get("minecraft:use_duration").and_then(value_f64) {
        return duration.round() as i32;
    }
    if let Some(duration) = map
        .get("minecraft:use_modifiers")
        .and_then(|value| value_nested_f64(value, "use_duration"))
    {
        return (duration * 20.0).round() as i32;
    }
    0
}

fn value_bool(value: &Value) -> Option<bool> {
    value.as_bool()
}

fn value_i32(value: &Value) -> Option<i32> {
    value.as_i64().and_then(|value| i32::try_from(value).ok())
}

fn value_f64(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn value_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn value_nested_bool(value: &Value, key: &str) -> Option<bool> {
    value.as_object()?.get(key)?.as_bool()
}

fn value_nested_i32(value: &Value, key: &str) -> Option<i32> {
    value
        .as_object()?
        .get(key)?
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
}

fn value_nested_f64(value: &Value, key: &str) -> Option<f64> {
    value.as_object()?.get(key)?.as_f64()
}

fn value_nested_string(value: &Value, key: &str) -> Option<String> {
    value.as_object()?.get(key)?.as_str().map(str::to_string)
}

fn json_value_to_nbt(value: &Value) -> Option<NbtValue> {
    match value {
        Value::Null => None,
        Value::Bool(value) => Some(NbtValue::Byte(if *value { 1 } else { 0 })),
        Value::Number(value) => json_number_to_nbt(value),
        Value::String(value) => Some(NbtValue::String(value.clone())),
        Value::Array(values) => {
            let values = values
                .iter()
                .filter_map(json_value_to_nbt)
                .collect::<Vec<_>>();
            Some(NbtValue::List(values))
        }
        Value::Object(map) => {
            let mut compound = CompoundNbt::new(None);
            for (key, value) in map {
                if let Some(value) = json_value_to_nbt(value) {
                    compound.insert(key, value);
                }
            }
            Some(NbtValue::Compound(compound))
        }
    }
}

fn json_number_to_nbt(value: &serde_json::Number) -> Option<NbtValue> {
    if let Some(value) = value.as_i64() {
        if let Ok(value) = i32::try_from(value) {
            Some(NbtValue::Int(value))
        } else {
            Some(NbtValue::Long(value))
        }
    } else if let Some(value) = value.as_u64() {
        if let Ok(value) = i32::try_from(value) {
            Some(NbtValue::Int(value))
        } else if let Ok(value) = i64::try_from(value) {
            Some(NbtValue::Long(value))
        } else {
            Some(NbtValue::Double(value as f64))
        }
    } else {
        value.as_f64().map(|value| NbtValue::Float(value as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version_control::json_budget::BudgetReader;
    use crate::{MinecraftJson, MinecraftJsonDeserializer};

    fn parse_item(json: &str) -> MinecraftItemSpawner {
        match serde_json::from_reader::<_, MinecraftJsonDeserializer>(BudgetReader::new(
            json.as_bytes(),
        ))
        .unwrap()
        .get()
        {
            MinecraftJson::MinecraftItemSpawner(item) => item,
            _ => panic!("expected minecraft:item"),
        }
    }

    fn compound(value: &NbtValue) -> &CompoundNbt {
        value.as_compound().expect("expected compound")
    }

    #[test]
    fn modern_item_json_builds_pnx_shaped_component_tag() {
        let item = parse_item(
            r#"{
              "format_version": "1.26.30",
              "minecraft:item": {
                "description": {
                  "identifier": "minecraft:apple",
                  "menu_category": { "category": "items" }
                },
                "components": {
                  "minecraft:display_name": { "value": "item.apple.name" },
                  "minecraft:food": {
                    "nutrition": 4,
                    "saturation_modifier": 0.3
                  },
                  "minecraft:icon": {
                    "textures": { "default": "apple" }
                  },
                  "minecraft:tags": {
                    "tags": [ "minecraft:is_food" ]
                  },
                  "minecraft:use_animation": { "value": "eat" },
                  "minecraft:use_modifiers": {
                    "start_using": "always",
                    "use_duration": 1.6,
                    "movement_modifier": 0.35
                  }
                }
              }
            }"#,
        );

        let tag = item.to_network_component_tag();
        let components = compound(tag.get("components").unwrap());
        let item_properties = compound(components.get("item_properties").unwrap());
        assert!(item_properties.contains_key("minecraft:icon"));
        assert_eq!(
            item_properties
                .get("use_animation")
                .and_then(NbtValue::as_i32),
            Some(1)
        );
        assert_eq!(
            item_properties
                .get("use_duration")
                .and_then(NbtValue::as_i32),
            Some(32)
        );

        let item_tags = components
            .get("item_tags")
            .and_then(NbtValue::as_list)
            .unwrap();
        assert_eq!(item_tags.len(), 1);
        assert_eq!(
            item_tags[0].as_string().map(String::as_str),
            Some("minecraft:is_food")
        );

        let food = compound(components.get("minecraft:food").unwrap());
        assert!(matches!(
            food.get("using_converts_to"),
            Some(NbtValue::Compound(value)) if value.is_empty()
        ));
    }

    #[test]
    fn legacy_item_json_keeps_legacy_component_shape() {
        let item = parse_item(
            r#"{
              "format_version": "1.10",
              "minecraft:item": {
                "description": {
                  "identifier": "minecraft:beetroot_soup"
                },
                "components": {
                  "minecraft:use_duration": 32,
                  "minecraft:max_stack_size": 1,
                  "minecraft:food": {
                    "nutrition": 6,
                    "saturation_modifier": "normal",
                    "using_converts_to": "bowl"
                  }
                }
              }
            }"#,
        );

        let tag = item.to_network_component_tag();
        let components = compound(tag.get("components").unwrap());
        assert!(!components.contains_key("item_properties"));
        assert_eq!(
            components
                .get("minecraft:max_stack_size")
                .and_then(NbtValue::as_i32),
            Some(1)
        );

        let food = compound(components.get("minecraft:food").unwrap());
        assert_eq!(
            food.get("cooldown_time").and_then(NbtValue::as_i32),
            Some(0)
        );
        assert_eq!(
            food.get("cooldown_type")
                .and_then(NbtValue::as_string)
                .map(String::as_str),
            Some("")
        );
        assert_eq!(
            food.get("on_use_action").and_then(NbtValue::as_i32),
            Some(-1)
        );
        assert_eq!(
            food.get("using_converts_to")
                .and_then(NbtValue::as_string)
                .map(String::as_str),
            Some("bowl")
        );
    }

    #[test]
    fn item_component_table_builds_dense_table_from_spawners() {
        use crate::item::component::Food;
        use crate::version_control::runtime::MinecraftRuntimeJson;
        use std::collections::HashMap;

        let spawner = parse_item(
            r#"{
              "format_version": "1.26.30",
              "minecraft:item": {
                "description": { "identifier": "minecraft:apple" },
                "components": {
                  "minecraft:food": {
                    "nutrition": 4,
                    "saturation_modifier": "normal",
                    "can_always_eat": false
                  }
                }
              }
            }"#,
        );
        let mut manager = MinecraftRuntimeManager::new(vec![MinecraftRuntimeJson {
            name: "minecraft:apple".to_string(),
            id: 257,
            version: 1,
            component_based: true,
        }]);
        let mut map = HashMap::new();
        map.insert("minecraft:apple".to_string(), spawner);
        manager.push_item_map(map);

        let mut table = ItemComponentTable::new();
        table.build(&manager);

        // Dense runtime_id index: slot 257 has components with queryable Food semantics.
        assert!(table.has::<Food>(257));
        let food = table.get::<Food>(257).expect("apple should be food");
        assert_eq!(food.nutrition, 4);
        // Hole indices without defined components return None instead of panicking.
        assert!(!table.has::<Food>(999));
        assert_eq!(table.count(), 1);
    }
}
