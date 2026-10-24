//! 提供用于静态驻留（intern）不可变值的类型。
//! 驻留值去重相同值，比较按引用身份进行。
//!
//! Provides types used to statically intern immutable values.
//! Interned values are deduplicated and compared by reference identity.

use std::collections::HashSet;
use std::{
    fmt::Debug,
    hash::Hash,
    ops::Deref,
    sync::{OnceLock, PoisonError, RwLock},
};

/// 驻留值句柄：指向 `'static` 驻留值的轻量引用，可复制、可比较。
///
/// Interned handle: a lightweight, copyable, comparable reference to a
/// `'static` interned value.
pub struct Interned<T: ?Sized + 'static>(pub &'static T);

impl<T: ?Sized> Deref for Interned<T> {
    type Target = T;

    /// 解引用到被驻留的值。
    ///
    /// Dereferences to the interned value.
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<T: ?Sized> Clone for Interned<T> {
    /// 复制句柄（值本身不可变）。
    ///
    /// Clones the handle (the value itself is immutable).
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for Interned<T> {}

// Two `Interned<T>` should only be equal if they are clones from the same instance.
// Therefore, we only use the pointer to determine equality.
impl<T: ?Sized + Internable> PartialEq for Interned<T> {
    /// 比较两个句柄是否指向同一被驻留的值。
    ///
    /// Compares whether two handles point to the same interned value.
    fn eq(&self, other: &Self) -> bool {
        self.0.ref_eq(other.0)
    }
}

impl<T: ?Sized + Internable> Eq for Interned<T> {}

// Important: This must be kept in sync with the PartialEq/Eq implementation
impl<T: ?Sized + Internable> Hash for Interned<T> {
    /// 将被驻留值的身份送入哈希器。
    ///
    /// Feeds the identity of the interned value into the hasher.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.ref_hash(state);
    }
}

impl<T: ?Sized + Debug> Debug for Interned<T> {
    /// 以被驻留值自身的调试格式输出。
    ///
    /// Formats using the debug representation of the interned value itself.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<T> From<&Interned<T>> for Interned<T> {
    /// 从引用复制出句柄。
    ///
    /// Copies the handle out of a reference.
    fn from(value: &Interned<T>) -> Self {
        *value
    }
}

/// 可驻留值的 trait。
///
/// 本 trait 由 [`Interner<T>`] 使用，为被驻留的值创建静态引用。
///
/// A trait for internable values.
///
/// This is used by [`Interner<T>`] to create static references for values that are interned.
pub trait Internable: Hash + Eq {
    /// 创建指向 `self` 的静态引用，可能泄漏内存。
    ///
    /// Creates a static reference to `self`, possibly leaking memory.
    fn leak(&self) -> &'static Self;

    /// 判断两个引用是否指向同一个值，相同返回 `true`。
    ///
    /// Returns `true` if the two references point to the same value.
    fn ref_eq(&self, other: &Self) -> bool;

    /// 将引用送入哈希器。
    ///
    /// Feeds the reference to the hasher.
    fn ref_hash<H: std::hash::Hasher>(&self, state: &mut H);
}

impl Internable for str {
    /// 复制为装箱字符串并泄漏，得到 `'static` 引用。
    ///
    /// Copies into a boxed string and leaks it to obtain a `'static` reference.
    fn leak(&self) -> &'static Self {
        let str = self.to_owned().into_boxed_str();
        Box::leak(str)
    }

    /// 比较两个字符串引用是否指向同一段内存。
    ///
    /// Compares whether two string references point to the same memory.
    fn ref_eq(&self, other: &Self) -> bool {
        self.as_ptr() == other.as_ptr() && self.len() == other.len()
    }

    /// 把长度与指针地址送入哈希器。
    ///
    /// Feeds the length and pointer address into the hasher.
    fn ref_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        self.as_ptr().hash(state);
    }
}

/// 线程安全的驻留器：可由 `&T` 创建 [`Interned<T>`]。
/// 两个相等的值得到两个相等的 [`Interned<T>`]。
/// 使用 [`Interner<T>`] 要求 `T` 实现 [`Internable`]。
///
/// A thread-safe interner which can be used to create [`Interned<T>`] from
/// `&T`. Two equal values return two equal [`Interned<T>`] values.
/// To use an [`Interner<T>`], `T` must implement [`Internable`].
pub struct Interner<T: ?Sized + 'static>(OnceLock<RwLock<HashSet<&'static T>>>);

impl<T: ?Sized> Interner<T> {
    /// 创建一个新的空驻留器。
    ///
    /// Creates a new empty interner.
    pub const fn new() -> Self {
        Self(OnceLock::new())
    }
}

impl<T: Internable + ?Sized> Interner<T> {
    /// 返回 `value` 对应的 [`Interned<T>`]。
    ///
    /// 首次对 `value` 调用时可能泄漏该值，并用获得的静态引用返回
    /// [`Interned<T>`]；之后对相同 `value` 的调用将使用同一静态引用返回。
    ///
    /// Returns the [`Interned<T>`] corresponding to `value`.
    ///
    /// If it is called the first time for `value`, it will possibly leak the value and return an
    /// [`Interned<T>`] using the obtained static reference. Subsequent calls for the same `value`
    /// will return [`Interned<T>`] using the same static reference.
    ///
    /// - `value`: 要驻留的值
    pub fn intern(&self, value: &T) -> Interned<T> {
        let lock = self.0.get_or_init(Default::default);
        {
            let set = lock.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(value) = set.get(value) {
                return Interned(*value);
            }
        }
        {
            let mut set = lock.write().unwrap_or_else(PoisonError::into_inner);
            if let Some(value) = set.get(value) {
                Interned(*value)
            } else {
                let leaked = value.leak();
                set.insert(leaked);
                Interned(leaked)
            }
        }
    }
}

impl<T: ?Sized> Default for Interner<T> {
    /// 返回一个空的驻留器。
    ///
    /// Returns an empty interner.
    fn default() -> Self {
        Self::new()
    }
}
