//! 实体属性（properties）的 NBT 写入模型。
//!
//! NBT-writing model for entity properties.

use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;

/// 实体属性的 NBT 写入接口。
///
/// NBT-writing interface for entity properties.
pub trait EntityPropertyTrait {
    /// 将属性写入指定 NBT 复合体。
    ///
    /// Writes the property into the given NBT compound.
    ///
    /// - `nbt`: 目标 NBT 复合体
    fn write(&self, nbt: &mut CompoundNbt);
}

/// 带标识名的实体属性：标识 + 具体属性数据。
///
/// Named entity property: identifier plus concrete property data.
pub struct EntityProperty<P: EntityPropertyTrait> {
    /// 属性标识名。
    ///
    /// Property identifier.
    pub identifier: String,
    /// 具体属性数据。
    ///
    /// Concrete property data.
    property: P,
}

impl<P: EntityPropertyTrait> EntityProperty<P> {
    /// 用标识名与属性数据构造实体属性。
    ///
    /// Constructs an entity property with an identifier and property data.
    ///
    /// - `identifier`: 属性标识名
    /// - `property`: 具体属性数据
    pub fn new(identifier: String, property: P) -> Self {
        Self {
            identifier,
            property,
        }
    }

    /// 将底层属性写入指定 NBT 复合体。
    ///
    /// Writes the underlying property into the given NBT compound.
    ///
    /// - `nbt`: 目标 NBT 复合体
    pub fn write(&self, nbt: &mut CompoundNbt) {
        self.property.write(nbt);
    }
}

/// 整型实体属性（带取值范围）。
///
/// Integer entity property (with a value range).
pub struct IntEntityProperty {
    /// 最大值。
    ///
    /// Maximum value.
    pub max_value: i32,
    /// 最小值。
    ///
    /// Minimum value.
    pub min_value: i32,
}

impl EntityPropertyTrait for IntEntityProperty {
    /// 以整型属性格式（type=0）写入 NBT。
    ///
    /// Writes the property as an int property (type=0).
    fn write(&self, nbt: &mut CompoundNbt) {
        nbt.insert("type", NbtValue::Int(0))
            .insert("max", NbtValue::Int(self.max_value))
            .insert("min", NbtValue::Int(self.min_value));
    }
}

/// 浮点型实体属性（带取值范围）。
///
/// Float entity property (with a value range).
pub struct FloatEntityProperty {
    /// 最大值。
    ///
    /// Maximum value.
    pub max_value: f32,
    /// 最小值。
    ///
    /// Minimum value.
    pub min_value: f32,
}

impl EntityPropertyTrait for FloatEntityProperty {
    /// 以浮点属性格式（type=1）写入 NBT。
    ///
    /// Writes the property as a float property (type=1).
    fn write(&self, nbt: &mut CompoundNbt) {
        nbt.insert("type", NbtValue::Int(1))
            .insert("max", NbtValue::Float(self.max_value))
            .insert("min", NbtValue::Float(self.min_value));
    }
}

/// 枚举型实体属性（候选值列表）。
///
/// Enum entity property (list of candidate values).
pub struct EnumEntityProperty {
    /// 候选值列表。
    ///
    /// Candidate values.
    pub enums: Vec<String>,
}

impl EntityPropertyTrait for EnumEntityProperty {
    /// 以枚举属性格式（type=3）写入 NBT。
    ///
    /// Writes the property as an enum property (type=3).
    fn write(&self, nbt: &mut CompoundNbt) {
        nbt.insert("type", NbtValue::Int(3));
        let mut enum_list = Vec::new();
        for i in &self.enums {
            enum_list.push(NbtValue::String(i.clone()));
        }
        nbt.insert("enum", NbtValue::List(enum_list));
    }
}
