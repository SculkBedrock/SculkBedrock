use crate::resource::Resource;
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;
use parking_lot::{ArcRwLockReadGuard, ArcRwLockWriteGuard, RawRwLock};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use sc_log::t_log;

pub enum ResourceError {
    NotFound(String),
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "Resource({}) not found", name),
        }
    }
}

impl Debug for ResourceError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "Resource({}) not found", name),
        }
    }
}

impl std::error::Error for ResourceError {}

/// Shared read handle: an owned read guard (`read_arc_recursive`, so
/// re-reading the same resource on one thread never deadlocks against a
/// queued writer). May be held across await; writers wait while it is held, so
/// avoid holding `Res` of hot resources in async tasks for long.
pub struct Res<T: 'static + Resource + Send + Sync> {
    pub(crate) guard: ArcRwLockReadGuard<RawRwLock, T>,
}

impl<T: 'static + Resource + Send + Sync> Res<T> {
    pub(crate) fn new(guard: ArcRwLockReadGuard<RawRwLock, T>) -> Self {
        Self { guard }
    }
}

impl<T: 'static + Resource + Send + Sync> Deref for Res<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

pub struct ParamResState<T> {
    _phantom: PhantomData<T>,
}

impl<T: 'static + Resource + Send + Sync> SystemParamState for ParamResState<T> {
    type Item = Res<T>;

    fn init() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }

    fn get_param<'a>(
        _state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        let resource = world.get_resource();
        if resource.is_none() {
            log::error!(
                "{}",
                t_log!("console.ecs.skipped_resource", name = T::name())
            );
        }
        resource
    }
}

unsafe impl<T: 'static + Resource + Send + Sync> SystemParam for Res<T> {
    type This<'a> = Res<T>;
    type State = ParamResState<T>;
}

/// Exclusive write handle: an owned write guard. All reads and writes of this
/// resource wait while it is held. Declaring `Res` + `ResMut` for the same
/// resource in one system (or re-fetching it through `World` while holding
/// `ResMut`) deadlocks by design.
pub struct ResMut<T: 'static + Resource + Send + Sync> {
    pub(crate) guard: ArcRwLockWriteGuard<RawRwLock, T>,
}

impl<T: 'static + Resource + Send + Sync> ResMut<T> {
    pub(crate) fn new(guard: ArcRwLockWriteGuard<RawRwLock, T>) -> Self {
        Self { guard }
    }
}

impl<T: 'static + Resource + Send + Sync> Deref for ResMut<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<T: 'static + Resource + Send + Sync> DerefMut for ResMut<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

pub struct ParamResMutState<T> {
    _phantom: PhantomData<T>,
}

impl<T: 'static + Resource + Send + Sync> SystemParamState for ParamResMutState<T> {
    type Item = ResMut<T>;

    fn init() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }

    fn get_param<'a>(
        _state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        let resource = world.get_resource_mut();
        if resource.is_none() {
            log::error!(
                "{}",
                t_log!("console.ecs.skipped_mut_resource", name = T::name())
            );
        }
        resource
    }
}

unsafe impl<T: 'static + Resource + Send + Sync> SystemParam for ResMut<T> {
    type This<'a> = ResMut<T>;
    type State = ParamResMutState<T>;
}
