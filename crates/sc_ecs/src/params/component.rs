use crate::component::Component;
use crate::entity::EntityId;
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;
use std::marker::PhantomData;
use std::sync::Arc;

pub struct Components<T: 'static + Component + Send + Sync> {
    world: World,
    _phantom: PhantomData<T>,
}

impl<T: 'static + Component + Send + Sync> Components<T> {
    pub fn new(world: World) -> Self {
        Self {
            world,
            _phantom: Default::default(),
        }
    }

    pub fn get(&self, entity: &EntityId) -> Option<Arc<T>> {
        self.world.get_component(entity)
    }
}

pub struct ParamComponentsState<T: 'static + Component + Send + Sync> {
    _phantom: PhantomData<T>,
}

impl<T: 'static + Component + Send + Sync> SystemParamState for ParamComponentsState<T> {
    type Item = Components<T>;

    fn init() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }

    fn get_param<'a>(
        _state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        Some(Components::new(world.clone()))
    }
}

unsafe impl<T: 'static + Component + Send + Sync> SystemParam for Components<T> {
    type This<'a> = Components<T>;
    type State = ParamComponentsState<T>;
}
