//! 物品 NBT 切片（Facet）系统。
//!
//! 提供结构化、强类型、命名空间隔离的物品 NBT 读写机制。

use crate::compound::CompoundNbt;
use crate::NbtValue;

/// 从 NBT 值转换为 Rust 类型的转换 Trait。
pub trait FromNbt: Sized {
    fn from_nbt(value: &NbtValue) -> Option<Self>;
}

/// 从 Rust 类型转换为 NBT 值的转换 Trait。
pub trait ToNbt {
    fn to_nbt(&self) -> NbtValue;
}

macro_rules! impl_from_to_primitive {
    ($ty:ty, $variant:ident, $as_fn:ident) => {
        impl FromNbt for $ty {
            #[inline]
            fn from_nbt(value: &NbtValue) -> Option<Self> {
                value.$as_fn()
            }
        }
        impl ToNbt for $ty {
            #[inline]
            fn to_nbt(&self) -> NbtValue {
                NbtValue::$variant(*self)
            }
        }
        impl From<$ty> for NbtValue {
            #[inline]
            fn from(v: $ty) -> Self {
                NbtValue::$variant(v)
            }
        }
        impl NbtField for $ty {
            #[inline]
            fn from_field(val: Option<&NbtValue>) -> Option<Self> {
                val.and_then(FromNbt::from_nbt)
            }
            #[inline]
            fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
                compound.insert(key, self.to_nbt());
            }
        }
    };
}

impl_from_to_primitive!(i8, Byte, as_i8);
impl_from_to_primitive!(i16, Short, as_i16);
impl_from_to_primitive!(i32, Int, as_i32);
impl_from_to_primitive!(i64, Long, as_i64);
impl_from_to_primitive!(f32, Float, as_f32);
impl_from_to_primitive!(f64, Double, as_f64);

impl FromNbt for bool {
    #[inline]
    fn from_nbt(value: &NbtValue) -> Option<Self> {
        value.as_bool()
    }
}

impl ToNbt for bool {
    #[inline]
    fn to_nbt(&self) -> NbtValue {
        NbtValue::Byte(if *self { 1 } else { 0 })
    }
}

impl From<bool> for NbtValue {
    #[inline]
    fn from(v: bool) -> Self {
        NbtValue::Byte(if v { 1 } else { 0 })
    }
}

impl NbtField for bool {
    #[inline]
    fn from_field(val: Option<&NbtValue>) -> Option<Self> {
        val.and_then(FromNbt::from_nbt)
    }
    #[inline]
    fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
        compound.insert(key, self.to_nbt());
    }
}

impl FromNbt for u32 {
    #[inline]
    fn from_nbt(value: &NbtValue) -> Option<Self> {
        value.as_i32().map(|v| v as u32)
    }
}

impl ToNbt for u32 {
    #[inline]
    fn to_nbt(&self) -> NbtValue {
        NbtValue::Int(*self as i32)
    }
}

impl From<u32> for NbtValue {
    #[inline]
    fn from(v: u32) -> Self {
        NbtValue::Int(v as i32)
    }
}

impl NbtField for u32 {
    #[inline]
    fn from_field(val: Option<&NbtValue>) -> Option<Self> {
        val.and_then(FromNbt::from_nbt)
    }
    #[inline]
    fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
        compound.insert(key, self.to_nbt());
    }
}

impl FromNbt for u16 {
    #[inline]
    fn from_nbt(value: &NbtValue) -> Option<Self> {
        value.as_i16().map(|v| v as u16)
    }
}

impl ToNbt for u16 {
    #[inline]
    fn to_nbt(&self) -> NbtValue {
        NbtValue::Short(*self as i16)
    }
}

impl From<u16> for NbtValue {
    #[inline]
    fn from(v: u16) -> Self {
        NbtValue::Short(v as i16)
    }
}

impl NbtField for u16 {
    #[inline]
    fn from_field(val: Option<&NbtValue>) -> Option<Self> {
        val.and_then(FromNbt::from_nbt)
    }
    #[inline]
    fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
        compound.insert(key, self.to_nbt());
    }
}

impl FromNbt for u8 {
    #[inline]
    fn from_nbt(value: &NbtValue) -> Option<Self> {
        value.as_i8().map(|v| v as u8)
    }
}

impl ToNbt for u8 {
    #[inline]
    fn to_nbt(&self) -> NbtValue {
        NbtValue::Byte(*self as i8)
    }
}

impl From<u8> for NbtValue {
    #[inline]
    fn from(v: u8) -> Self {
        NbtValue::Byte(v as i8)
    }
}

impl NbtField for u8 {
    #[inline]
    fn from_field(val: Option<&NbtValue>) -> Option<Self> {
        val.and_then(FromNbt::from_nbt)
    }
    #[inline]
    fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
        compound.insert(key, self.to_nbt());
    }
}

impl FromNbt for String {
    #[inline]
    fn from_nbt(value: &NbtValue) -> Option<Self> {
        value.as_string().cloned()
    }
}

impl ToNbt for String {
    #[inline]
    fn to_nbt(&self) -> NbtValue {
        NbtValue::String(self.clone())
    }
}

impl From<String> for NbtValue {
    #[inline]
    fn from(v: String) -> Self {
        NbtValue::String(v)
    }
}

impl NbtField for String {
    #[inline]
    fn from_field(val: Option<&NbtValue>) -> Option<Self> {
        val.and_then(FromNbt::from_nbt)
    }
    #[inline]
    fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
        compound.insert(key, self.to_nbt());
    }
}

impl ToNbt for &str {
    #[inline]
    fn to_nbt(&self) -> NbtValue {
        NbtValue::String((*self).to_string())
    }
}

impl From<&str> for NbtValue {
    #[inline]
    fn from(v: &str) -> Self {
        NbtValue::String(v.to_string())
    }
}

impl FromNbt for CompoundNbt {
    #[inline]
    fn from_nbt(value: &NbtValue) -> Option<Self> {
        value.as_compound().cloned()
    }
}

impl ToNbt for CompoundNbt {
    #[inline]
    fn to_nbt(&self) -> NbtValue {
        NbtValue::Compound(self.clone())
    }
}

impl From<CompoundNbt> for NbtValue {
    #[inline]
    fn from(v: CompoundNbt) -> Self {
        NbtValue::Compound(v)
    }
}

impl NbtField for CompoundNbt {
    #[inline]
    fn from_field(val: Option<&NbtValue>) -> Option<Self> {
        val.and_then(FromNbt::from_nbt)
    }
    #[inline]
    fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
        compound.insert(key, self.to_nbt());
    }
}

impl FromNbt for NbtValue {
    #[inline]
    fn from_nbt(value: &NbtValue) -> Option<Self> {
        Some(value.clone())
    }
}

impl ToNbt for NbtValue {
    #[inline]
    fn to_nbt(&self) -> NbtValue {
        self.clone()
    }
}

impl NbtField for NbtValue {
    #[inline]
    fn from_field(val: Option<&NbtValue>) -> Option<Self> {
        val.cloned()
    }
    #[inline]
    fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
        compound.insert(key, self.clone());
    }
}

impl<T: FromNbt> FromNbt for Vec<T> {
    fn from_nbt(value: &NbtValue) -> Option<Self> {
        let list = value.as_list()?;
        list.iter().map(T::from_nbt).collect()
    }
}

impl<T: ToNbt> ToNbt for Vec<T> {
    fn to_nbt(&self) -> NbtValue {
        NbtValue::List(self.iter().map(T::to_nbt).collect())
    }
}

impl From<Vec<NbtValue>> for NbtValue {
    #[inline]
    fn from(v: Vec<NbtValue>) -> Self {
        NbtValue::List(v)
    }
}

/// NBT 复合标签内的字段存取规范。
pub trait NbtField: Sized {
    fn from_field(val: Option<&NbtValue>) -> Option<Self>;
    fn write_field(&self, key: &str, compound: &mut CompoundNbt);
}

impl<T: NbtField> NbtField for Option<T> {
    #[inline]
    fn from_field(val: Option<&NbtValue>) -> Option<Self> {
        Some(T::from_field(val))
    }

    #[inline]
    fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
        match self {
            Some(v) => v.write_field(key, compound),
            None => {
                compound.remove(key);
            }
        }
    }
}

impl<T: FromNbt + ToNbt> NbtField for Vec<T> {
    #[inline]
    fn from_field(val: Option<&NbtValue>) -> Option<Self> {
        Some(val.and_then(FromNbt::from_nbt).unwrap_or_default())
    }

    #[inline]
    fn write_field(&self, key: &str, compound: &mut CompoundNbt) {
        if self.is_empty() {
            compound.remove(key);
        } else {
            compound.insert(key, self.to_nbt());
        }
    }
}

/// NBT 切片（Facet）特征：可独立向根 `CompoundNbt` 读写或抹除。
pub trait NbtFacet: Sized {
    fn read_from(root: &CompoundNbt) -> Option<Self>;
    fn write_to(&self, root: &mut CompoundNbt);
    fn remove_from(root: &mut CompoundNbt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_nbt;
    use sc_nbt_macros::NbtFacet;

    #[derive(Clone, Debug, PartialEq, Eq, NbtFacet)]
    struct DamageFacet {
        #[nbt(rename = "Damage")]
        damage: i32,
    }

    #[derive(Clone, Debug, PartialEq, Eq, NbtFacet)]
    #[nbt(tag = "display")]
    struct DisplayFacet {
        #[nbt(rename = "Name")]
        name: Option<String>,
        #[nbt(rename = "Lore")]
        lore: Vec<String>,
    }

    #[test]
    fn item_nbt_macro_constructs_compound() {
        let nbt = item_nbt! {
            "Damage" => 15i32,
            "display" => {
                "Name" => "Excalibur",
                "Lore" => ["line1", "line2"],
            }
        };
        assert_eq!(nbt.get("Damage").unwrap().as_i32(), Some(15));
        let display = nbt.get("display").unwrap().as_compound().unwrap();
        assert_eq!(display.get("Name").unwrap().as_string().unwrap(), "Excalibur");
        let lore = display.get("Lore").unwrap().as_list().unwrap();
        assert_eq!(lore.len(), 2);
    }

    #[test]
    fn nbt_facet_read_write_round_trip() {
        let mut root = CompoundNbt::new(None);
        let dmg = DamageFacet { damage: 42 };
        dmg.write_to(&mut root);

        let disp = DisplayFacet {
            name: Some("Sword of Truth".into()),
            lore: vec!["Ancient".into(), "Sharp".into()],
        };
        disp.write_to(&mut root);

        // Read facets back
        let read_dmg = DamageFacet::read_from(&root).expect("read damage");
        assert_eq!(read_dmg, dmg);

        let read_disp = DisplayFacet::read_from(&root).expect("read display");
        assert_eq!(read_disp, disp);

        // Remove damage facet
        DamageFacet::remove_from(&mut root);
        assert!(DamageFacet::read_from(&root).is_none());
        // Display facet still intact
        assert!(DisplayFacet::read_from(&root).is_some());
    }
}
