//! `Local<T>` 参数：每个系统实例独享的跨帧本地状态。
//!
//! `Local<T>` 绑定到全局 `LocalManager` 中的一个槽位，值在系统多次执行之间
//! 保留；不同系统的 `Local<T>` 互不影响。
//!
//! The `Local<T>` parameter: per-system-instance local state kept across runs.
//!
//! `Local<T>` is bound to a slot in the global `LocalManager` and its value is
//! preserved between system runs; `Local<T>` of different systems are
//! independent.
use crate::local_manager::{LocalId, LocalManager};
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;
use parking_lot::{MappedMutexGuard, MutexGuard};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

/// 系统本地状态：独占的可变访问句柄。
///
/// System-local state: an exclusive mutable access handle.
#[derive(Debug)]
pub struct Local<T: Default + Send + Sync + 'static>(
    /// 指向全局管理器中该槽位的映射守卫。
    ///
    /// The mapped guard pointing at this slot in the global manager.
    pub(crate) MappedMutexGuard<'static, T>,
);

unsafe impl<T: Default + Send + Sync + 'static> Send for Local<T> {}
unsafe impl<T: Default + Send + Sync + 'static> Sync for Local<T> {}

impl<T: Default + Send + Sync + 'static> Deref for Local<T> {
    type Target = T;

    /// 解引用到本地状态的值。
    ///
    /// Dereferences to the local state value.
    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<T: Default + Send + Sync + 'static> DerefMut for Local<T> {
    /// 可变解引用到本地状态的值。
    ///
    /// Mutably dereferences to the local state value.
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a, T: Default + Send + Sync + 'static> IntoIterator for &'a Local<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;

    /// 以引用的迭代器消费本地状态。
    ///
    /// Consumes the local state with the iterator of the reference.
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T: Default + Send + Sync + 'static> IntoIterator for &'a mut Local<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;

    /// 以可变引用的迭代器消费本地状态。
    ///
    /// Consumes the local state with the iterator of the mutable reference.
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// `Local<T>` 的参数状态：记录该系统实例在全局管理器中的槽位。
///
/// The parameter state of `Local<T>`: records the slot this system instance
/// occupies in the global manager.
pub struct LocalState<T: Default + Send + Sync + 'static> {
    /// 该实例在全局管理器中的槽位 id。
    ///
    /// The slot id of this instance in the global manager.
    id: LocalId,
    /// 状态类型占位。
    ///
    /// Phantom data for the state type.
    phantom: PhantomData<T>,
}

impl<T: Default + Send + Sync + 'static> LocalState<T> {
    /// 获取该槽位内部值的可变访问句柄；全局管理器被占用时返回 `None`。
    ///
    /// Gets a mutable access handle to the inner value of this slot; returns
    /// `None` when the global manager is locked.
    pub fn get(&mut self) -> Option<MappedMutexGuard<'static, T>> {
        MutexGuard::try_map(LocalManager::global().lock(), |manager| {
            manager.get_mut(self.id)
        })
        .ok()
    }

    /// 对实现了 [`Sync`] 的类型，获取该槽位内部值的共享访问句柄；全局管理器被
    /// 占用时返回 `None`。
    ///
    /// For types that implement [`Sync`], gets shared access to the inner value
    /// of this slot; returns `None` when the global manager is locked.
    pub fn read(&self) -> Option<MappedMutexGuard<'static, T>>
    where
        T: Sync,
    {
        MutexGuard::try_map(LocalManager::global().lock(), |manager| {
            manager.get_mut(self.id)
        })
        .ok()
    }
}

impl<'s, T: Default + Send + Sync + 'static> SystemParamState for LocalState<T> {
    type Item = Local<T>;

    /// 初始化参数状态：在全局管理器中为该实例分配一个新槽位。
    ///
    /// Initializes the parameter state: allocates a fresh slot for this
    /// instance in the global manager.
    fn init() -> Self {
        let id = LocalManager::global().lock().push(T::default());
        Self {
            id,
            phantom: Default::default(),
        }
    }

    /// 提取本次执行的本地状态句柄；槽位不可访问时返回 `None`。
    ///
    /// Extracts the local state handle for this run; returns `None` when the
    /// slot is inaccessible.
    fn get_param<'a>(
        state: &'a mut Self,
        _world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        state.get().map(Local)
    }
}

/// `Local<T>` 可作为系统参数注入。
///
/// `Local<T>` can be injected as a system parameter.
unsafe impl<T: Default + Send + Sync + 'static> SystemParam for Local<T> {
    type This<'a> = Local<T>;
    type State = LocalState<T>;
}
