//! 动态相等与哈希 trait：为 `dyn` 对象提供可比较、可哈希的能力。
//!
//! `DynEq`/`DynHash` 让异构容器（如 `HashMap<dyn DynEq, V>`）能够按值比较
//! 与哈希，而无需知道具体类型。
//!
//! Dynamic equality and hashing traits: giving `dyn` objects the ability to
//! be compared and hashed.
//!
//! `DynEq`/`DynHash` let heterogeneous containers (e.g. `HashMap<dyn DynEq, V>`)
//! compare and hash by value without knowing the concrete type.
use std::any::Any;
use std::hash::{Hash, Hasher};

/// 动态相等性 trait：为 `dyn` 对象提供可比较相等性的能力。
///
/// DynEq: a trait giving `dyn` objects the ability to compare for equality.
pub trait DynEq: Any {
    /// 将类型转换为 `dyn Any`。
    ///
    /// Casts the type to `dyn Any`.
    fn as_any(&self) -> &dyn Any;

    /// 测试 `self` 与 `other` 是否相等。
    ///
    /// 实现者应避免在底层类型不同时返回 `true`。
    ///
    /// Tests for `self` and `other` values to be equal.
    ///
    /// Implementers should avoid returning `true` when the underlying types are
    /// not the same.
    fn dyn_eq(&self, other: &dyn DynEq) -> bool;
}

impl<T> DynEq for T
where
    T: Any + Eq,
{
    /// 返回自身的 `dyn Any` 视图。
    ///
    /// Returns the `dyn Any` view of self.
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// 在底层类型一致时按值比较，否则返回 `false`。
    ///
    /// Compares by value when the underlying types match, otherwise returns
    /// `false`.
    fn dyn_eq(&self, other: &dyn DynEq) -> bool {
        if let Some(other) = other.as_any().downcast_ref::<T>() {
            return self == other;
        }
        false
    }
}

/// 动态哈希 trait：为 `dyn` 对象提供可哈希的能力。
///
/// DynHash: a trait giving `dyn` objects the ability to be hashed.
pub trait DynHash: DynEq {
    /// 将类型转换为 `dyn DynEq`。
    ///
    /// Casts the type to `dyn DynEq`.
    fn as_dyn_eq(&self) -> &dyn DynEq;

    /// 将本值送入给定的 [`Hasher`]。
    ///
    /// Feeds this value into the given [`Hasher`].
    fn dyn_hash(&self, state: &mut dyn Hasher);
}

impl<T> DynHash for T
where
    T: DynEq + Hash,
{
    /// 返回自身的 `dyn DynEq` 视图。
    ///
    /// Returns the `dyn DynEq` view of self.
    fn as_dyn_eq(&self) -> &dyn DynEq {
        self
    }

    /// 把值的哈希与类型 id 一并送入哈希器，使不同类型不会互相碰撞。
    ///
    /// Feeds the value hash and the type id into the hasher so that values of
    /// different types never collide.
    fn dyn_hash(&self, mut state: &mut dyn Hasher) {
        T::hash(self, &mut state);
        self.type_id().hash(&mut state);
    }
}
