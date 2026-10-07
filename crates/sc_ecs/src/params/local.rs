use crate::local_manager::{LocalId, LocalManager};
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;
use parking_lot::{MappedMutexGuard, MutexGuard};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

#[derive(Debug)]
pub struct Local<T: Default + Send + Sync + 'static>(pub(crate) MappedMutexGuard<'static, T>);

unsafe impl<T: Default + Send + Sync + 'static> Send for Local<T> {}
unsafe impl<T: Default + Send + Sync + 'static> Sync for Local<T> {}

impl<T: Default + Send + Sync + 'static> Deref for Local<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<T: Default + Send + Sync + 'static> DerefMut for Local<T> {
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

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

pub struct LocalState<T: Default + Send + Sync + 'static> {
    id: LocalId,
    phantom: PhantomData<T>,
}

impl<T: Default + Send + Sync + 'static> LocalState<T> {
    /// Get a reference to this `SyncCell`'s inner value.
    pub fn get(&mut self) -> Option<MappedMutexGuard<'static, T>> {
        MutexGuard::try_map(LocalManager::global().lock(), |manager| {
            manager.get_mut(self.id)
        })
        .ok()
    }

    /// For types that implement [`Sync`], get shared access to this `SyncCell`'s inner value.
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
    fn init() -> Self {
        let id = LocalManager::global().lock().push(T::default());
        Self {
            id,
            phantom: Default::default(),
        }
    }
    fn get_param<'a>(
        state: &'a mut Self,
        _world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        state.get().map(Local)
    }
}

unsafe impl<T: Default + Send + Sync + 'static> SystemParam for Local<T> {
    type This<'a> = Local<T>;
    type State = LocalState<T>;
}
