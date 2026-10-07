//! Provides types used to statically intern immutable values.
//!
//! Interning is a pattern used to save memory by deduplicating identical values,
//! speed up code by shrinking the stack size of large types,
//! and make comparisons for any type as fast as integers.

use std::collections::HashSet;
use std::{
    fmt::Debug,
    hash::Hash,
    ops::Deref,
    sync::{OnceLock, PoisonError, RwLock},
};

pub struct Interned<T: ?Sized + 'static>(pub &'static T);

impl<T: ?Sized> Deref for Interned<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<T: ?Sized> Clone for Interned<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for Interned<T> {}

// Two Interned<T> should only be equal if they are clones from the same instance.
// Therefore, we only use the pointer to determine equality.
impl<T: ?Sized + Internable> PartialEq for Interned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.ref_eq(other.0)
    }
}

impl<T: ?Sized + Internable> Eq for Interned<T> {}

// Important: This must be kept in sync with the PartialEq/Eq implementation
impl<T: ?Sized + Internable> Hash for Interned<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.ref_hash(state);
    }
}

impl<T: ?Sized + Debug> Debug for Interned<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<T> From<&Interned<T>> for Interned<T> {
    fn from(value: &Interned<T>) -> Self {
        *value
    }
}

/// A trait for internable values.
///
/// This is used by [`Interner<T>`] to create static references for values that are interned.
pub trait Internable: Hash + Eq {
    /// Creates a static reference to `self`, possibly leaking memory.
    fn leak(&self) -> &'static Self;

    /// Returns `true` if the two references point to the same value.
    fn ref_eq(&self, other: &Self) -> bool;

    /// Feeds the reference to the hasher.
    fn ref_hash<H: std::hash::Hasher>(&self, state: &mut H);
}

impl Internable for str {
    fn leak(&self) -> &'static Self {
        let str = self.to_owned().into_boxed_str();
        Box::leak(str)
    }

    fn ref_eq(&self, other: &Self) -> bool {
        self.as_ptr() == other.as_ptr() && self.len() == other.len()
    }

    fn ref_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        self.as_ptr().hash(state);
    }
}

/// A thread-safe interner which can be used to create [`Interned<T>`] from `&T`
///
/// For details on interning, see [the module level docs](self).
///
/// The implementation ensures that two equal values return two equal [`Interned<T>`] values.
///
/// To use an [`Interner<T>`], `T` must implement [`Internable`].
pub struct Interner<T: ?Sized + 'static>(OnceLock<RwLock<HashSet<&'static T>>>);

impl<T: ?Sized> Interner<T> {
    /// Creates a new empty interner
    pub const fn new() -> Self {
        Self(OnceLock::new())
    }
}

impl<T: Internable + ?Sized> Interner<T> {
    /// Return the [`Interned<T>`] corresponding to `value`.
    ///
    /// If it is called the first time for `value`, it will possibly leak the value and return an
    /// [`Interned<T>`] using the obtained static reference. Subsequent calls for the same `value`
    /// will return [`Interned<T>`] using the same static reference.
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
    fn default() -> Self {
        Self::new()
    }
}
