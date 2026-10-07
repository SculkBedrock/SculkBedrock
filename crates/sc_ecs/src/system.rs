//! `System`: the system trait plus `SystemParam` injection.
//!
//! A system is the ECS execution unit: `run(&mut self, world)` consumes
//! components/resources/events from the World. `SystemParam` supports
//! Res/ResMut/World/Components/Query/EventReader/Local parameters; the
//! `async_system` macro runs a system on the shared tokio runtime.

use crate::world::World;
use crate::{
    impl_system_function, impl_system_param_state_tuple, impl_system_param_tuple,
    impl_tuple_system_function,
};
use std::marker::PhantomData;
use sc_ecs_macros::all_tuples;

pub trait System: Send + Sync {
    fn run(&mut self, world: &World);
}

impl System for () {
    fn run(&mut self, _world: &World) {}
}

pub unsafe trait SystemParam: Send + Sync {
    type This<'a>;
    type State: SystemParamState<Item = Self>;
}

all_tuples!(impl_system_param_tuple, 0, 20, P, p);

pub trait SystemParamState: Send + Sync {
    type Item: SystemParam<State = Self>;

    fn init() -> Self;

    fn get_param<'a>(
        state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>>;
}

all_tuples!(impl_system_param_state_tuple, 0, 20, P, p);

pub trait IntoSystem<S, Marker> {
    type System: System;

    fn into_system(self) -> Self::System;
}

impl<S, Marker> IntoSystem<S, Marker> for S
where
    Marker: 'static + Send + Sync,
    S: SystemFunction<Marker>,
{
    type System = FunctionSystem<Marker, S>;

    fn into_system(self) -> Self::System {
        FunctionSystem::new(self)
    }
}

pub trait SystemFunction<Marker>: 'static + Send + Sync {
    type Param: SystemParam;

    fn run(&mut self, param: <Self::Param as SystemParam>::This<'_>);
}

all_tuples!(impl_system_function, 0, 20, P, p);

pub struct FunctionSystem<Marker, F: SystemFunction<Marker>> {
    func: F,
    state: <F::Param as SystemParam>::State,
    _phantom_data: PhantomData<Marker>,
}

impl<Marker, F> FunctionSystem<Marker, F>
where
    F: SystemFunction<Marker>,
{
    pub fn new(func: F) -> Self {
        Self {
            func,
            state: <F::Param as SystemParam>::State::init(),
            _phantom_data: Default::default(),
        }
    }
}

impl<Marker: Send + Sync, F: SystemFunction<Marker>> System for FunctionSystem<Marker, F> {
    fn run(&mut self, world: &World) {
        if let Some(param) = <F::Param as SystemParam>::State::get_param(&mut self.state, world) {
            self.func.run(param);
        }
    }
}

pub struct TupleFunctionSystem {
    systems: Vec<Box<dyn System>>,
}

impl TupleFunctionSystem {
    pub fn new(systems: Vec<Box<dyn System>>) -> Self {
        Self { systems }
    }
}

impl System for TupleFunctionSystem {
    fn run(&mut self, world: &World) {
        for system in self.systems.iter_mut() {
            system.run(world);
        }
    }
}

all_tuples!(impl_tuple_system_function, 2, 20, P, p, M);

pub struct SystemManager {
    systems: Vec<Box<dyn System>>,
}

impl SystemManager {
    pub fn new() -> Self {
        Self { systems: vec![] }
    }

    pub fn add_systems<S, Marker>(&mut self, system: S)
    where
        S: IntoSystem<S, Marker> + 'static,
        Marker: 'static,
    {
        self.systems.push(Box::new(system.into_system()));
    }

    pub fn add_boxed(&mut self, system: Box<dyn System>) {
        self.systems.push(system);
    }

    /// Appends every system from another set (order preserved).
    pub fn append(&mut self, other: SystemManager) {
        self.systems.extend(other.systems);
    }

    pub fn run(&mut self, world: &World) {
        for system in self.systems.iter_mut() {
            system.run(world);
        }
    }
}
