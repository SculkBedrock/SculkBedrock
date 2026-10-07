use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;

pub trait EntityPropertyTrait {
    fn write(&self, nbt: &mut CompoundNbt);
}

pub struct EntityProperty<P: EntityPropertyTrait> {
    pub identifier: String,
    property: P,
}

impl<P: EntityPropertyTrait> EntityProperty<P> {
    pub fn new(identifier: String, property: P) -> Self {
        Self {
            identifier,
            property,
        }
    }

    pub fn write(&self, nbt: &mut CompoundNbt) {
        self.property.write(nbt);
    }
}

pub struct IntEntityProperty {
    pub max_value: i32,
    pub min_value: i32,
}

impl EntityPropertyTrait for IntEntityProperty {
    fn write(&self, nbt: &mut CompoundNbt) {
        nbt.insert("type", NbtValue::Int(0))
            .insert("max", NbtValue::Int(self.max_value))
            .insert("min", NbtValue::Int(self.min_value));
    }
}

pub struct FloatEntityProperty {
    pub max_value: f32,
    pub min_value: f32,
}

impl EntityPropertyTrait for FloatEntityProperty {
    fn write(&self, nbt: &mut CompoundNbt) {
        nbt.insert("type", NbtValue::Int(1))
            .insert("max", NbtValue::Float(self.max_value))
            .insert("min", NbtValue::Float(self.min_value));
    }
}

pub struct EnumEntityProperty {
    pub enums: Vec<String>,
}

impl EntityPropertyTrait for EnumEntityProperty {
    fn write(&self, nbt: &mut CompoundNbt) {
        nbt.insert("type", NbtValue::Int(3));
        let mut enum_list = Vec::new();
        for i in &self.enums {
            enum_list.push(NbtValue::String(i.clone()));
        }
        nbt.insert("enum", NbtValue::List(enum_list));
    }
}
