//! `Query<T>` — 遍历所有持有组件 `T` 的实体。
//!
//! 作为系统参数使用，迭代 `(EntityId, Arc<T>)` 对。
//!
//! ```ignore
//! fn greet_players(query: Query<PlayerName>) {
//!     for (entity, name) in query.iter() {
//!         // ...
//!     }
//! }
//! ```
//!
//! `Query<T>`: the read-side query parameter of the ECS. Systems declare it
//! and iterate `(EntityId, Arc<T>)` pairs.

use crate::component::Component;
use crate::entity::EntityId;
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;
use std::marker::PhantomData;
use std::sync::Arc;

/// 查询参数：按组件类型枚举实体并读取其组件。
///
/// Query parameter: enumerates entities by component type and reads their
/// components.
pub struct Query<T: 'static + Component + Send + Sync> {
    /// 共享的世界句柄：查询的数据来源。
    ///
    /// The shared world handle: the source of the queried data.
    world: World,
    /// 组件类型占位。
    ///
    /// Phantom data for the component type.
    _phantom: PhantomData<T>,
}

impl<T: 'static + Component + Send + Sync> Query<T> {
    /// 由世界句柄创建一个 `Query<T>`。
    ///
    /// Creates a `Query<T>` from a world handle.
    ///
    /// - `world`: 查询所在的世界
    pub fn new(world: World) -> Self {
        Self {
            world,
            _phantom: Default::default(),
        }
    }

    /// 当前持有 `T` 的实体 id 列表；顺序未定义。
    ///
    /// Entity ids that currently have `T`. Order is unspecified.
    pub fn entities(&self) -> Vec<EntityId> {
        self.world.entities_with_component::<T>()
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

    /// 获取同一实体的另一种组件，用于免去第二个查询参数的临时联表。
    ///
    /// Fetches a different component of the same entity, for ad-hoc joins
    /// without a second query parameter.
    ///
    /// - `entity`: 目标实体 id
    /// - `C`: 要读取的组件类型
    pub fn get_other<C: Component>(&self, entity: &EntityId) -> Option<Arc<C>> {
        self.world.get_component(entity)
    }

    /// 迭代所有持有 `T` 的实体及其组件；遍历期间组件被移除的实体会被跳过。
    ///
    /// Iterates every entity holding `T` together with its component; entities
    /// whose component is removed during the iteration are skipped.
    pub fn iter(&self) -> impl Iterator<Item = (EntityId, Arc<T>)> + '_ {
        self.entities()
            .into_iter()
            .filter_map(move |entity| self.get(&entity).map(|component| (entity, component)))
    }

    /// 当前持有 `T` 的实体数量。
    ///
    /// The number of entities currently holding `T`.
    pub fn len(&self) -> usize {
        self.world.component_count::<T>()
    }

    /// 是否没有任何实体持有 `T`。
    ///
    /// Whether no entity currently holds `T`.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// `Query<T>` 的参数状态（无状态）。
///
/// The parameter state of `Query<T>` (stateless).
pub struct ParamQueryState<T: 'static + Component + Send + Sync> {
    /// 组件类型占位。
    ///
    /// Phantom data for the component type.
    _phantom: PhantomData<T>,
}

impl<T: 'static + Component + Send + Sync> SystemParamState for ParamQueryState<T> {
    type Item = Query<T>;

    /// 初始化参数状态。
    ///
    /// Initializes the parameter state.
    fn init() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }

    /// 基于世界句柄构造本次执行的查询参数。
    ///
    /// Builds the query parameter for this run from the world handle.
    fn get_param<'a>(
        _state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        Some(Query::new(world.clone()))
    }
}

/// `Query<T>` 可作为系统参数注入。
///
/// `Query<T>` can be injected as a system parameter.
unsafe impl<T: 'static + Component + Send + Sync> SystemParam for Query<T> {
    type This<'a> = Query<T>;
    type State = ParamQueryState<T>;
}
