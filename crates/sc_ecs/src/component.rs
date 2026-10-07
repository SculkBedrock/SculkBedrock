//! `Component`: entity component trait plus component table storage.
//!
//! Components are stored as `Arc<dyn Component>` (the `Any` supertrait
//! allows safe downcasting). Mutable state lives behind interior locks on
//! the component itself.
use crate::entity::EntityLocation;
use ahash::{HashMap, HashMapExt};
use std::any::Any;
use std::sync::Arc;
pub use sc_ecs_macros::Component;

/// `Any` supertrait lets `Arc<dyn Component>` upcast safely to
/// `Arc<dyn Any + Send + Sync>` and then downcast.
pub trait Component: Any + Send + Sync {
    fn name() -> String
    where
        Self: Sized;

    /// Static name for derive-generated components. Hand-written components may
    /// keep the allocation-based `name()` fallback if their name is dynamic.
    fn name_static() -> Option<&'static str>
    where
        Self: Sized,
    {
        None
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ComponentId(u32, Arc<str>);

pub struct ComponentManager {
    components: HashMap<ComponentId, Arc<dyn Component>>,
    entity_bindings: HashMap<EntityLocation, HashMap<Arc<str>, ComponentId>>,
    /// Reverse index for component queries; avoids scanning every entity binding.
    component_entities: HashMap<Arc<str>, HashMap<EntityLocation, ()>>,
    /// Monotonically increasing component id counter. Ids are never reused,
    /// so a freed slot can never collide with a live component.
    next_component_id: u32,
}

impl ComponentManager {
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
            entity_bindings: Default::default(),
            component_entities: Default::default(),
            next_component_id: 0,
        }
    }

    pub fn add_box_component(&mut self, component: (Box<dyn Component>, String)) -> ComponentId {
        let id = ComponentId(self.next_component_id, Arc::from(component.1));
        self.next_component_id = self
            .next_component_id
            .checked_add(1)
            .expect("component id space exhausted");
        self.components.insert(id.clone(), Arc::from(component.0));
        id
    }

    /// Registers a component as an Arc (used for cross-World migration; same id semantics as `add_box_component`).
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

    /// Snapshots every component of an entity as (name, Arc) pairs (used for
    /// cross-World migration; the Arc shares one instance and interior locks
    /// keep mutable state safe, see docs/ecs_concurrency.md).
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

    pub fn remove_entity(&mut self, location: &EntityLocation) -> Option<()> {
        let entity_bindings = self.entity_bindings.remove(location)?;
        for (name, component_id) in entity_bindings {
            self.components.remove(&component_id);
            self.remove_entity_from_index(&name, *location);
        }
        Some(())
    }

    /// Shrinks the capacity of the internal HashMaps as much as possible.
    /// Useful for periodic memory maintenance after batch entity removals.
    pub fn shrink_to_fit(&mut self) {
        self.components.shrink_to_fit();
        self.entity_bindings.shrink_to_fit();
        self.component_entities.shrink_to_fit();
        for entities in self.component_entities.values_mut() {
            entities.shrink_to_fit();
        }
    }

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

    /// Removes one component binding by component name and releases its storage (returns the removed ComponentId).
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

    pub fn get_component_ids(&self, entity: EntityLocation) -> Option<Vec<&ComponentId>> {
        self.entity_bindings
            .get(&entity)
            .map(|map| map.values().collect())
    }

    /// Returns every entity location that currently has a component with the
    /// given name bound. Backs `World::entities_with_component` / `Query<T>`.
    pub fn entities_with(&self, component_name: &str) -> Vec<EntityLocation> {
        self.component_entities
            .get(component_name)
            .map(|entities| entities.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn entities_with_count(&self, component_name: &str) -> usize {
        self.component_entities
            .get(component_name)
            .map(HashMap::len)
            .unwrap_or(0)
    }

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
