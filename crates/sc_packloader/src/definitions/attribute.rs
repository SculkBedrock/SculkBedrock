use parking_lot::{ArcRwLockWriteGuard, RawRwLock, RwLock};
use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use serde::Deserialize;
use serde_json::Map;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Resource, Debug)]
pub struct DefaultEntityAttributes {
    map: HashMap<String, EntityAttribute>,
}

impl DefaultEntityAttributes {
    pub fn new(map: HashMap<String, EntityAttribute>) -> Self {
        Self { map }
    }

    pub fn default(&self) -> EntityAttributes {
        EntityAttributes::from_map(self.map.clone())
    }
}

#[derive(Component, Debug, Clone)]
pub struct EntityAttributes {
    map: Arc<RwLock<HashMap<String, Arc<RwLock<EntityAttribute>>>>>,
}

impl EntityAttributes {
    pub const ABSORPTION: &'static str = "minecraft:absorption";
    pub const SATURATION: &'static str = "minecraft:player.saturation";
    pub const EXHAUSTION: &'static str = "minecraft:player.exhaustion";
    pub const KNOCKBACK_RESISTANCE: &'static str = "minecraft:knockback_resistance";
    pub const MAX_HEALTH: &'static str = "minecraft:health";
    pub const MOVEMENT_SPEED: &'static str = "minecraft:movement";
    pub const FLIGHT_SPEED: &'static str = "ur:flight";
    pub const UNDER_WATER_MOVEMENT_SPEED: &'static str = "minecraft:underwater_movement";
    pub const LAVA_MOVEMENT_SPEED: &'static str = "minecraft:lava_movement";
    pub const FOLLOW_RANGE: &'static str = "minecraft:follow_range";
    pub const MAX_HUNGER: &'static str = "minecraft:player.hunger";
    pub const ATTACK_DAMAGE: &'static str = "minecraft:attack_damage";
    pub const EXPERIENCE_LEVEL: &'static str = "minecraft:player.level";
    pub const EXPERIENCE: &'static str = "minecraft:player.experience";
    pub const LUCK: &'static str = "minecraft:luck";
    pub const HORSE_JUMP_STRENGTH: &'static str = "minecraft:horse.jump_strength";

    pub fn new() -> Self {
        Self {
            map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn from_map(m: HashMap<String, EntityAttribute>) -> Self {
        let mut map = HashMap::new();
        for (name, attribute) in m {
            map.insert(name, Arc::new(RwLock::new(attribute)));
        }
        Self {
            map: Arc::new(RwLock::new(map)),
        }
    }

    pub fn insert(&self, key: String, value: EntityAttribute) {
        self.map.write().insert(key, Arc::new(RwLock::new(value)));
    }

    pub fn get(&self, key: &str) -> Option<EntityAttribute> {
        self.map.read().get(key).map(|a| *a.read())
    }

    pub fn get_mut(&self, key: &str) -> Option<ArcRwLockWriteGuard<RawRwLock, EntityAttribute>> {
        let attribute = self.map.read().get(key).cloned()?;
        Some(attribute.write_arc())
    }

    pub fn change(&self, key: &str, value: EntityAttribute) -> Option<EntityAttribute> {
        let mut attr = self.get_mut(key)?;
        let origin = *attr;
        *attr = value;
        Some(origin)
    }

    pub fn len(&self) -> usize {
        self.map.read().len()
    }

    pub fn iter(&self) -> std::vec::IntoIter<(String, Arc<RwLock<EntityAttribute>>)> {
        self.map
            .read()
            .iter()
            .map(|(name, attribute)| (name.clone(), attribute.clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EntityAttribute {
    pub min_value: f32,
    pub max_value: f32,
    pub default_min_value: f32,
    pub default_max_value: f32,
    pub default_value: f32,
    pub current_value: f32,
    pub should_send: bool,
}

impl<'de> Deserialize<'de> for EntityAttribute {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let map = Map::deserialize(deserializer)?;
        let min_value = map.get("min_value").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let max_value = map.get("max_value").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let default_min_value = map
            .get("default_min_value")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let default_max_value = map
            .get("default_max_value")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let default_value = map
            .get("default_value")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let should_send = map
            .get("should_send")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        Ok(Self {
            min_value,
            max_value,
            default_min_value,
            default_max_value,
            default_value,
            current_value: default_value,
            should_send,
        })
    }
}

impl EntityAttribute {
    pub fn to_nbt(&self, name: String) -> CompoundNbt {
        let mut compound = CompoundNbt::new(None);
        compound
            .insert("Name", NbtValue::String(name))
            .insert("current", NbtValue::Float(self.current_value))
            .insert("DefaultMax", NbtValue::Float(self.default_max_value))
            .insert("DefaultMin", NbtValue::Float(self.default_min_value))
            .insert("Max", NbtValue::Float(self.max_value))
            .insert("Min", NbtValue::Float(self.min_value));
        compound
    }
}
