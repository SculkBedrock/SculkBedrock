//! `World`：ECS 存储核心：实体表、组件表、资源槽、事件流。
//!
//! `World`: the ECS storage core: entity table, component table, resource slots, event streams.
use crate::bundle::Bundle;
use crate::component::{Component, ComponentManager};
use crate::entity::{EntityId, EntityLocation, EntityManager};
use crate::event::{Event, EventId, Events};
use crate::params::resource::{Res, ResMut};
use crate::resource::{Resource, ResourceManager};
use crate::system::{IntoSystem, SystemManager, SystemParam, SystemParamState};
use parking_lot::RwLock;
use std::sync::Arc;

/// ECS 世界：实体/组件/资源/事件/系统的存储与访问入口。克隆共享；组件以
/// `Arc` 存储，`get_component` 返回 `Arc` 克隆，可跨 await 持有。
///
/// The ECS world: storage and access entry for entities/components/
/// resources/events/systems. Clones share the same storage; components are
/// stored as `Arc` and `get_component` returns an `Arc` clone that may be
/// held across await.
#[derive(Clone)]
pub struct World {
    /// 系统管理器（共享锁）。
    ///
    /// The system manager (shared lock).
    systems: Arc<RwLock<SystemManager>>,
    /// 实体管理器（共享锁）。
    ///
    /// The entity manager (shared lock).
    entities: Arc<RwLock<EntityManager>>,
    /// 资源管理器（共享锁）。
    ///
    /// The resource manager (shared lock).
    resources: Arc<RwLock<ResourceManager>>,
    /// 组件管理器（共享锁）。
    ///
    /// The component manager (shared lock).
    components: Arc<RwLock<ComponentManager>>,
}

impl World {
    /// 创建一个空的世界。
    ///
    /// Creates an empty world.
    pub fn new() -> Self {
        Self {
            systems: Arc::new(RwLock::new(SystemManager::new())),
            entities: Arc::new(RwLock::new(EntityManager::new())),
            resources: Arc::new(RwLock::new(ResourceManager::new())),
            components: Arc::new(RwLock::new(ComponentManager::new())),
        }
    }

    /// 注册一个系统，返回 `self` 以便链式调用。
    ///
    /// Registers a system and returns `self` for chaining.
    ///
    /// - `system`: 要注册的系统源
    pub fn add_systems<S, Marker>(&self, system: S) -> &Self
    where
        S: IntoSystem<S, Marker> + 'static,
        Marker: 'static,
    {
        self.systems.write().add_systems(system);
        self
    }

    /// 插入一个资源，返回 `self` 以便链式调用。
    ///
    /// Inserts a resource and returns `self` for chaining.
    ///
    /// - `resource`: 要插入的资源实例
    pub fn insert_resource<R: Resource + 'static + Send + Sync>(&self, resource: R) -> &Self {
        self.resources.write().add_resource(resource);
        self
    }

    /// 移除某个类型的资源，返回 `self` 以便链式调用；资源不存在时无操作。
    ///
    /// Removes the resource of a type and returns `self` for chaining;
    /// a no-op if the resource is absent.
    ///
    /// - `R`: 要移除的资源类型
    pub fn remove_resource<R: Resource + 'static + Send + Sync>(&self) -> &Self {
        self.resources.write().remove_resource::<R>();
        self
    }

    /// 获取资源的只读守卫；资源不存在时返回 `None`。
    ///
    /// Returns a read guard of a resource; returns `None` if the resource
    /// is absent.
    ///
    /// - `R`: 资源类型
    pub fn get_resource<R: Resource + 'static + Send + Sync>(&self) -> Option<Res<R>> {
        let lock = self.resources.read().get_lock::<R>()?;
        // read_arc_recursive: re-reading the same resource on one thread never
        // deadlocks against a queued writer.
        Some(Res::new(lock.read_arc_recursive()))
    }

    /// 获取资源的读写守卫；资源不存在时返回 `None`。
    ///
    /// Returns a read-write guard of a resource; returns `None` if the
    /// resource is absent.
    ///
    /// - `R`: 资源类型
    pub fn get_resource_mut<R: Resource + 'static + Send + Sync>(&self) -> Option<ResMut<R>> {
        let lock = self.resources.read().get_lock::<R>()?;
        Some(ResMut::new(lock.write_arc()))
    }

    /// 生成一个实体并附加组件束，返回实体 id。
    ///
    /// Spawns an entity with a bundle attached and returns its id.
    ///
    /// - `bundle`: 实体初始组件束
    pub fn spawn<B: Bundle>(&self, bundle: B) -> EntityId {
        let (id, location) = self.entities.write().spawn();
        let mut components = self.components.write();
        for component in bundle.get_components() {
            let component_id = components.add_box_component(component);
            components.bind_component(location, component_id);
        }
        id
    }

    /// 仅分配实体（不附加组件），返回 id 与位置。
    ///
    /// Allocates an entity only (without components) and returns its id
    /// and location.
    pub fn pre_spawn(&self) -> (EntityId, EntityLocation) {
        self.entities.write().spawn()
    }

    /// 在指定位置上生成实体并附加组件束，返回 `self` 以便链式调用。
    ///
    /// Spawns an entity at a given location with a bundle attached and
    /// returns `self` for chaining.
    ///
    /// - `bundle`: 实体初始组件束
    /// - `location`: 目标位置
    pub fn spawn_with_location<B: Bundle>(&self, bundle: B, location: EntityLocation) -> &Self {
        let mut components = self.components.write();
        for component in bundle.get_components() {
            let component_id = components.add_box_component(component);
            components.bind_component(location, component_id);
        }
        self
    }

    /// 由（名称, Arc<Component>）列表创建实体；组件实例通过 Arc 共享。
    ///
    /// Creates an entity from a (name, Arc<Component>) list; component
    /// instances are shared via Arc.
    ///
    /// - `components`: 组件（名称, 共享引用）列表
    pub fn spawn_with_components(&self, components: Vec<(String, Arc<dyn Component>)>) -> EntityId {
        let (entity, location) = self.pre_spawn();
        let mut component_manager = self.components.write();
        for (name, component) in components {
            let id = component_manager.add_arc_component(name, component);
            component_manager.bind_component(location, id);
        }
        entity
    }

    /// 把实体（含全部组件）迁移到目标 World：先在目标重建，再 despawn 来源；
    /// 组件实例通过 Arc 共享（无深拷贝）。实体或组件缺失时返回 `None`。
    ///
    /// Moves an entity (with all its components) to the target World: rebuild
    /// the entity in the target, then despawn the source; component instances
    /// are shared via Arc (no deep copy). Returns `None` if the entity or a
    /// component lookup fails.
    ///
    /// - `target`: 目标世界
    /// - `entity`: 要迁移的实体 id
    pub fn transfer_entity(&self, target: &World, entity: &EntityId) -> Option<EntityId> {
        let location = self.entities.read().get_entity_location(entity)?;
        let components = self.components.read().components_of(&location)?;
        let new_entity = target.spawn_with_components(components);
        self.despawn(entity)?;
        Some(new_entity)
    }

    /// 销毁一个实体及其全部组件；实体不存在时返回 `None`。
    ///
    /// Destroys an entity and all its components; returns `None` if the
    /// entity is absent.
    ///
    /// - `entity`: 要销毁的实体 id
    pub fn despawn(&self, entity: &EntityId) -> Option<()> {
        let location = self.entities.write().despawn(entity)?;
        self.components.write().remove_entity(&location)?;
        Some(())
    }

    /// 给实体附加组件束，返回 `self` 以便链式调用；实体不存在时无操作。
    ///
    /// Attaches a bundle to an entity and returns `self` for chaining;
    /// a no-op if the entity is absent.
    ///
    /// - `entity`: 目标实体 id
    /// - `bundle`: 要附加的组件束
    pub fn add_component<B: Bundle>(&self, entity: &EntityId, bundle: B) -> &Self {
        if let Some(location) = self.entities.read().get_entity_location(entity) {
            let mut components = self.components.write();
            for component in bundle.get_components() {
                let component_id = components.add_box_component(component);
                components.bind_component(location, component_id);
            }
        }
        self
    }

    /// 从实体上移除一个类型的组件（实体或组件缺失时返回 `None`）。
    ///
    /// Removes one component from an entity (returns `None` when the entity or
    /// the component is missing).
    ///
    /// - `entity`: 目标实体 id
    /// - `C`: 要移除的组件类型
    pub fn remove_component<C: Component>(&self, entity: &EntityId) -> Option<()> {
        let location = self.entities.read().get_entity_location(entity)?;
        let mut components = self.components.write();
        if let Some(name) = C::name_static() {
            components.remove_component_by_name(&location, name)?;
        } else {
            let name = C::name();
            components.remove_component_by_name(&location, &name)?;
        }
        Some(())
    }

    /// 返回组件的共享句柄。可跨 await 持有；实体销毁后句柄仍有效，但不再
    /// 从 World 可达。
    ///
    /// Returns the shared handle of a component. May be held across await;
    /// after the entity is destroyed the handle stays valid but is no longer
    /// reachable from the World.
    ///
    /// - `entity`: 实体 id；组件不存在或类型不匹配时返回 `None`
    pub fn get_component<C: Component>(&self, entity: &EntityId) -> Option<Arc<C>> {
        let location = self.entities.read().get_entity_location(entity)?;
        self.components.read().get_component::<C>(location)
    }

    /// 返回当前持有组件 `C` 的全部存活实体。
    ///
    /// Returns every live entity that currently has component `C`.
    pub fn entities_with_component<C: Component>(&self) -> Vec<EntityId> {
        let locations = {
            let components = self.components.read();
            if let Some(name) = C::name_static() {
                components.entities_with(name)
            } else {
                let name = C::name();
                components.entities_with(&name)
            }
        };
        let entities = self.entities.read();
        locations
            .into_iter()
            .filter_map(|location| entities.get_entity_id(&location))
            .collect()
    }

    /// 持有 `C` 的实体数量。
    ///
    /// Number of entities with `C`.
    pub fn component_count<C: Component>(&self) -> usize {
        let components = self.components.read();
        if let Some(name) = C::name_static() {
            components.entities_with_count(name)
        } else {
            let name = C::name();
            components.entities_with_count(&name)
        }
    }

    /// 发送一个事件，返回其事件 id；事件流资源不存在时返回 `None`。
    ///
    /// Sends an event and returns its event id; returns `None` if the
    /// event stream resource is absent.
    ///
    /// - `event`: 要发送的事件
    pub fn send_event<E: Event + Send + Sync + 'static>(&self, event: E) -> Option<EventId> {
        let id = self.get_resource_mut::<Events<E>>()?.send(event);
        Some(id)
    }

    /// 运行全部系统一次。
    ///
    /// Runs all systems once.
    pub fn run(&self) {
        self.systems.write().run(self);
    }
}

/// World 作为系统参数时的状态（无状态）。
///
/// The state of `World` as a system parameter (stateless).
pub struct ParamWorldState;

impl SystemParamState for ParamWorldState {
    type Item = World;

    fn init() -> Self {
        ParamWorldState
    }

    fn get_param<'a>(
        _state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        Some(world.clone())
    }
}

/// World 本身可作为系统参数注入（克隆共享）。
///
/// `World` itself can be injected as a system parameter (cloned and
/// shared).
unsafe impl SystemParam for World {
    type This<'a> = World;
    type State = ParamWorldState;
}
