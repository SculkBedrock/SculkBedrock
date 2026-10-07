//! `Query<T>` — iterate every entity that has component `T`.
//!
//! This is the read-side query primitive of the ECS: systems declare
//! `Query<T>` as a parameter and iterate `(EntityId, &T)` pairs without
//! knowing where entities came from. It follows the same pattern as
//! [`Components<T>`](crate::params::component::Components) (the param clones
//! the `Arc`-backed [`World`]), adding entity enumeration on top.
//!
//! ```ignore
//! fn greet_players(query: Query<MinecraftClient>) {
//!     for (entity, client) in query.iter() {
//!         // ...
//!     }
//! }
//! ```

use crate::component::Component;
use crate::entity::EntityId;
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;
use std::marker::PhantomData;
use std::sync::Arc;

pub struct Query<T: 'static + Component + Send + Sync> {
    world: World,
    _phantom: PhantomData<T>,
}

impl<T: 'static + Component + Send + Sync> Query<T> {
    pub fn new(world: World) -> Self {
        Self {
            world,
            _phantom: Default::default(),
        }
    }

    /// Entity ids that currently have `T`. Order is unspecified.
    pub fn entities(&self) -> Vec<EntityId> {
        self.world.entities_with_component::<T>()
    }

    pub fn get(&self, entity: &EntityId) -> Option<Arc<T>> {
        self.world.get_component(entity)
    }

    /// Fetch a different component of the same entity, for ad-hoc joins
    /// without a second query parameter.
    pub fn get_other<C: Component>(&self, entity: &EntityId) -> Option<Arc<C>> {
        self.world.get_component(entity)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntityId, Arc<T>)> + '_ {
        self.entities()
            .into_iter()
            .filter_map(move |entity| self.get(&entity).map(|component| (entity, component)))
    }

    pub fn len(&self) -> usize {
        self.world.component_count::<T>()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct ParamQueryState<T: 'static + Component + Send + Sync> {
    _phantom: PhantomData<T>,
}

impl<T: 'static + Component + Send + Sync> SystemParamState for ParamQueryState<T> {
    type Item = Query<T>;

    fn init() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }

    fn get_param<'a>(
        _state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        Some(Query::new(world.clone()))
    }
}

unsafe impl<T: 'static + Component + Send + Sync> SystemParam for Query<T> {
    type This<'a> = Query<T>;
    type State = ParamQueryState<T>;
}
