//! `World`: the ECS storage core: entity table, component table, resource slots, event streams.
//!
//! Concurrency model (docs/ecs_concurrency.md):
//! - the four managers are guarded by `parking_lot::RwLock`, with guards held
//!   only briefly inside methods;
//! - components are stored as `Arc<dyn Component>` and `get_component`
//!   returns a cloned `Arc<C>` (safe across await; despawn only drops the
//!   storage-side reference);
//! - resource slots are type-erased `Arc<RwLock<R>>`, with `Res`/`ResMut` as
//!   owned guards.
use crate::bundle::Bundle;
use crate::component::{Component, ComponentManager};
use crate::entity::{EntityId, EntityLocation, EntityManager};
use crate::event::{Event, EventId, Events};
use crate::params::resource::{Res, ResMut};
use crate::resource::{Resource, ResourceManager};
use crate::system::{IntoSystem, SystemManager, SystemParam, SystemParamState};
use parking_lot::RwLock;
use std::sync::Arc;

/// Concurrency model:
/// - the four managers are guarded by real `parking_lot::RwLock`s, with guards
///   held only briefly inside `World` methods;
/// - components are stored as `Arc<dyn Component>` and `get_component`
///   returns a cloned `Arc<C>`; async tasks may hold it across await, and
///   entity despawn only drops the storage-side reference, never dangles;
/// - resources are accessed through `Res`/`ResMut` (owned arc guards), see
///   `resource.rs`.
#[derive(Clone)]
pub struct World {
    systems: Arc<RwLock<SystemManager>>,
    entities: Arc<RwLock<EntityManager>>,
    resources: Arc<RwLock<ResourceManager>>,
    components: Arc<RwLock<ComponentManager>>,
}

impl World {
    pub fn new() -> Self {
        Self {
            systems: Arc::new(RwLock::new(SystemManager::new())),
            entities: Arc::new(RwLock::new(EntityManager::new())),
            resources: Arc::new(RwLock::new(ResourceManager::new())),
            components: Arc::new(RwLock::new(ComponentManager::new())),
        }
    }

    pub fn add_systems<S, Marker>(&self, system: S) -> &Self
    where
        S: IntoSystem<S, Marker> + 'static,
        Marker: 'static,
    {
        self.systems.write().add_systems(system);
        self
    }

    pub fn insert_resource<R: Resource + 'static + Send + Sync>(&self, resource: R) -> &Self {
        self.resources.write().add_resource(resource);
        self
    }

    pub fn remove_resource<R: Resource + 'static + Send + Sync>(&self) -> &Self {
        self.resources.write().remove_resource::<R>();
        self
    }

    pub fn get_resource<R: Resource + 'static + Send + Sync>(&self) -> Option<Res<R>> {
        let lock = self.resources.read().get_lock::<R>()?;
        // read_arc_recursive: re-reading the same resource on one thread never
        // deadlocks against a queued writer.
        Some(Res::new(lock.read_arc_recursive()))
    }

    pub fn get_resource_mut<R: Resource + 'static + Send + Sync>(&self) -> Option<ResMut<R>> {
        let lock = self.resources.read().get_lock::<R>()?;
        Some(ResMut::new(lock.write_arc()))
    }

    pub fn spawn<B: Bundle>(&self, bundle: B) -> EntityId {
        let (id, location) = self.entities.write().spawn();
        let mut components = self.components.write();
        for component in bundle.get_components() {
            let component_id = components.add_box_component(component);
            components.bind_component(location, component_id);
        }
        id
    }

    pub fn pre_spawn(&self) -> (EntityId, EntityLocation) {
        self.entities.write().spawn()
    }

    pub fn spawn_with_location<B: Bundle>(&self, bundle: B, location: EntityLocation) -> &Self {
        let mut components = self.components.write();
        for component in bundle.get_components() {
            let component_id = components.add_box_component(component);
            components.bind_component(location, component_id);
        }
        self
    }

    /// Creates an entity from a (name, Arc<Component>) list (for cross-World
    /// migration; component instances are shared via Arc, with interior locks
    /// keeping mutable state safe, see docs/ecs_concurrency.md).
    pub fn spawn_with_components(&self, components: Vec<(String, Arc<dyn Component>)>) -> EntityId {
        let (entity, location) = self.pre_spawn();
        let mut component_manager = self.components.write();
        for (name, component) in components {
            let id = component_manager.add_arc_component(name, component);
            component_manager.bind_component(location, id);
        }
        entity
    }

    /// Moves an entity (with all its components) to the target World: rebuild
    /// the entity in the target, then despawn the source. Component instances
    /// are shared via Arc (no deep copy; extend `Component: Clone` and rebuild
    /// by value if isolated copies are needed).
    ///
    /// Used for moving entities across GameWorlds in the multi-world layout
    /// (docs/multi_world_ecs.md).
    pub fn transfer_entity(&self, target: &World, entity: &EntityId) -> Option<EntityId> {
        let location = self.entities.read().get_entity_location(entity)?;
        let components = self.components.read().components_of(&location)?;
        let new_entity = target.spawn_with_components(components);
        self.despawn(entity)?;
        Some(new_entity)
    }

    pub fn despawn(&self, entity: &EntityId) -> Option<()> {
        let location = self.entities.write().despawn(entity)?;
        self.components.write().remove_entity(&location)?;
        Some(())
    }

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

    /// Removes one component from an entity (returns `None` when the entity or
    /// the component is missing).
    ///
    /// Region migration (sc_ecs::region) uses it to detach the `EntityRegion`
    /// membership component; it also serves general component lifecycle use.
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

    /// Returns the shared handle of a component. Safe to hold across await: the
    /// handle stays valid after the entity is destroyed (data is freed with
    /// the last Arc) but is no longer reachable from the World.
    pub fn get_component<C: Component>(&self, entity: &EntityId) -> Option<Arc<C>> {
        let location = self.entities.read().get_entity_location(entity)?;
        self.components.read().get_component::<C>(location)
    }

    /// Returns every live entity that currently has component `C`.
    /// This is the backbone of the `Query<C>` system parameter.
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

    /// Number of entities with `C`, using the reverse index without allocating
    /// an intermediate entity-id/location vector.
    pub fn component_count<C: Component>(&self) -> usize {
        let components = self.components.read();
        if let Some(name) = C::name_static() {
            components.entities_with_count(name)
        } else {
            let name = C::name();
            components.entities_with_count(&name)
        }
    }

    pub fn send_event<E: Event + Send + Sync + 'static>(&self, event: E) -> Option<EventId> {
        let id = self.get_resource_mut::<Events<E>>()?.send(event);
        Some(id)
    }

    pub fn run(&self) {
        self.systems.write().run(self);
    }
}

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

unsafe impl SystemParam for World {
    type This<'a> = World;
    type State = ParamWorldState;
}
