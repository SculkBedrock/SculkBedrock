//! `Component`：实体组件 trait 与组件表存储。
//! 组件以 `Arc<dyn Component>` 存储；可变状态需由组件自身用内部锁保护。
//!
//! `Component`: entity component trait plus component table storage.
//! Components are stored as `Arc<dyn Component>`; mutable state must be
//! guarded by interior locks on the component itself.
use crate::entity::EntityLocation;
use ahash::{HashMap, HashMapExt};
use std::any::Any;
use std::sync::Arc;
pub use sc_ecs_macros::Component;

/// 组件 trait：所有实体组件的基础接口。
///
/// Component trait: the base interface of every entity component.
///
/// `Any` 超 trait 支持从 `Arc<dyn Component>` 向下转型到具体组件类型。
///
/// The `Any` supertrait enables downcasting from `Arc<dyn Component>` to
/// the concrete type.
pub trait Component: Any + Send + Sync {
    /// 返回组件的运行时名称（可能涉及分配）。
    ///
    /// Returns the runtime name of the component (may allocate).
    fn name() -> String
    where
        Self: Sized;

    /// derive 生成组件的静态名称；名称动态的手写组件可只实现 `name()`。
    ///
    /// Static name for derive-generated components; hand-written components
    /// with dynamic names may implement only `name()`.
    fn name_static() -> Option<&'static str>
    where
        Self: Sized,
    {
        None
    }
}

/// 组件 id：由（自增序号, 名称）构成的唯一标识。
///
/// Component id: a unique identifier composed of (monotonic index, name).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ComponentId(u32, Arc<str>);

/// 组件管理器：存放组件实例、实体绑定与查询索引。
///
/// Component manager: stores component instances, entity bindings and the
/// query index.
pub struct ComponentManager {
    /// 组件实例表：以 `ComponentId` 为键保存组件的共享引用。
    ///
    /// Component instance table: shared references of components keyed by `ComponentId`.
    components: HashMap<ComponentId, Arc<dyn Component>>,
    /// 实体绑定表：实体位置到（组件名 -> 组件 id）的映射。
    ///
    /// Entity binding table: maps an entity location to (component name -> component id).
    entity_bindings: HashMap<EntityLocation, HashMap<Arc<str>, ComponentId>>,
    /// 组件查询的反向索引（组件名 -> 实体位置集合）。
    ///
    /// Reverse index for component queries (component name -> entity locations).
    component_entities: HashMap<Arc<str>, HashMap<EntityLocation, ()>>,
    /// 单调递增的组件 id 计数器；id 永不重用。
    ///
    /// Monotonically increasing component id counter; ids are never reused.
    next_component_id: u32,
}

impl ComponentManager {
    /// 创建一个空的组件管理器。
    ///
    /// Creates an empty component manager.
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
            entity_bindings: Default::default(),
            component_entities: Default::default(),
            next_component_id: 0,
        }
    }

    /// 存入一个盒装组件并返回其 id。
    ///
    /// Stores a boxed component and returns its id.
    ///
    /// - `component`: （组件，名称）对
    pub fn add_box_component(&mut self, component: (Box<dyn Component>, String)) -> ComponentId {
        let id = ComponentId(self.next_component_id, Arc::from(component.1));
        self.next_component_id = self
            .next_component_id
            .checked_add(1)
            .expect("component id space exhausted");
        self.components.insert(id.clone(), Arc::from(component.0));
        id
    }

    /// 以 Arc 形式注册组件；id 语义与 `add_box_component` 相同。
    ///
    /// Registers a component as an Arc; same id semantics as `add_box_component`.
    ///
    /// - `name`: 组件名称
    /// - `component`: 组件实例的共享引用
    pub fn add_arc_component(
        &mut self,
        name: String,
        component: Arc<dyn Component>,
    ) -> ComponentId {
        let id = ComponentId(self.next_component_id, Arc::from(name));
        self.next_component_id = self
            .next_component_id
            .checked_add(1)
            .expect("component id space exhausted");
        self.components.insert(id.clone(), component);
        id
    }

    /// 快照某个实体的全部组件为（名称, Arc）对；Arc 共享同一实例，无拷贝。
    ///
    /// Snapshots every component of an entity as (name, Arc) pairs; the Arcs
    /// share the same instances, no copy.
    ///
    /// - `location`: 实体位置；无绑定时返回 `None`
    pub fn components_of(
        &self,
        location: &EntityLocation,
    ) -> Option<Vec<(String, Arc<dyn Component>)>> {
        let bindings = self.entity_bindings.get(location)?;
        let mut out = Vec::with_capacity(bindings.len());
        for (name, id) in bindings {
            if let Some(component) = self.components.get(id) {
                out.push((name.to_string(), component.clone()));
            }
        }
        Some(out)
    }

    /// 移除一个实体的全部组件绑定与存储；实体无绑定时返回 `None`。
    ///
    /// Removes all component bindings and storage of an entity; returns
    /// `None` if the entity has no bindings.
    ///
    /// - `location`: 实体位置
    pub fn remove_entity(&mut self, location: &EntityLocation) -> Option<()> {
        let entity_bindings = self.entity_bindings.remove(location)?;
        for (name, component_id) in entity_bindings {
            self.components.remove(&component_id);
            self.remove_entity_from_index(&name, *location);
        }
        Some(())
    }

    /// 尽可能压缩内部 HashMap 的容量。
    ///
    /// Shrinks the capacity of the internal HashMaps as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.components.shrink_to_fit();
        self.entity_bindings.shrink_to_fit();
        self.component_entities.shrink_to_fit();
        for entities in self.component_entities.values_mut() {
            entities.shrink_to_fit();
        }
    }

    /// 将组件绑定到实体；若同名组件已存在则替换之。
    ///
    /// Binds a component to an entity, replacing any existing component
    /// with the same name.
    ///
    /// - `entity`: 实体位置
    /// - `component`: 要绑定的组件 id
    pub fn bind_component(&mut self, entity: EntityLocation, component: ComponentId) {
        let name = Arc::clone(&component.1);
        let replaced = self
            .entity_bindings
            .entry(entity)
            .or_insert_with(HashMap::new)
            .insert(Arc::clone(&name), component.clone());
        if let Some(replaced) = replaced {
            // Replacing a binding must release the previous Arc storage, except
            // when callers rebind the same ComponentId.
            if replaced != component {
                self.components.remove(&replaced);
            }
        } else {
            self.component_entities
                .entry(name)
                .or_insert_with(HashMap::new)
                .insert(entity, ());
        }
    }

    /// 按组件名移除一条绑定并释放其存储（返回被移除的 ComponentId）。
    ///
    /// Removes one component binding by component name and releases its storage (returns the removed ComponentId).
    ///
    /// - `entity`: 实体位置
    /// - `name`: 组件名称；绑定不存在时返回 `None`
    pub fn remove_component_by_name(
        &mut self,
        entity: &EntityLocation,
        name: &str,
    ) -> Option<ComponentId> {
        let id = self.entity_bindings.get(entity)?.get(name)?.clone();
        self.unbind_component(*entity, id.clone());
        self.components.remove(&id);
        Some(id)
    }

    /// 解除组件与实体的绑定（保留组件存储本身）。
    ///
    /// Unbinds a component from an entity (keeps the component storage
    /// itself).
    ///
    /// - `entity`: 实体位置
    /// - `component`: 要解绑的组件 id
    pub fn unbind_component(&mut self, entity: EntityLocation, component: ComponentId) {
        let removed = self
            .entity_bindings
            .get_mut(&entity)
            .and_then(|bindings| bindings.remove(&component.1));
        if removed.is_some() {
            self.remove_entity_from_index(&component.1, entity);
        }
        if self
            .entity_bindings
            .get(&entity)
            .is_some_and(|bindings| bindings.is_empty())
        {
            self.entity_bindings.remove(&entity);
        }
    }

    /// 从反向索引中移除一个实体；若该组件名已无实体则删除整条目。
    ///
    /// Removes an entity from the reverse index; drops the whole entry
    /// once no entity remains for the component name.
    ///
    /// - `name`: 组件名称
    /// - `entity`: 实体位置
    fn remove_entity_from_index(&mut self, name: &Arc<str>, entity: EntityLocation) {
        let remove_name = self
            .component_entities
            .get_mut(name)
            .is_some_and(|entities| {
                entities.remove(&entity);
                entities.is_empty()
            });
        if remove_name {
            self.component_entities.remove(name);
        }
    }

    /// 获取一个实体的全部组件 id；实体无绑定时返回 `None`。
    ///
    /// Returns all component ids of an entity; returns `None` if the
    /// entity has no bindings.
    ///
    /// - `entity`: 实体位置
    pub fn get_component_ids(&self, entity: EntityLocation) -> Option<Vec<&ComponentId>> {
        self.entity_bindings
            .get(&entity)
            .map(|map| map.values().collect())
    }

    /// 返回当前持有指定名称组件的全部实体位置。
    ///
    /// Returns every entity location that currently has a component with the
    /// given name bound.
    ///
    /// - `component_name`: 组件名称
    pub fn entities_with(&self, component_name: &str) -> Vec<EntityLocation> {
        self.component_entities
            .get(component_name)
            .map(|entities| entities.keys().copied().collect())
            .unwrap_or_default()
    }

    /// 返回当前持有指定名称组件的实体数量。
    ///
    /// Returns the number of entities that currently have a component with
    /// the given name bound.
    ///
    /// - `component_name`: 组件名称
    pub fn entities_with_count(&self, component_name: &str) -> usize {
        self.component_entities
            .get(component_name)
            .map(HashMap::len)
            .unwrap_or(0)
    }

    /// 获取实体上某个类型组件的 `Arc<T>`；不存在或类型不匹配时返回 `None`。
    ///
    /// Returns the `Arc<T>` of a typed component on an entity; returns
    /// `None` if it is absent or of a different type.
    ///
    /// - `entity`: 实体位置
    pub fn get_component<T: Component>(&self, entity: EntityLocation) -> Option<Arc<T>> {
        let bindings = self.entity_bindings.get(&entity)?;
        let component_id = if let Some(name) = T::name_static() {
            bindings.get(name)?
        } else {
            let name = T::name();
            bindings.get(name.as_str())?
        };
        let component = self.components.get(component_id)?.clone();
        // Upcast dyn Component to dyn Any via supertrait upcasting, then
        // downcast to the concrete type; a type mismatch returns None.
        let any: Arc<dyn Any + Send + Sync> = component;
        any.downcast::<T>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityManager;

    struct TestValue(u32);

    impl Component for TestValue {
        fn name() -> String {
            "TestValue".to_string()
        }
    }

    #[test]
    fn component_query_index_tracks_add_replace_remove_and_despawn() {
        let mut manager = ComponentManager::new();
        let mut entities = EntityManager::new();
        let (_, first) = entities.spawn();
        let (_, second) = entities.spawn();

        let first_old = manager.add_box_component((Box::new(TestValue(1)), TestValue::name()));
        manager.bind_component(first, first_old);
        let second_id = manager.add_box_component((Box::new(TestValue(2)), TestValue::name()));
        manager.bind_component(second, second_id);
        assert_eq!(manager.entities_with("TestValue").len(), 2);

        let replacement = manager.add_box_component((Box::new(TestValue(3)), TestValue::name()));
        manager.bind_component(first, replacement);
        assert_eq!(
            manager.components.len(),
            2,
            "replacement releases old storage"
        );
        assert_eq!(manager.get_component::<TestValue>(first).unwrap().0, 3);
        assert_eq!(manager.entities_with("TestValue").len(), 2);

        manager.remove_component_by_name(&first, "TestValue");
        assert_eq!(manager.entities_with("TestValue"), vec![second]);
        manager.remove_entity(&second);
        assert!(manager.entities_with("TestValue").is_empty());
        assert!(manager.component_entities.is_empty());
        assert!(manager.components.is_empty());
    }
}
