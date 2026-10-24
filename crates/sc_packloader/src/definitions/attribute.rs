//! 实体属性（attribute）定义：默认值资源、每实体属性组件与 NBT 序列化。
//!
//! Entity attribute definitions: default-value resource, per-entity
//! attribute component, and NBT serialization.

use parking_lot::{ArcRwLockWriteGuard, RawRwLock, RwLock};
use sc_ecs::component::Component;
use sc_ecs::resource::Resource;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use serde::Deserialize;
use serde_json::Map;
use std::collections::HashMap;
use std::sync::Arc;

/// 默认实体属性表（ECS 资源）：属性名 → 属性定义。
///
/// Default entity attribute table (ECS resource): name → attribute definition.
#[derive(Resource, Debug)]
pub struct DefaultEntityAttributes {
    /// 属性名到属性定义的映射。
    ///
    /// Map from attribute name to attribute definition.
    map: HashMap<String, EntityAttribute>,
}

impl DefaultEntityAttributes {
    /// 从映射构造默认属性表。
    ///
    /// Builds the default attribute table from a map.
    ///
    /// - `map`: 属性名到属性定义的映射
    pub fn new(map: HashMap<String, EntityAttribute>) -> Self {
        Self { map }
    }

    /// 复制一份默认属性表为每实体属性组件。
    ///
    /// Clones the default table into a per-entity attribute component.
    pub fn default(&self) -> EntityAttributes {
        EntityAttributes::from_map(self.map.clone())
    }
}

/// 每实体属性组件：属性名 → 可写锁包裹的属性值。
///
/// Per-entity attribute component: name → write-locked attribute value.
#[derive(Component, Debug, Clone)]
pub struct EntityAttributes {
    /// 属性名到共享可写属性的映射。
    ///
    /// Map from attribute name to shared writable attribute.
    map: Arc<RwLock<HashMap<String, Arc<RwLock<EntityAttribute>>>>>,
}

impl EntityAttributes {
    /// 伤害吸收属性键。
    ///
    /// Absorption attribute key.
    pub const ABSORPTION: &'static str = "minecraft:absorption";
    /// 饱和度属性键。
    ///
    /// Saturation attribute key.
    pub const SATURATION: &'static str = "minecraft:player.saturation";
    /// 饥饿度消耗属性键。
    ///
    /// Exhaustion attribute key.
    pub const EXHAUSTION: &'static str = "minecraft:player.exhaustion";
    /// 击退抗性属性键。
    ///
    /// Knockback resistance attribute key.
    pub const KNOCKBACK_RESISTANCE: &'static str = "minecraft:knockback_resistance";
    /// 最大生命值属性键。
    ///
    /// Max health attribute key.
    pub const MAX_HEALTH: &'static str = "minecraft:health";
    /// 移动速度属性键。
    ///
    /// Movement speed attribute key.
    pub const MOVEMENT_SPEED: &'static str = "minecraft:movement";
    /// 飞行速度属性键。
    ///
    /// Flight speed attribute key.
    pub const FLIGHT_SPEED: &'static str = "sc:flight";
    /// 水下移动速度属性键。
    ///
    /// Underwater movement speed attribute key.
    pub const UNDER_WATER_MOVEMENT_SPEED: &'static str = "minecraft:underwater_movement";
    /// 岩浆中移动速度属性键。
    ///
    /// Lava movement speed attribute key.
    pub const LAVA_MOVEMENT_SPEED: &'static str = "minecraft:lava_movement";
    /// 追踪范围属性键。
    ///
    /// Follow range attribute key.
    pub const FOLLOW_RANGE: &'static str = "minecraft:follow_range";
    /// 最大饥饿值属性键。
    ///
    /// Max hunger attribute key.
    pub const MAX_HUNGER: &'static str = "minecraft:player.hunger";
    /// 攻击伤害属性键。
    ///
    /// Attack damage attribute key.
    pub const ATTACK_DAMAGE: &'static str = "minecraft:attack_damage";
    /// 经验等级属性键。
    ///
    /// Experience level attribute key.
    pub const EXPERIENCE_LEVEL: &'static str = "minecraft:player.level";
    /// 经验值属性键。
    ///
    /// Experience attribute key.
    pub const EXPERIENCE: &'static str = "minecraft:player.experience";
    /// 幸运值属性键。
    ///
    /// Luck attribute key.
    pub const LUCK: &'static str = "minecraft:luck";
    /// 马跳跃力量属性键。
    ///
    /// Horse jump strength attribute key.
    pub const HORSE_JUMP_STRENGTH: &'static str = "minecraft:horse.jump_strength";
    /// 玩家氧气属性键。
    ///
    /// Player air supply attribute key.
    pub const AIR: &'static str = "sc:player.air";

    /// 构造空属性组件。
    ///
    /// Creates an empty attribute component.
    pub fn new() -> Self {
        Self {
            map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 从映射构造属性组件（每个值套独立写锁）。
    ///
    /// Builds an attribute component from a map (each value under its own
    /// write lock).
    ///
    /// - `m`: 属性名到属性定义的映射
    pub fn from_map(m: HashMap<String, EntityAttribute>) -> Self {
        let mut map = HashMap::new();
        for (name, attribute) in m {
            map.insert(name, Arc::new(RwLock::new(attribute)));
        }
        Self {
            map: Arc::new(RwLock::new(map)),
        }
    }

    /// 插入/覆盖一个属性。
    ///
    /// Inserts or overwrites an attribute.
    ///
    /// - `key`: 属性名
    /// - `value`: 属性定义
    pub fn insert(&self, key: String, value: EntityAttribute) {
        self.map.write().insert(key, Arc::new(RwLock::new(value)));
    }

    /// 读取属性副本；不存在返回 `None`。
    ///
    /// Reads a copy of an attribute; `None` when absent.
    ///
    /// - `key`: 属性名
    pub fn get(&self, key: &str) -> Option<EntityAttribute> {
        self.map.read().get(key).map(|a| *a.read())
    }

    /// 获取属性的写锁守卫；不存在返回 `None`。
    ///
    /// Takes a write-lock guard for an attribute; `None` when absent.
    ///
    /// - `key`: 属性名
    pub fn get_mut(&self, key: &str) -> Option<ArcRwLockWriteGuard<RawRwLock, EntityAttribute>> {
        let attribute = self.map.read().get(key).cloned()?;
        Some(attribute.write_arc())
    }

    /// 替换属性值并返回旧值；不存在返回 `None`。
    ///
    /// Replaces an attribute value and returns the old one; `None` when absent.
    ///
    /// - `key`: 属性名
    /// - `value`: 新的属性定义
    pub fn change(&self, key: &str, value: EntityAttribute) -> Option<EntityAttribute> {
        let mut attr = self.get_mut(key)?;
        let origin = *attr;
        *attr = value;
        Some(origin)
    }

    /// 属性数量。
    ///
    /// Number of attributes.
    pub fn len(&self) -> usize {
        self.map.read().len()
    }

    /// 遍历全部属性（副本，顺序不定）。
    ///
    /// Iterates all attributes (copies, unordered).
    pub fn iter(&self) -> std::vec::IntoIter<(String, Arc<RwLock<EntityAttribute>>)> {
        self.map
            .read()
            .iter()
            .map(|(name, attribute)| (name.clone(), attribute.clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

/// 单个实体属性：取值范围、默认值与当前值。
///
/// A single entity attribute: value range, defaults, and current value.
#[derive(Debug, Clone, Copy)]
pub struct EntityAttribute {
    /// 最小值。
    ///
    /// Minimum value.
    pub min_value: f32,
    /// 最大值。
    ///
    /// Maximum value.
    pub max_value: f32,
    /// 默认最小值。
    ///
    /// Default minimum value.
    pub default_min_value: f32,
    /// 默认最大值。
    ///
    /// Default maximum value.
    pub default_max_value: f32,
    /// 默认值。
    ///
    /// Default value.
    pub default_value: f32,
    /// 当前值（反序列化时初始化为默认值）。
    ///
    /// Current value (initialized to the default when deserialized).
    pub current_value: f32,
    /// 是否需要同步给客户端。
    ///
    /// Whether the attribute is sent to clients.
    pub should_send: bool,
}

impl<'de> Deserialize<'de> for EntityAttribute {
    /// 从 JSON 对象反序列化属性；缺失字段取 0（`should_send` 取 true），
    /// 当前值初始化为默认值。
    ///
    /// Deserializes an attribute from a JSON object; missing fields default
    /// to 0 (`should_send` to true), and the current value starts at the
    /// default.
    ///
    /// - `deserializer`: serde 反序列化器
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
    /// 序列化为属性 NBT（Name/current/Min/Max/DefaultMin/DefaultMax）。
    ///
    /// Serializes into an attribute NBT (Name/current/Min/Max/DefaultMin/DefaultMax).
    ///
    /// - `name`: 属性名
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
