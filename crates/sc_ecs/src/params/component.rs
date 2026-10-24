//! `Components<T>` 参数：按实体读取指定类型的组件。
//!
//! `Components<T>` 提供对世界内全部实体上的 `T` 组件的随机访问。
//!
//! The `Components<T>` parameter: reads components of a given type by entity.
//!
//! `Components<T>` provides random access to the `T` component of every entity
//! in the world.
use crate::component::Component;
use crate::entity::EntityId;
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;
use std::marker::PhantomData;
use std::sync::Arc;

/// `Components<T>` 参数：对世界内全部实体上的 `T` 组件的随机访问器。
///
/// Components parameter: random access to the `T` component of every entity
/// in the world.
pub struct Components<T: 'static + Component + Send + Sync> {
    /// 共享的世界句柄：组件的读取来源。
    ///
    /// The shared world handle: the source the components are read from.
    world: World,
    /// 组件类型占位。
    ///
    /// Phantom data for the component type.
    _phantom: PhantomData<T>,
}

impl<T: 'static + Component + Send + Sync> Components<T> {
    /// 由世界句柄创建一个 `Components<T>`。
    ///
    /// Creates a `Components<T>` from a world handle.
    ///
    /// - `world`: 组件所在的世界
    pub fn new(world: World) -> Self {
        Self {
            world,
            _phantom: Default::default(),
        }
    }

    /// 返回实体上 `T` 组件的共享引用；实体不存在或无该组件时返回 `None`。
    ///
    /// Returns the shared reference of the `T` component on an entity;
    /// returns `None` if the entity is absent or lacks the component.
    ///
    /// - `entity`: 目标实体 id
    pub fn get(&self, entity: &EntityId) -> Option<Arc<T>> {
        self.world.get_component(entity)
    }
}

/// `Components<T>` 的参数状态（无状态）。
///
/// The parameter state of `Components<T>` (stateless).
pub struct ParamComponentsState<T: 'static + Component + Send + Sync> {
    /// 组件类型占位。
    ///
    /// Phantom data for the component type.
    _phantom: PhantomData<T>,
}

impl<T: 'static + Component + Send + Sync> SystemParamState for ParamComponentsState<T> {
    type Item = Components<T>;

    /// 初始化参数状态。
    ///
    /// Initializes the parameter state.
    fn init() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }

    /// 基于世界句柄构造本次执行的参数。
    ///
    /// Builds the parameter for this run from the world handle.
    fn get_param<'a>(
        _state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        Some(Components::new(world.clone()))
    }
}

/// `Components<T>` 可作为系统参数注入。
///
/// `Components<T>` can be injected as a system parameter.
unsafe impl<T: 'static + Component + Send + Sync> SystemParam for Components<T> {
    type This<'a> = Components<T>;
    type State = ParamComponentsState<T>;
}
